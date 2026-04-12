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
use kodo_store::auth;
use kodo_store::crypto::KeychainStore;
use kodo_store::db;
use kodo_ui::command::Command;
use kodo_ui::event::{EventHandler, map_event};
use kodo_ui::message::Message;
use kodo_ui::model::Model;
use kodo_ui::tui::{init_terminal, restore_terminal, view};
use kodo_ui::update::update;

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
        "anthropic" => Ok(Arc::new(AnthropicProvider::from_env_or_empty())),
        "openai" => Ok(Arc::new(OpenAiProvider::from_env_or_empty())),
        "gemini" => Ok(Arc::new(GeminiProvider::from_env_or_empty())),
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
/// Agent communication types
#[derive(Debug)]
enum AgentRequest {
    ProcessMessage(String),
    ClearConversation,
    SetModel(String),
    ListProviders,
    LogoutProvider(String),
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

    // Initialize terminal
    let mut terminal = init_terminal()?;
    let mut events = EventHandler::new(Duration::from_millis(100));

    // Initialize application state (Model) following Elm Architecture
    let mut model = Model::new(cli.debug);
    model.provider = agent.provider_name().to_string();
    model.model_name = agent.model().to_string();
    model.mode = agent.mode.to_string();

    if cli.debug {
        model.debug_panel_open = true;
        model
            .debug_logs
            .push("Debug mode enabled. Toggle panel with F12".to_string());
    }

    // Channel for agent events -> TUI
    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentEvent>();
    // Channel for TUI -> agent requests
    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<AgentRequest>();

    // Spawn agent task
    let agent_event_tx = agent_tx.clone();
    tokio::spawn(async move {
        while let Some(request) = req_rx.recv().await {
            match request {
                AgentRequest::ProcessMessage(input) => {
                    if let Err(e) = agent.process_message(&input, Some(&agent_event_tx)).await {
                        let _ = agent_event_tx.send(AgentEvent::Error(format!("{e:#}")));
                    }
                    // Always send Done so the TUI knows processing finished
                    let _ = agent_event_tx.send(AgentEvent::Done);
                }
                AgentRequest::ClearConversation => {
                    agent.clear_conversation();
                }
                AgentRequest::SetModel(model) => {
                    agent.set_model(model);
                }
                AgentRequest::ListProviders => match load_configured_providers().await {
                    Ok(providers) => {
                        let _ = agent_event_tx.send(AgentEvent::ProvidersListed(providers));
                    }
                    Err(error) => {
                        let _ = agent_event_tx.send(AgentEvent::Error(format!("{error:#}")));
                    }
                },
                AgentRequest::LogoutProvider(account_id) => {
                    match logout_provider(&account_id).await {
                        Ok(()) => {
                            let _ = agent_event_tx.send(AgentEvent::LogoutComplete(account_id));
                        }
                        Err(error) => {
                            let _ = agent_event_tx.send(AgentEvent::Error(format!("{error:#}")));
                        }
                    }
                }
                AgentRequest::Quit => {
                    agent.shutdown_lsp().await;
                    break;
                }
            }
        }
    });

    // Main MVU (Model-View-Update) runtime loop following Elm Architecture
    loop {
        // VIEW: Render current model state
        terminal.draw(|frame| view(frame, &model))?;

        // HANDLE EVENTS: Process input and agent events
        tokio::select! {
            // Terminal/keyboard events -> Messages -> Update
            event_result = events.next() => {
                let event = event_result?;

                // Map crossterm Event to application Message
                if let Some(message) = map_event(&event, &model) {
                    // UPDATE: Pure function that modifies model and returns commands
                    let commands = update(&mut model, message);

                    // EXECUTE COMMANDS: Handle side effects
                    for command in commands {
                        execute_command(command, &req_tx).await;
                    }
                }
            }

            // Agent events -> Messages -> Update
            Some(agent_event) = agent_rx.recv() => {
                // Map AgentEvent to application Message
                let message = map_agent_event(agent_event);

                // UPDATE: Pure function that modifies model and returns commands
                let commands = update(&mut model, message);

                // EXECUTE COMMANDS: Handle side effects
                for command in commands {
                    execute_command(command, &req_tx).await;
                }
            }
        }

        // Check if application should quit
        if model.should_quit {
            let _ = req_tx.send(AgentRequest::Quit);
            break;
        }
    }

    // Cleanup: Restore terminal to normal mode
    restore_terminal(&mut terminal)?;
    Ok(())
}

/// Execute Commands returned by update() - this is where side effects happen.
/// The update() function is pure, but Commands describe side effects that
/// the runtime must perform (sending messages to agents, quitting, etc).
async fn execute_command(command: Command, req_tx: &mpsc::UnboundedSender<AgentRequest>) {
    match command {
        Command::SendToAgent(message) => {
            let _ = req_tx.send(AgentRequest::ProcessMessage(message));
        }
        Command::ClearConversation => {
            let _ = req_tx.send(AgentRequest::ClearConversation);
        }
        Command::SetModel(model) => {
            let _ = req_tx.send(AgentRequest::SetModel(model));
        }
        Command::ListProviders => {
            let _ = req_tx.send(AgentRequest::ListProviders);
        }
        Command::LogoutProvider(account_id) => {
            let _ = req_tx.send(AgentRequest::LogoutProvider(account_id));
        }
        Command::Quit => {
            let _ = req_tx.send(AgentRequest::Quit);
        }
        Command::None => {
            // No-op
        }
    }
}

/// Map AgentEvent to application Message.
/// This converts external agent events into internal application messages
/// that can be processed by the pure update() function.
fn map_agent_event(event: AgentEvent) -> Message {
    match event {
        AgentEvent::TextDelta(chunk) => Message::AgentTextDelta(chunk),
        AgentEvent::TextDone => Message::AgentTextDone,
        AgentEvent::ToolStart { name } => Message::AgentToolStart { name },
        AgentEvent::ToolDone { name, success } => Message::AgentToolDone { name, success },
        AgentEvent::ToolDenied { name, reason } => Message::AgentToolDenied { name, reason },
        AgentEvent::ToolCancelled { name } => Message::AgentToolCancelled { name },
        AgentEvent::Formatted { message } => Message::AgentFormatted { message },
        AgentEvent::Diagnostics { summary, count } => Message::AgentDiagnostics { summary, count },
        AgentEvent::Error(error) => Message::AgentError(error),
        AgentEvent::Notice(message) => Message::Notice(message),
        AgentEvent::ProvidersListed(providers) => Message::ProvidersListed(providers),
        AgentEvent::LogoutComplete(account_id) => Message::LogoutComplete(account_id),
        AgentEvent::Done => Message::AgentDone,
    }
}

async fn load_configured_providers() -> Result<Vec<String>> {
    let pool = db::open(&db::default_db_path()).await?;
    auth::list_providers(&pool).await
}

async fn logout_provider(account_id: &str) -> Result<()> {
    let pool = db::open(&db::default_db_path()).await?;
    let store = KeychainStore;
    auth::delete_token(&pool, &store, account_id).await
}
