use std::io::{self, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use tracing::debug;

use kodo_fmt::registry::FormatterRegistry;
use kodo_fmt::runner::format_file;
use kodo_llm::provider::Provider;
use kodo_llm::types::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, StreamEvent, ToolDefinition,
};
use kodo_tools::registry::ToolRegistry;
use kodo_tools::tool::ToolContext;

use crate::checkpoint::CheckpointManager;
use crate::context::ContextTracker;
use crate::mode::Mode;
use crate::safety;

const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Tools that produce file writes, triggering the formatter.
const FILE_WRITE_TOOLS: &[&str] = &["file_write", "file_edit"];

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
    formatter_registry: FormatterRegistry,
    checkpoints: CheckpointManager,
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
            formatter_registry: FormatterRegistry::with_builtins(),
            checkpoints: CheckpointManager::new(),
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

    /// Access the formatter registry for customization.
    pub fn formatter_registry_mut(&mut self) -> &mut FormatterRegistry {
        &mut self.formatter_registry
    }

    /// Get the context tracker for display purposes.
    pub fn context(&self) -> &ContextTracker {
        &self.context
    }

    /// Read-only access to the checkpoint manager.
    pub fn checkpoints(&self) -> &CheckpointManager {
        &self.checkpoints
    }

    /// Undo the most recent file edit.
    pub async fn undo(&mut self) -> Result<String> {
        self.checkpoints.undo_last().await
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
        // Only expose tools that the current mode permits.
        let mode = self.mode;
        let tools: Vec<ToolDefinition> = self
            .tool_registry
            .tool_definitions_filtered(|level| mode.allows(level))
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
    async fn handle_tool_calls(
        &mut self,
        assistant_message: &Message,
    ) -> Result<Vec<ContentBlock>> {
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

                // Enforce mode restrictions.
                if let Some(tool) = self.tool_registry.get(name)
                    && !self.mode.allows(tool.permission_level())
                {
                    let msg = format!(
                        "Tool '{}' requires {:?} permission, which is not allowed in {} mode.",
                        name,
                        tool.permission_level(),
                        self.mode
                    );
                    eprintln!("\n  [denied: {name} — {msg}]");
                    results.push(ContentBlock::tool_result(id, &msg, true));
                    continue;
                }

                // Check for high-risk shell commands in Build mode.
                if name == "shell"
                    && let Some(command) = input.get("command").and_then(|v| v.as_str())
                    && let Some(reason) = safety::check_high_risk(command)
                    && !safety::prompt_confirmation(
                        "shell",
                        &format!("{reason}\n  Command: {command}"),
                    )
                {
                    eprintln!("  [cancelled: {name}]");
                    results.push(ContentBlock::tool_result(
                        id,
                        "User denied execution of high-risk command.",
                        true,
                    ));
                    continue;
                }

                // Snapshot file before write/edit for undo support.
                if FILE_WRITE_TOOLS.contains(&name.as_str())
                    && let Some(path_str) = input.get("path").and_then(|v| v.as_str())
                {
                    let path = PathBuf::from(path_str);
                    if let Err(e) = self.checkpoints.snapshot(&path).await {
                        debug!(error = %e, "failed to save checkpoint");
                    }
                }

                eprintln!("\n  [tool: {name}]");

                match self.tool_registry.execute(name, input.clone(), &ctx).await {
                    Ok(output) => {
                        debug!(tool = %name, success = output.success, "tool completed");

                        // Run formatter after file write/edit tools.
                        if output.success && FILE_WRITE_TOOLS.contains(&name.as_str()) {
                            self.maybe_format_file(name, input).await;
                        }

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

    /// Attempt to format a file after a write/edit tool has modified it.
    /// Runs silently — logs to UI but doesn't feed back to LLM.
    async fn maybe_format_file(&self, tool_name: &str, input: &serde_json::Value) {
        let path_str = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return,
        };

        let path = PathBuf::from(path_str);

        if let Some(result) = format_file(&self.formatter_registry, &path).await {
            if result.success {
                eprintln!("  [fmt: {}]", result.message);
            } else {
                debug!(
                    tool = tool_name,
                    error = %result.message,
                    "formatter failed after {}",
                    tool_name
                );
            }
        }
    }
}
