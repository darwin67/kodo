use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Parser;
use tokio::sync::mpsc;

use kodo_core::agent::{Agent, AgentEvent};
use kodo_llm::anthropic::AnthropicProvider;
use kodo_llm::gemini::GeminiProvider;
use kodo_llm::ollama::OllamaProvider;
use kodo_llm::openai::OpenAiProvider;
use kodo_llm::provider::Provider;
use kodo_tui::app::{self, Action, App, ChatRole};
use kodo_tui::event::EventHandler;
use kodo_tui::theme::Theme;

#[derive(Parser)]
#[command(name = "kodo", about = "A coding agent for your terminal")]
struct Cli {
    /// Model to use (e.g. claude-sonnet-4-20250514, gpt-4o, gemini-2.5-flash)
    #[arg(long, short)]
    model: Option<String>,

    /// Provider to use: anthropic, openai, gemini, ollama
    #[arg(long, short)]
    provider: Option<String>,

    /// Enable debug mode (shows debug side panel with Ctrl+\)
    #[arg(long)]
    debug: bool,
}

fn create_provider(name: &str) -> Result<Arc<dyn Provider>> {
    match name {
        "anthropic" => Ok(Arc::new(AnthropicProvider::from_env()?)),
        "openai" => Ok(Arc::new(OpenAiProvider::from_env()?)),
        "gemini" => Ok(Arc::new(GeminiProvider::from_env()?)),
        "ollama" => Ok(Arc::new(OllamaProvider::from_env())),
        _ => bail!("unknown provider: {name}. Available: anthropic, openai, gemini, ollama"),
    }
}

fn default_model(provider_name: &str) -> &str {
    match provider_name {
        "anthropic" => "claude-sonnet-4-20250514",
        "openai" => "gpt-4o",
        "gemini" => "gemini-2.5-flash",
        "ollama" => "llama3.1",
        _ => "claude-sonnet-4-20250514",
    }
}

/// Messages sent from the TUI event loop to the agent task.
enum AgentRequest {
    ProcessMessage(String),
    Quit,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .with_writer(std::io::stderr) // Log to stderr, not stdout (TUI owns stdout).
        .init();

    let cli = Cli::parse();

    let provider_name = cli.provider.as_deref().unwrap_or("anthropic");
    let provider = create_provider(provider_name)?;
    let model = cli
        .model
        .unwrap_or_else(|| default_model(provider_name).to_string());

    let mut agent = Agent::new(provider).with_model(&model);
    kodo_tools::register_builtin_tools(agent.tool_registry_mut());

    // Initialize TUI.
    let mut terminal = app::init_terminal()?;
    let mut events = EventHandler::new(Duration::from_millis(100));

    let mut tui_app = App::new();
    tui_app.provider = agent.provider_name().to_string();
    tui_app.model = agent.model().to_string();
    tui_app.mode = agent.mode.to_string();
    tui_app.debug_mode = cli.debug;
    if cli.debug {
        tui_app.debug_panel_open = true;
        tui_app.push_debug_log("Debug mode enabled. Toggle panel with Ctrl+\\");
    }

    // Channel for agent events -> TUI.
    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentEvent>();
    // Channel for TUI -> agent requests.
    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<AgentRequest>();

    // Spawn agent task.
    let agent_event_tx = agent_tx.clone();
    tokio::spawn(async move {
        while let Some(request) = req_rx.recv().await {
            match request {
                AgentRequest::ProcessMessage(input) => {
                    if let Err(e) = agent.process_message(&input, Some(&agent_event_tx)).await {
                        let _ = agent_event_tx.send(AgentEvent::Error(format!("{e:#}")));
                    }
                    // Always send Done so the TUI knows processing finished.
                    let _ = agent_event_tx.send(AgentEvent::Done);
                }
                AgentRequest::Quit => break,
            }
        }
    });

    // Main TUI event loop.
    loop {
        // Draw.
        terminal.draw(|frame| app::render(frame, &tui_app))?;

        // Process events with a non-blocking select.
        tokio::select! {
            // Terminal/keyboard events.
            event = events.next() => {
                let event = event?;
                let action = app::handle_key(&mut tui_app, &event);

                match action {
                    Action::Submit(input) => {
                        tui_app.push_message(ChatRole::User, &input);
                        tui_app.is_streaming = true;
                        tui_app.streaming_text.clear();
                        let _ = req_tx.send(AgentRequest::ProcessMessage(input));
                    }
                    Action::Quit => {
                        let _ = req_tx.send(AgentRequest::Quit);
                        break;
                    }
                    Action::ToggleMode => {
                        let new_mode = if tui_app.mode == "plan" { "build" } else { "plan" };
                        tui_app.mode = new_mode.into();
                        tui_app.push_debug_log(format!("Mode toggled to {new_mode}"));
                    }
                    Action::ToggleDebugPanel => {
                        tui_app.debug_panel_open = !tui_app.debug_panel_open;
                    }
                    Action::PaletteCommand(cmd) => {
                        tui_app.push_debug_log(format!("Palette command: {cmd}"));
                        handle_palette_command(&mut tui_app, &cmd);
                    }
                    Action::None => {}
                }
            }
            // Agent events (streaming text, tool status, etc.).
            Some(agent_event) = agent_rx.recv() => {
                // Log all agent events to debug panel.
                tui_app.push_debug_log(format!("{agent_event:?}"));

                match agent_event {
                    AgentEvent::TextDelta(text) => {
                        tui_app.append_streaming(&text);
                    }
                    AgentEvent::TextDone => {
                        tui_app.finish_streaming();
                    }
                    AgentEvent::ToolStart { name } => {
                        tui_app.push_message(ChatRole::Tool, format!("[{name}]"));
                    }
                    AgentEvent::ToolDone { name, success } => {
                        let status = if success { "done" } else { "failed" };
                        tui_app.push_message(ChatRole::Tool, format!("[{name}: {status}]"));
                    }
                    AgentEvent::ToolDenied { name, reason } => {
                        tui_app.push_message(ChatRole::Tool, format!("[denied: {name}] {reason}"));
                    }
                    AgentEvent::ToolCancelled { name } => {
                        tui_app.push_message(ChatRole::Tool, format!("[cancelled: {name}]"));
                    }
                    AgentEvent::Formatted { message } => {
                        tui_app.push_message(ChatRole::Tool, format!("[fmt: {message}]"));
                    }
                    AgentEvent::Error(msg) => {
                        tui_app.push_message(ChatRole::System, format!("Error: {msg}"));
                        tui_app.is_streaming = false;
                    }
                    AgentEvent::Done => {
                        if tui_app.is_streaming {
                            tui_app.finish_streaming();
                        }
                    }
                }
            }
        }
    }

    // Restore terminal.
    app::restore_terminal(&mut terminal)?;

    Ok(())
}

fn handle_palette_command(app: &mut App, cmd: &str) {
    match cmd {
        "Dark Theme" => {
            app.theme = Theme::dark();
            app.push_message(ChatRole::System, "Switched to dark theme.");
        }
        "Light Theme" => {
            app.theme = Theme::light();
            app.push_message(ChatRole::System, "Switched to light theme.");
        }
        "Quit" => {
            app.should_quit = true;
        }
        _ => {
            app.push_message(
                ChatRole::System,
                format!("Command not yet implemented: {cmd}"),
            );
        }
    }
}
