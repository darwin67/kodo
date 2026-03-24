use std::io::{self, Write};
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use tracing::debug;

use kodo_llm::provider::Provider;
use kodo_llm::types::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, StreamEvent, ToolDefinition,
};
use kodo_tools::registry::ToolRegistry;
use kodo_tools::tool::ToolContext;

use crate::context::ContextTracker;
use crate::mode::Mode;

const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const DEFAULT_MAX_TOKENS: u32 = 8192;

const SYSTEM_PROMPT: &str = "\
You are Kodo, a coding assistant that runs in the user's terminal. \
You help with software engineering tasks: writing code, fixing bugs, \
explaining code, running commands, and more.

You have access to tools for reading files, editing files, searching \
codebases, running shell commands, and fetching web content. Use them \
as needed to accomplish the user's request.

Be concise and direct. Focus on solving the problem.";

/// The main agent that orchestrates the agentic loop.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tool_registry: ToolRegistry,
    messages: Vec<Message>,
    context: ContextTracker,
    pub mode: Mode,
    model: String,
    system_prompt: String,
}

impl Agent {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            tool_registry: ToolRegistry::new(),
            messages: Vec::new(),
            context: ContextTracker::new(),
            mode: Mode::default(),
            model: DEFAULT_MODEL.to_string(),
            system_prompt: SYSTEM_PROMPT.to_string(),
        }
    }

    /// Set the model to use.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set the operating mode.
    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    /// Access the tool registry for registering tools.
    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tool_registry
    }

    /// Read-only access to the tool registry.
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// Get the context tracker for display purposes.
    pub fn context(&self) -> &ContextTracker {
        &self.context
    }

    /// Process a user message through the agentic loop.
    ///
    /// This sends the message to the LLM, streams the response, handles any
    /// tool calls, and loops until the model produces a final response.
    pub async fn process_message(&mut self, user_input: &str) -> Result<()> {
        self.messages.push(Message::user(user_input));

        loop {
            let request = self.build_request();

            debug!(
                model = %request.model,
                messages = request.messages.len(),
                tools = request.tools.len(),
                "sending request to LLM"
            );

            let mut stream = self.provider.stream(request).await?;

            let (assistant_message, stop_reason) = self.consume_stream(&mut stream).await?;

            self.messages.push(assistant_message.clone());

            match stop_reason {
                StopReason::ToolUse => {
                    let tool_results = self.handle_tool_calls(&assistant_message).await?;
                    if !tool_results.is_empty() {
                        self.messages.push(Message::tool_results(tool_results));
                    }
                    // Loop back to send tool results to the LLM.
                }
                StopReason::EndTurn | StopReason::MaxTokens => {
                    // Done.
                    break;
                }
            }
        }

        Ok(())
    }

    /// Build a completion request from the current conversation state.
    fn build_request(&self) -> CompletionRequest {
        let tools: Vec<ToolDefinition> = self
            .tool_registry
            .tool_definitions()
            .into_iter()
            .map(|v| serde_json::from_value(v).unwrap())
            .collect();

        CompletionRequest {
            model: self.model.clone(),
            system: Some(self.system_prompt.clone()),
            messages: self.messages.clone(),
            tools,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// Consume a stream of events, printing text deltas and building the
    /// complete assistant message. Returns the message and stop reason.
    async fn consume_stream(
        &mut self,
        stream: &mut Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>,
    ) -> Result<(Message, StopReason)> {
        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut current_text = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_input_json = String::new();
        let mut stop_reason = StopReason::EndTurn;

        while let Some(event) = stream.next().await {
            let event = event?;

            match event {
                StreamEvent::MessageStart { usage } => {
                    self.context.record(&usage);
                }
                StreamEvent::TextDelta { text } => {
                    print!("{text}");
                    io::stdout().flush()?;
                    current_text.push_str(&text);
                }
                StreamEvent::ToolUseStart { id, name } => {
                    // If we were accumulating text, push it as a block.
                    if !current_text.is_empty() {
                        content_blocks.push(ContentBlock::text(&current_text));
                        current_text.clear();
                    }
                    current_tool_id = id;
                    current_tool_name = name;
                    current_tool_input_json.clear();
                }
                StreamEvent::ToolInputDelta { json } => {
                    current_tool_input_json.push_str(&json);
                }
                StreamEvent::BlockStop => {
                    if !current_tool_id.is_empty() {
                        let input: serde_json::Value =
                            serde_json::from_str(&current_tool_input_json)
                                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        content_blocks.push(ContentBlock::tool_use(
                            &current_tool_id,
                            &current_tool_name,
                            input,
                        ));
                        current_tool_id.clear();
                        current_tool_name.clear();
                        current_tool_input_json.clear();
                    }
                }
                StreamEvent::MessageDone {
                    stop_reason: sr,
                    usage,
                } => {
                    self.context.record(&usage);
                    stop_reason = sr;
                }
            }
        }

        // Push any remaining text.
        if !current_text.is_empty() {
            content_blocks.push(ContentBlock::text(&current_text));
            // Newline after the streamed text.
            println!();
        }

        let message = Message {
            role: Role::Assistant,
            content: content_blocks,
        };

        Ok((message, stop_reason))
    }

    /// Execute tool calls from an assistant message and return the results.
    async fn handle_tool_calls(&self, assistant_message: &Message) -> Result<Vec<ContentBlock>> {
        let tool_uses = assistant_message.tool_uses();
        if tool_uses.is_empty() {
            return Ok(vec![]);
        }

        let ctx = ToolContext {
            working_dir: std::env::current_dir()?,
        };

        let mut results = Vec::new();

        for block in tool_uses {
            if let ContentBlock::ToolUse { id, name, input } = block {
                debug!(tool = %name, id = %id, "executing tool");
                eprintln!("\n  [tool: {name}]");

                match self.tool_registry.execute(name, input.clone(), &ctx).await {
                    Ok(output) => {
                        debug!(tool = %name, success = output.success, "tool completed");
                        results.push(ContentBlock::tool_result(
                            id,
                            &output.content,
                            !output.success,
                        ));
                    }
                    Err(e) => {
                        debug!(tool = %name, error = %e, "tool failed");
                        results.push(ContentBlock::tool_result(id, format!("Error: {e}"), true));
                    }
                }
            }
        }

        Ok(results)
    }
}
