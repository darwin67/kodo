use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use futures::StreamExt;
use tokio::sync::mpsc;
use tracing::debug;

use kodo_fmt::registry::FormatterRegistry;
use kodo_fmt::runner::format_file;
use kodo_llm::provider::Provider;
use kodo_llm::types::{
    CompletionRequest, ContentBlock, Message, Role, StopReason, StreamEvent, ToolDefinition,
};
use kodo_lsp::diagnostics;
use kodo_lsp::manager::LspManager;
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

// ---------------------------------------------------------------------------
// Agent events — emitted to the UI layer
// ---------------------------------------------------------------------------

/// Events emitted by the agent during message processing.
/// These replace direct stdout/stderr writes, allowing the TUI to render them.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A chunk of streamed text from the assistant.
    TextDelta(String),
    /// Assistant finished streaming text.
    TextDone,
    /// A tool is about to be executed.
    ToolStart { name: String },
    /// A tool was denied (mode restriction).
    ToolDenied { name: String, reason: String },
    /// A tool was cancelled by user (high-risk).
    ToolCancelled { name: String },
    /// A tool completed.
    ToolDone { name: String, success: bool },
    /// Formatter ran on a file.
    Formatted { message: String },
    /// LSP diagnostics collected after a file change.
    Diagnostics { summary: String, count: usize },
    /// An error occurred.
    Error(String),
    /// Message processing is complete.
    Done,
}

/// Sender type for agent events.
pub type AgentEventTx = mpsc::UnboundedSender<AgentEvent>;

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// The main agent that orchestrates the agentic loop.
pub struct Agent {
    provider: Arc<dyn Provider>,
    tool_registry: ToolRegistry,
    formatter_registry: FormatterRegistry,
    lsp_manager: LspManager,
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
            lsp_manager: LspManager::new(std::env::current_dir().unwrap_or_default()),
            checkpoints: CheckpointManager::new(),
            messages: Vec::new(),
            context: ContextTracker::new(),
            mode: Mode::default(),
            model: DEFAULT_MODEL.to_string(),
            system_prompt: SYSTEM_PROMPT.to_string(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tool_registry
    }

    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    pub fn formatter_registry_mut(&mut self) -> &mut FormatterRegistry {
        &mut self.formatter_registry
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn set_provider(&mut self, provider: Arc<dyn Provider>) {
        self.provider = provider;
        self.messages.clear();
    }

    pub fn context(&self) -> &ContextTracker {
        &self.context
    }

    pub fn checkpoints(&self) -> &CheckpointManager {
        &self.checkpoints
    }

    pub async fn undo(&mut self) -> Result<String> {
        self.checkpoints.undo_last().await
    }

    /// Process a user message through the agentic loop.
    /// Emits `AgentEvent`s to the provided sender for UI rendering.
    /// If no sender is provided, events are silently discarded (headless mode).
    pub async fn process_message(
        &mut self,
        user_input: &str,
        tx: Option<&AgentEventTx>,
    ) -> Result<()> {
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

            let (assistant_message, stop_reason) = self.consume_stream(&mut stream, tx).await?;

            self.messages.push(assistant_message.clone());

            match stop_reason {
                StopReason::ToolUse => {
                    let tool_results = self.handle_tool_calls(&assistant_message, tx).await?;
                    if !tool_results.is_empty() {
                        self.messages.push(Message::tool_results(tool_results));
                    }
                }
                StopReason::EndTurn | StopReason::MaxTokens => {
                    emit(tx, AgentEvent::Done);
                    break;
                }
            }
        }

        Ok(())
    }

    fn build_request(&self) -> CompletionRequest {
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

    async fn consume_stream(
        &mut self,
        stream: &mut Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>,
        tx: Option<&AgentEventTx>,
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
                    emit(tx, AgentEvent::TextDelta(text.clone()));
                    current_text.push_str(&text);
                }
                StreamEvent::ToolUseStart { id, name } => {
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

        if !current_text.is_empty() {
            content_blocks.push(ContentBlock::text(&current_text));
            emit(tx, AgentEvent::TextDone);
        }

        let message = Message {
            role: Role::Assistant,
            content: content_blocks,
        };

        Ok((message, stop_reason))
    }

    async fn handle_tool_calls(
        &mut self,
        assistant_message: &Message,
        tx: Option<&AgentEventTx>,
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
                    emit(
                        tx,
                        AgentEvent::ToolDenied {
                            name: name.clone(),
                            reason: msg.clone(),
                        },
                    );
                    results.push(ContentBlock::tool_result(id, &msg, true));
                    continue;
                }

                // Check for high-risk shell commands.
                if name == "shell"
                    && let Some(command) = input.get("command").and_then(|v| v.as_str())
                    && let Some(reason) = safety::check_high_risk(command)
                    && !safety::prompt_confirmation(
                        "shell",
                        &format!("{reason}\n  Command: {command}"),
                    )
                {
                    emit(tx, AgentEvent::ToolCancelled { name: name.clone() });
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

                emit(tx, AgentEvent::ToolStart { name: name.clone() });

                match self.tool_registry.execute(name, input.clone(), &ctx).await {
                    Ok(output) => {
                        debug!(tool = %name, success = output.success, "tool completed");

                        // Run formatter and collect LSP diagnostics after file write/edit.
                        if output.success && FILE_WRITE_TOOLS.contains(&name.as_str()) {
                            self.maybe_format_file(name, input, tx).await;
                            self.maybe_collect_diagnostics(input, tx).await;
                        }

                        emit(
                            tx,
                            AgentEvent::ToolDone {
                                name: name.clone(),
                                success: output.success,
                            },
                        );

                        results.push(ContentBlock::tool_result(
                            id,
                            &output.content,
                            !output.success,
                        ));
                    }
                    Err(e) => {
                        debug!(tool = %name, error = %e, "tool failed");
                        emit(tx, AgentEvent::Error(format!("Tool '{name}' failed: {e}")));
                        results.push(ContentBlock::tool_result(id, format!("Error: {e}"), true));
                    }
                }
            }
        }

        Ok(results)
    }

    async fn maybe_format_file(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        tx: Option<&AgentEventTx>,
    ) {
        let path_str = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return,
        };

        let path = PathBuf::from(path_str);

        if let Some(result) = format_file(&self.formatter_registry, &path).await {
            if result.success {
                emit(
                    tx,
                    AgentEvent::Formatted {
                        message: result.message,
                    },
                );
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

    /// Collect LSP diagnostics after a file was written/edited.
    /// Notifies the LSP of the change and injects any diagnostics into context.
    async fn maybe_collect_diagnostics(
        &mut self,
        input: &serde_json::Value,
        tx: Option<&AgentEventTx>,
    ) {
        let path_str = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return,
        };

        let path = PathBuf::from(path_str);

        if !self.lsp_manager.has_server_for(&path) {
            return;
        }

        // Read the file content after formatting.
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(_) => return,
        };

        match self
            .lsp_manager
            .diagnostics_after_change(&path, &content)
            .await
        {
            Ok(diags) if !diags.is_empty() => {
                let summary = diagnostics::format_diagnostics(&diags);
                let count = diags.len();
                debug!(file = %path.display(), count, "LSP diagnostics collected");
                emit(
                    tx,
                    AgentEvent::Diagnostics {
                        summary: summary.clone(),
                        count,
                    },
                );
                // Inject diagnostics into the conversation so the LLM can see them.
                self.messages.push(Message::user(format!(
                    "[LSP diagnostics after editing {}]\n{}",
                    path.display(),
                    summary
                )));
            }
            Ok(_) => {
                debug!(file = %path.display(), "no LSP diagnostics");
            }
            Err(e) => {
                debug!(error = %e, "failed to collect LSP diagnostics");
            }
        }
    }

    /// Access the LSP manager.
    pub fn lsp_manager(&self) -> &LspManager {
        &self.lsp_manager
    }

    /// Shut down all LSP servers (call on session end).
    pub async fn shutdown_lsp(&mut self) {
        self.lsp_manager.shutdown_all().await;
    }
}

/// Emit an event to the UI channel (if provided).
fn emit(tx: Option<&AgentEventTx>, event: AgentEvent) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}
