use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Parser;
use rpassword::prompt_password;
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
use kodo_store::oauth::ProviderOAuthConfig;
use kodo_ui::command::Command;
use kodo_ui::event::{EventHandler, map_event};
use kodo_ui::message::Message;
use kodo_ui::model::{ChatMessage, ChatRole, Model};
use kodo_ui::skills::{default_skill_dirs, load_skills};
use kodo_ui::tui::{init_terminal, restore_terminal, view};
use kodo_ui::update::update;

const EVENT_TICK_RATE: Duration = Duration::from_millis(100);

#[derive(Parser)]
#[command(name = "kodo", about = "A coding agent for your terminal")]
struct Cli {
    /// Model to use (e.g. claude-sonnet-4-20250514, gpt-4o, gemini-2.5-flash)
    #[arg(long, short)]
    model: Option<String>,

    /// Provider to use: anthropic, openai, gemini, ollama
    #[arg(long, short)]
    provider: Option<String>,

    /// Enable in-chat debug logging at startup
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
    SetProvider(String),
    SetModel(String),
    ListModels,
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
    let mut events = EventHandler::new(EVENT_TICK_RATE);

    // Initialize application state (Model) following Elm Architecture
    let mut model = Model::new(cli.debug);
    model.provider = agent.provider_name().to_string();
    model.model_name = agent.model().to_string();
    model.mode = agent.mode.to_string();
    let (personal_skill_dir, project_skill_dir) = default_skill_dirs();
    model.commands =
        kodo_ui::slash::merge_commands(load_skills(&personal_skill_dir, &project_skill_dir));

    if cli.debug {
        model.messages.push(ChatMessage {
            role: ChatRole::System,
            content: "[debug] Debug logging enabled at startup.".to_string(),
        });
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
                AgentRequest::SetProvider(provider_name) => match create_provider(&provider_name) {
                    Ok(provider) => {
                        let model_name = default_model(&provider_name).to_string();
                        agent.set_provider(provider);
                        agent.set_model(model_name.clone());
                        let _ = agent_event_tx.send(AgentEvent::ProviderChanged {
                            provider: provider_name,
                            model: model_name,
                        });
                    }
                    Err(error) => {
                        let _ = agent_event_tx.send(AgentEvent::Error(format!("{error:#}")));
                    }
                },
                AgentRequest::SetModel(model) => {
                    match agent.list_models().await {
                        Ok(models) => {
                            if models.iter().any(|candidate| candidate.id == model) {
                                agent.set_model(model.clone());
                                let _ = agent_event_tx.send(AgentEvent::ModelChanged(model));
                            } else {
                                let _ = agent_event_tx.send(AgentEvent::Error(format!(
                                    "Model `{model}` is not available for provider `{}` with the current credentials.",
                                    agent.provider_name()
                                )));
                            }
                        }
                        Err(error) => {
                            let _ = agent_event_tx.send(AgentEvent::Error(format!("{error:#}")));
                        }
                    }
                }
                AgentRequest::ListModels => match agent.list_models().await {
                    Ok(models) => {
                        let _ = agent_event_tx.send(AgentEvent::ModelsListed {
                            current_model: agent.model().to_string(),
                            models: models.into_iter().map(|model| model.id).collect(),
                        });
                    }
                    Err(error) => {
                        let _ = agent_event_tx.send(AgentEvent::Error(format!("{error:#}")));
                    }
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
                        execute_command(
                            command,
                            &req_tx,
                            &agent_tx,
                            &mut terminal,
                            &mut events,
                        )
                        .await;
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
                    execute_command(
                        command,
                        &req_tx,
                        &agent_tx,
                        &mut terminal,
                        &mut events,
                    )
                    .await;
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
async fn execute_command(
    command: Command,
    req_tx: &mpsc::UnboundedSender<AgentRequest>,
    agent_tx: &mpsc::UnboundedSender<AgentEvent>,
    terminal: &mut kodo_ui::Tui,
    events: &mut EventHandler,
) {
    match command {
        Command::SendToAgent(message) => {
            let _ = req_tx.send(AgentRequest::ProcessMessage(message));
        }
        Command::ClearConversation => {
            let _ = req_tx.send(AgentRequest::ClearConversation);
        }
        Command::SetProvider(provider) => {
            let _ = req_tx.send(AgentRequest::SetProvider(provider));
        }
        Command::SetModel(model) => {
            let _ = req_tx.send(AgentRequest::SetModel(model));
        }
        Command::ListModels => {
            let _ = req_tx.send(AgentRequest::ListModels);
        }
        Command::ListProviders => {
            let _ = req_tx.send(AgentRequest::ListProviders);
        }
        Command::LoginProvider { provider, name } => {
            if provider == "openai" {
                start_openai_login(agent_tx.clone(), name);
            } else if let Err(error) =
                handle_login_command(terminal, events, agent_tx, &provider, name.clone()).await
            {
                let _ = agent_tx.send(AgentEvent::Error(format!("{error:#}")));
            }
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

fn start_openai_login(agent_tx: mpsc::UnboundedSender<AgentEvent>, name: Option<String>) {
    let _ = agent_tx.send(AgentEvent::Notice(
        "Starting OpenAI login. Finish the OAuth flow in your browser.".to_string(),
    ));
    tokio::spawn(async move {
        match login_openai_provider(&agent_tx).await {
            Ok(account_id) => {
                let _ = agent_tx.send(AgentEvent::LoginComplete { account_id, name });
            }
            Err(error) => {
                let _ = agent_tx.send(AgentEvent::Error(format!("{error:#}")));
            }
        }
    });
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
        AgentEvent::ModelsListed {
            current_model,
            models,
        } => Message::ModelsListed {
            current_model,
            models,
        },
        AgentEvent::ProvidersListed(providers) => Message::ProvidersListed(providers),
        AgentEvent::ModelChanged(model_id) => Message::ModelChanged(model_id),
        AgentEvent::ProviderChanged { provider, model } => {
            Message::ProviderChanged { provider, model }
        }
        AgentEvent::LoginComplete { account_id, name } => {
            Message::LoginComplete { account_id, name }
        }
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

async fn handle_login_command(
    terminal: &mut kodo_ui::Tui,
    events: &mut EventHandler,
    agent_tx: &mpsc::UnboundedSender<AgentEvent>,
    provider: &str,
    name: Option<String>,
) -> Result<()> {
    events.shutdown();
    restore_terminal(terminal)?;

    let login_result = login_provider(provider).await;

    *terminal = init_terminal()?;
    *events = EventHandler::new(EVENT_TICK_RATE);

    match login_result {
        Ok(account_id) => {
            let _ = agent_tx.send(AgentEvent::LoginComplete { account_id, name });
            Ok(())
        }
        Err(error) => Err(error),
    }
}

async fn login_provider(provider: &str) -> Result<String> {
    let pool = db::open(&db::default_db_path()).await?;
    let store = KeychainStore;

    match provider {
        "openai" => {
            return login_openai_provider_silent().await;
        }
        "anthropic" | "gemini" => {
            let prompt = match provider {
                "anthropic" => "Anthropic API key: ",
                "gemini" => "Gemini API key: ",
                _ => unreachable!(),
            };

            eprintln!("Logging in to `{provider}`. Credentials are stored in the OS keychain.");
            let secret = prompt_password(prompt)?;
            let secret = secret.trim().to_string();
            if secret.is_empty() {
                bail!("No credential entered.");
            }

            auth::save_token(&pool, &store, provider, &secret, None, None, None).await?;
        }
        "ollama" => bail!("`ollama` does not require login."),
        other => bail!(
            "Unknown provider `{other}`. Available providers: anthropic, openai, gemini, ollama."
        ),
    }

    Ok(provider.to_string())
}

async fn login_openai_provider(agent_tx: &mpsc::UnboundedSender<AgentEvent>) -> Result<String> {
    let pool = db::open(&db::default_db_path()).await?;
    let store = KeychainStore;
    let tx = agent_tx.clone();
    let tokens = kodo_store::oauth::run_openai_oauth_flow_with_notifier(
        &ProviderOAuthConfig::openai_default(),
        Box::new(move |notice| {
            let _ = tx.send(AgentEvent::Notice(notice));
        }),
    )
    .await?;

    let metadata = tokens
        .id_token
        .as_deref()
        .map(kodo_store::oauth::parse_openai_id_token_metadata)
        .transpose()?;
    let _ = agent_tx.send(AgentEvent::Notice(
        "OpenAI OAuth tokens received. Saving credentials to the OS keychain...".to_string(),
    ));

    let expires_at = tokens.expires_in.map(format_oauth_expiry);
    auth::save_token(
        &pool,
        &store,
        "openai",
        &tokens.access_token,
        tokens.refresh_token.as_deref(),
        expires_at.as_deref(),
        metadata.as_ref().and_then(|meta| meta.chatgpt_account_id.as_deref()),
    )
    .await?;

    Ok("openai".to_string())
}

async fn login_openai_provider_silent() -> Result<String> {
    let pool = db::open(&db::default_db_path()).await?;
    let store = KeychainStore;
    let tokens =
        kodo_store::oauth::run_openai_oauth_flow(&ProviderOAuthConfig::openai_default()).await?;
    let metadata = tokens
        .id_token
        .as_deref()
        .map(kodo_store::oauth::parse_openai_id_token_metadata)
        .transpose()?;

    let expires_at = tokens.expires_in.map(format_oauth_expiry);
    auth::save_token(
        &pool,
        &store,
        "openai",
        &tokens.access_token,
        tokens.refresh_token.as_deref(),
        expires_at.as_deref(),
        metadata.as_ref().and_then(|meta| meta.chatgpt_account_id.as_deref()),
    )
    .await?;

    Ok("openai".to_string())
}

fn format_oauth_expiry(expires_in: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(expires_in)).to_rfc3339()
}
