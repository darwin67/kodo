use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Parser;
use tokio::sync::{Mutex, mpsc};

use kodo_auth::{
    AuthConfig, AuthProvider, AuthToken,
    oauth::{OAuthProvider, PendingCodePaste},
    storage::TokenStorage,
};
use kodo_config::{Config, loader::load_or_default};

/// Global storage for a pending OAuth code-paste flow.
/// This is set when the user starts a code-paste flow and consumed
/// when they submit the authorization code.
static PENDING_CODE_PASTE: std::sync::LazyLock<Mutex<Option<PendingCodePaste>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
use kodo_core::agent::{Agent, AgentEvent};
use kodo_llm::anthropic::AnthropicProvider;
use kodo_llm::gemini::GeminiProvider;
use kodo_llm::ollama::OllamaProvider;
use kodo_llm::openai::OpenAiProvider;
use kodo_llm::provider::Provider;
use kodo_ui::command::Command;
use kodo_ui::event::{EventHandler, map_event};
use kodo_ui::message::Message;
use kodo_ui::model::{AuthMethod, Model, ProviderModalState, ProviderOption};
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

    /// Authenticate with a provider (anthropic, openai)
    #[arg(long)]
    auth: Option<String>,
}

fn create_provider(name: &str, api_key: Option<&str>) -> Result<Arc<dyn Provider>> {
    match name {
        "anthropic" => {
            if let Some(key) = api_key {
                Ok(Arc::new(AnthropicProvider::new(key.to_string())))
            } else {
                Ok(Arc::new(AnthropicProvider::from_env()?))
            }
        }
        "openai" => {
            if let Some(key) = api_key {
                Ok(Arc::new(OpenAiProvider::new(key.to_string())))
            } else {
                Ok(Arc::new(OpenAiProvider::from_env()?))
            }
        }
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
#[derive(Debug)]
enum AgentRequest {
    ProcessMessage(String),
    /// Replace the current agent with a new provider/model
    SwitchProvider {
        provider_name: String,
        model: String,
        api_key: String,
    },
    Quit,
}

/// Path to the last-used session file for persisting provider/model choice
fn session_state_path() -> std::path::PathBuf {
    let config_dir = dirs_next::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("kodo");
    let _ = std::fs::create_dir_all(&config_dir);
    config_dir.join("last_session.json")
}

/// Persisted session state (last used provider and model)
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SessionState {
    provider: Option<String>,
    model: Option<String>,
}

fn load_session_state() -> SessionState {
    let path = session_state_path();
    if let Ok(data) = std::fs::read_to_string(&path) {
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        SessionState::default()
    }
}

fn save_session_state(provider: &str, model: &str) {
    let state = SessionState {
        provider: Some(provider.to_string()),
        model: Some(model.to_string()),
    };
    let path = session_state_path();
    if let Ok(data) = serde_json::to_string_pretty(&state) {
        let _ = std::fs::write(&path, data);
    }
}

/// Build the list of available provider options for the connect modal
async fn build_provider_options() -> Vec<ProviderOption> {
    let storage = TokenStorage::new("kodo");

    let anthropic_authed = std::env::var("ANTHROPIC_API_KEY").is_ok()
        || storage.get("anthropic").await.ok().flatten().is_some();

    let openai_authed = std::env::var("OPENAI_API_KEY").is_ok()
        || storage.get("openai").await.ok().flatten().is_some();

    let openai_config = AuthConfig::openai();

    let anthropic_config = AuthConfig::anthropic();

    vec![
        ProviderOption {
            id: "anthropic".to_string(),
            display_name: "Anthropic (Claude)".to_string(),
            auth_methods: if anthropic_config.supports_oauth() {
                vec![AuthMethod::OAuthCodePaste, AuthMethod::ApiKey]
            } else {
                vec![AuthMethod::ApiKey]
            },
            is_authenticated: anthropic_authed,
        },
        ProviderOption {
            id: "openai".to_string(),
            display_name: "OpenAI (GPT / o-series)".to_string(),
            auth_methods: if openai_config.supports_oauth() {
                vec![AuthMethod::OAuth, AuthMethod::ApiKey]
            } else {
                vec![AuthMethod::ApiKey]
            },
            is_authenticated: openai_authed,
        },
        ProviderOption {
            id: "gemini".to_string(),
            display_name: "Google (Gemini)".to_string(),
            auth_methods: vec![AuthMethod::ApiKey],
            is_authenticated: std::env::var("GEMINI_API_KEY").is_ok()
                || std::env::var("GOOGLE_API_KEY").is_ok(),
        },
        ProviderOption {
            id: "ollama".to_string(),
            display_name: "Ollama (Local)".to_string(),
            auth_methods: vec![],   // No auth needed
            is_authenticated: true, // Always available
        },
    ]
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Handle --auth command (non-TUI flow)
    if let Some(auth_provider) = cli.auth {
        handle_auth(&auth_provider).await?;
        return Ok(());
    }

    // Try to setup OAuth tokens as env vars from stored credentials
    setup_oauth_tokens().await?;

    // Load configuration
    let config = load_or_default();

    // Load last session state for provider/model defaults
    let session_state = load_session_state();

    // Determine provider and model from: CLI args > last session > config > defaults
    let provider_name = cli
        .provider
        .clone()
        .or_else(|| session_state.provider.clone())
        .unwrap_or_else(|| config.general.default_provider.clone());

    // Try to create the provider. If it fails, we'll launch TUI in "needs provider" mode.
    let maybe_provider = create_provider(&provider_name, None);

    let model_name = cli.model.clone().unwrap_or_else(|| {
        session_state
            .model
            .clone()
            .or_else(|| {
                config
                    .providers
                    .get(&provider_name)
                    .and_then(|p| p.default_model.clone())
            })
            .unwrap_or_else(|| {
                if config.general.default_model.is_empty() {
                    default_model(&provider_name).to_string()
                } else {
                    config.general.default_model.clone()
                }
            })
    });

    // Initialize terminal
    let mut terminal = init_terminal()?;
    let mut events = EventHandler::new(Duration::from_millis(100));

    // Initialize application state
    let debug_enabled = cli.debug || config.general.debug;
    let mut model = Model::new(debug_enabled);

    // Build provider options for the connect modal
    let provider_options = build_provider_options().await;
    model.provider_options = provider_options;

    // Channel for agent events -> TUI
    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentEvent>();
    // Channel for TUI -> agent requests
    let (req_tx, mut req_rx) = mpsc::unbounded_channel::<AgentRequest>();
    // Channel for OAuth/provider messages back to TUI
    let (ui_msg_tx, mut ui_msg_rx) = mpsc::unbounded_channel::<Message>();

    let has_provider = maybe_provider.is_ok();

    if let Ok(provider) = maybe_provider {
        // Provider available, set up agent
        let mut agent = Agent::new(provider).with_model(&model_name);
        agent.set_max_concurrent_subagents(config.general.max_subagents);
        agent.configure_lsp_servers(&config.lsp_servers, config.general.auto_install_lsp);
        agent.configure_formatters(&config.formatters);
        kodo_tools::register_builtin_tools(agent.tool_registry_mut());

        model.provider = agent.provider_name().to_string();
        model.model_name = agent.model().to_string();
        model.mode = agent.mode.to_string();

        let context = agent.context();
        model.context_tokens = context.current_conversation_tokens;
        model.context_limit = context.current_model_limit;

        // Save this as the last used session
        save_session_state(&model.provider, &model.model_name);

        // Spawn agent task
        let agent_event_tx = agent_tx.clone();
        let config_clone = config.clone();
        tokio::spawn(async move {
            run_agent_loop(agent, &mut req_rx, &agent_event_tx, &config_clone).await;
        });
    } else {
        // No provider available - show the connect modal
        model.needs_provider = true;
        model.provider_modal = ProviderModalState::SelectProvider;
        model.provider = "none".to_string();
        model.model_name = "none".to_string();

        // Spawn agent task that will wait for a provider switch
        let agent_event_tx = agent_tx.clone();
        let config_clone = config.clone();
        tokio::spawn(async move {
            run_agent_loop_deferred(&mut req_rx, &agent_event_tx, &config_clone).await;
        });
    }

    if debug_enabled {
        model.debug_panel_open = true;
        model
            .debug_logs
            .push("Debug mode enabled. Toggle panel with F12".to_string());
        if !has_provider {
            model
                .debug_logs
                .push("No provider authenticated. Showing connect modal.".to_string());
        }
    }

    // Main MVU runtime loop
    loop {
        // VIEW: Render current model state
        terminal.draw(|frame| view(frame, &model))?;

        // HANDLE EVENTS
        tokio::select! {
            // Terminal/keyboard events
            event_result = events.next() => {
                let event = event_result?;
                if let Some(message) = map_event(&event, &model) {
                    let commands = update(&mut model, message);
                    for command in commands {
                        execute_command(command, &req_tx, &ui_msg_tx).await;
                    }
                }
            }

            // Agent events
            Some(agent_event) = agent_rx.recv() => {
                let message = map_agent_event(agent_event);
                let commands = update(&mut model, message);
                for command in commands {
                    execute_command(command, &req_tx, &ui_msg_tx).await;
                }
            }

            // OAuth/provider UI messages (from background tasks)
            Some(ui_message) = ui_msg_rx.recv() => {
                let commands = update(&mut model, ui_message);
                for command in commands {
                    execute_command(command, &req_tx, &ui_msg_tx).await;
                }
            }
        }

        // Check if application should quit
        if model.should_quit {
            let _ = req_tx.send(AgentRequest::Quit);
            break;
        }
    }

    // Save session state on exit
    if model.provider != "none" {
        save_session_state(&model.provider, &model.model_name);
    }

    // Cleanup
    restore_terminal(&mut terminal)?;
    Ok(())
}

/// Run the agent loop with an existing agent. Handles provider switching.
async fn run_agent_loop(
    mut agent: Agent,
    req_rx: &mut mpsc::UnboundedReceiver<AgentRequest>,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    config: &Config,
) {
    while let Some(request) = req_rx.recv().await {
        match request {
            AgentRequest::ProcessMessage(input) => {
                if let Err(e) = agent.process_message(&input, Some(event_tx)).await {
                    let _ = event_tx.send(AgentEvent::Error(format!("{e:#}")));
                }
                let _ = event_tx.send(AgentEvent::Done);
            }
            AgentRequest::SwitchProvider {
                provider_name,
                model,
                api_key,
            } => {
                // Shutdown old LSP servers
                agent.shutdown_lsp().await;

                // Create new provider
                let api_key_opt = if api_key.is_empty() {
                    None
                } else {
                    Some(api_key.as_str())
                };
                match create_provider(&provider_name, api_key_opt) {
                    Ok(provider) => {
                        agent = Agent::new(provider).with_model(&model);
                        agent.set_max_concurrent_subagents(config.general.max_subagents);
                        agent.configure_lsp_servers(
                            &config.lsp_servers,
                            config.general.auto_install_lsp,
                        );
                        agent.configure_formatters(&config.formatters);
                        kodo_tools::register_builtin_tools(agent.tool_registry_mut());

                        save_session_state(&provider_name, &model);

                        let _ = event_tx.send(AgentEvent::ContextUpdate {
                            tokens: agent.context().current_conversation_tokens,
                            limit: agent.context().current_model_limit,
                            percent: 0.0,
                        });
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Error(format!(
                            "Failed to switch provider: {e:#}"
                        )));
                    }
                }
            }
            AgentRequest::Quit => {
                agent.shutdown_lsp().await;
                break;
            }
        }
    }
}

/// Run agent loop in deferred mode (no agent yet, wait for SwitchProvider first)
async fn run_agent_loop_deferred(
    req_rx: &mut mpsc::UnboundedReceiver<AgentRequest>,
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    config: &Config,
) {
    while let Some(request) = req_rx.recv().await {
        match request {
            AgentRequest::SwitchProvider {
                provider_name,
                model,
                api_key,
            } => {
                let api_key_opt = if api_key.is_empty() {
                    None
                } else {
                    Some(api_key.as_str())
                };
                match create_provider(&provider_name, api_key_opt) {
                    Ok(provider) => {
                        let agent = Agent::new(provider).with_model(&model);
                        save_session_state(&provider_name, &model);

                        let _ = event_tx.send(AgentEvent::ContextUpdate {
                            tokens: agent.context().current_conversation_tokens,
                            limit: agent.context().current_model_limit,
                            percent: 0.0,
                        });

                        // Now run the normal agent loop
                        run_agent_loop(agent, req_rx, event_tx, config).await;
                        return;
                    }
                    Err(e) => {
                        let _ = event_tx.send(AgentEvent::Error(format!(
                            "Failed to create provider: {e:#}"
                        )));
                    }
                }
            }
            AgentRequest::ProcessMessage(_) => {
                let _ = event_tx.send(AgentEvent::Error(
                    "No provider configured. Use Ctrl+K > Connect Provider to authenticate."
                        .to_string(),
                ));
                let _ = event_tx.send(AgentEvent::Done);
            }
            AgentRequest::Quit => {
                break;
            }
        }
    }
}

/// Execute Commands returned by update().
async fn execute_command(
    command: Command,
    req_tx: &mpsc::UnboundedSender<AgentRequest>,
    ui_msg_tx: &mpsc::UnboundedSender<Message>,
) {
    match command {
        Command::SendToAgent(message) => {
            let _ = req_tx.send(AgentRequest::ProcessMessage(message));
        }
        Command::StartOAuth { provider } => {
            let ui_tx = ui_msg_tx.clone();
            tokio::spawn(async move {
                run_oauth_flow(&provider, ui_tx).await;
            });
        }
        Command::StartOAuthCodePaste { provider } => {
            let ui_tx = ui_msg_tx.clone();
            tokio::spawn(async move {
                start_oauth_code_paste_flow(&provider, ui_tx).await;
            });
        }
        Command::ExchangeOAuthCode { provider, code } => {
            let ui_tx = ui_msg_tx.clone();
            tokio::spawn(async move {
                exchange_oauth_code(&provider, &code, ui_tx).await;
            });
        }
        Command::StoreApiKey { provider, api_key } => {
            let ui_tx = ui_msg_tx.clone();
            tokio::spawn(async move {
                store_api_key(&provider, &api_key, ui_tx).await;
            });
        }
        Command::SwitchProvider {
            provider,
            model,
            api_key,
        } => {
            let _ = req_tx.send(AgentRequest::SwitchProvider {
                provider_name: provider,
                model,
                api_key,
            });
        }
        Command::Quit => {
            let _ = req_tx.send(AgentRequest::Quit);
        }
        Command::None => {}
    }
}

/// Run the OAuth flow in a background task and send result back to UI
async fn run_oauth_flow(provider_name: &str, ui_tx: mpsc::UnboundedSender<Message>) {
    let config = match provider_name {
        "openai" => AuthConfig::openai(),
        other => {
            let _ = ui_tx.send(Message::OAuthError {
                provider: other.to_string(),
                error: format!("{} does not support OAuth. Use an API key instead.", other),
            });
            return;
        }
    };

    let oauth = OAuthProvider::new(config);
    match oauth.login().await {
        Ok(token) => {
            // Store token securely
            let storage = TokenStorage::new("kodo");
            if let Err(e) = storage.store(&token).await {
                tracing::error!("Failed to store OAuth token: {}", e);
            }

            // Set env var so provider can use it
            let env_key = match provider_name {
                "openai" => "OPENAI_API_KEY",
                _ => return,
            };
            unsafe {
                std::env::set_var(env_key, &token.access_token);
            }

            let _ = ui_tx.send(Message::OAuthComplete {
                provider: provider_name.to_string(),
                token: token.access_token,
            });
        }
        Err(e) => {
            let _ = ui_tx.send(Message::OAuthError {
                provider: provider_name.to_string(),
                error: format!("{e:#}"),
            });
        }
    }
}

/// Start OAuth code-paste flow: generate URL and send it back to TUI
async fn start_oauth_code_paste_flow(provider_name: &str, ui_tx: mpsc::UnboundedSender<Message>) {
    let config = match provider_name {
        "anthropic" => AuthConfig::anthropic(),
        "openai" => AuthConfig::openai(),
        other => {
            let _ = ui_tx.send(Message::OAuthError {
                provider: other.to_string(),
                error: format!("{} does not support OAuth code-paste flow.", other),
            });
            return;
        }
    };

    let oauth = OAuthProvider::new(config);
    match oauth.start_code_paste_flow() {
        Ok(pending) => {
            // Open browser automatically
            let _ = webbrowser::open(&pending.auth_url);

            // Clone URL before moving pending into storage
            let auth_url = pending.auth_url.clone();

            // Store the pending flow in a static for later exchange
            let mut guard = PENDING_CODE_PASTE.lock().await;
            *guard = Some(pending);

            // Send the URL back to TUI so user can see it
            let _ = ui_tx.send(Message::OAuthCodePasteReady {
                provider: provider_name.to_string(),
                auth_url,
            });
        }
        Err(e) => {
            let _ = ui_tx.send(Message::OAuthError {
                provider: provider_name.to_string(),
                error: format!("{e:#}"),
            });
        }
    }
}

/// Exchange a user-pasted OAuth code for a token
async fn exchange_oauth_code(
    provider_name: &str,
    code: &str,
    ui_tx: mpsc::UnboundedSender<Message>,
) {
    let pending = {
        let mut guard = PENDING_CODE_PASTE.lock().await;
        guard.take()
    };

    let Some(pending) = pending else {
        let _ = ui_tx.send(Message::OAuthError {
            provider: provider_name.to_string(),
            error: "No pending OAuth flow. Please start the flow again.".to_string(),
        });
        return;
    };

    match pending.exchange_code(code).await {
        Ok(token) => {
            // Store token securely
            let storage = TokenStorage::new("kodo");
            if let Err(e) = storage.store(&token).await {
                tracing::error!("Failed to store OAuth token: {}", e);
            }

            // Set env var
            let env_key = match provider_name {
                "anthropic" => "ANTHROPIC_API_KEY",
                "openai" => "OPENAI_API_KEY",
                _ => {
                    let _ = ui_tx.send(Message::OAuthComplete {
                        provider: provider_name.to_string(),
                        token: token.access_token,
                    });
                    return;
                }
            };
            unsafe {
                std::env::set_var(env_key, &token.access_token);
            }

            let _ = ui_tx.send(Message::OAuthComplete {
                provider: provider_name.to_string(),
                token: token.access_token,
            });
        }
        Err(e) => {
            let _ = ui_tx.send(Message::OAuthError {
                provider: provider_name.to_string(),
                error: format!("{e:#}"),
            });
        }
    }
}

/// Store an API key and send success back to UI
async fn store_api_key(provider_name: &str, api_key: &str, _ui_tx: mpsc::UnboundedSender<Message>) {
    // Store in keychain via TokenStorage
    let storage = TokenStorage::new("kodo");
    let token = AuthToken {
        provider: provider_name.to_string(),
        access_token: api_key.to_string(),
        refresh_token: None,
        expires_at: None,
    };
    if let Err(e) = storage.store(&token).await {
        tracing::error!("Failed to store API key: {}", e);
    }

    // Set env var so provider can use it immediately
    let env_key = match provider_name {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        _ => return,
    };
    unsafe {
        std::env::set_var(env_key, api_key);
    }

    // The modal will transition to AuthSuccess after StoreApiKey completes.
    // No additional message needed since the update() already transitions state.
}

/// Map AgentEvent to application Message
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
        AgentEvent::ContextUpdate {
            tokens,
            limit,
            percent,
        } => Message::ContextUpdate {
            tokens,
            limit,
            percent,
        },
        AgentEvent::Done => Message::AgentDone,
    }
}

/// Try to setup OAuth tokens as environment variables if not already set
async fn setup_oauth_tokens() -> Result<()> {
    let storage = TokenStorage::new("kodo");

    // Try Anthropic
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        if let Ok(Some(token)) = storage.get("anthropic").await {
            unsafe {
                std::env::set_var("ANTHROPIC_API_KEY", token.access_token);
            }
            tracing::debug!("Loaded Anthropic token from storage");
        }
    }

    // Try OpenAI
    if std::env::var("OPENAI_API_KEY").is_err() {
        if let Ok(Some(token)) = storage.get("openai").await {
            unsafe {
                std::env::set_var("OPENAI_API_KEY", token.access_token);
            }
            tracing::debug!("Loaded OpenAI token from storage");
        }
    }

    Ok(())
}

/// Handle --auth CLI flag (non-TUI OAuth flow)
async fn handle_auth(provider_name: &str) -> Result<()> {
    let config = match provider_name {
        "anthropic" => AuthConfig::anthropic(),
        "openai" => AuthConfig::openai(),
        _ => bail!(
            "Unsupported auth provider: {}. Available: anthropic, openai",
            provider_name
        ),
    };

    println!("Starting authentication for {}...", provider_name);

    let oauth = OAuthProvider::new(config);
    let token = oauth.login().await?;

    let storage = TokenStorage::new("kodo");
    storage.store(&token).await?;

    println!("Authentication successful! Token has been securely stored.");
    println!("You can now run kodo and it will use the stored credentials.");

    Ok(())
}
