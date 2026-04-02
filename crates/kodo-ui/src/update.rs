use crate::command::Command;
use crate::message::{Message, ThemeChoice};
use crate::model::{
    AuthMethod, ChatMessage, ChatRole, Model, ModelModalState, ModelOption, ProviderModalState,
};
use crate::theme::Theme;

/// The core update function following the Elm Architecture.
///
/// This is a PURE function that takes the current model and a message,
/// then returns a list of commands to execute. It performs NO side effects:
/// - No I/O operations
/// - No network calls  
/// - No file system access
/// - No printing to stdout/stderr
///
/// All side effects are represented as Commands that the runtime executes.
/// This makes the function:
/// - Completely testable
/// - Deterministic
/// - Reusable across different runtimes (TUI, GUI, tests)
///
/// Every possible state change in the application flows through this function.
pub fn update(model: &mut Model, message: Message) -> Vec<Command> {
    match message {
        // -- Input events --
        Message::KeyInput(ch) => {
            if model.palette_open {
                // Add character to palette search query
                model.palette_query.push(ch);
                // Reset selection to top when query changes
                model.palette_selected = 0;
            } else {
                // Add character to user input
                model.input.insert(model.cursor_pos, ch);
                model.cursor_pos += 1;
            }
            vec![Command::None]
        }

        Message::Backspace => {
            if model.palette_open {
                handle_palette_backspace(model)
            } else {
                handle_input_backspace(model)
            }
        }

        Message::Delete => {
            if !model.palette_open && model.cursor_pos < model.input.len() {
                model.input.remove(model.cursor_pos);
            }
            vec![Command::None]
        }

        Message::CursorLeft => {
            if !model.palette_open && model.cursor_pos > 0 {
                model.cursor_pos -= 1;
            }
            vec![Command::None]
        }

        Message::CursorRight => {
            if !model.palette_open && model.cursor_pos < model.input.len() {
                model.cursor_pos += 1;
            }
            vec![Command::None]
        }

        Message::CursorHome => {
            if !model.palette_open {
                model.cursor_pos = 0;
            }
            vec![Command::None]
        }

        Message::CursorEnd => {
            if !model.palette_open {
                model.cursor_pos = model.input.len();
            }
            vec![Command::None]
        }

        Message::Submit => {
            if model.palette_open {
                handle_palette_select(model)
            } else {
                handle_input_submit(model)
            }
        }

        Message::ScrollUp(lines) => {
            model.scroll_offset = model.scroll_offset.saturating_add(lines);
            vec![Command::None]
        }

        Message::ScrollDown(lines) => {
            model.scroll_offset = model.scroll_offset.saturating_sub(lines);
            vec![Command::None]
        }

        // -- Mode --
        Message::ToggleMode => {
            // Toggle between Plan and Build mode
            let new_mode = if model.mode == "Plan" {
                "Build".to_string()
            } else {
                "Plan".to_string()
            };
            model.mode = new_mode.clone();

            // Log mode change to debug panel if debug mode is enabled
            if model.debug_mode {
                model
                    .debug_logs
                    .push(format!("🔄 Mode toggled to {}", new_mode));
            }

            vec![Command::None]
        }

        // -- Command palette --
        Message::OpenPalette => {
            model.palette_open = true;
            model.palette_query.clear();
            model.palette_selected = 0;
            vec![Command::None]
        }

        Message::ClosePalette => {
            model.palette_open = false;
            model.palette_query.clear();
            model.palette_selected = 0;
            vec![Command::None]
        }

        Message::PaletteInput(ch) => {
            model.palette_query.push(ch);
            model.palette_selected = 0;
            vec![Command::None]
        }

        Message::PaletteBackspace => handle_palette_backspace(model),

        Message::PaletteUp => {
            if model.palette_selected > 0 {
                model.palette_selected -= 1;
            }
            vec![Command::None]
        }

        Message::PaletteDown => {
            // Increment selection (view layer will clamp to available items)
            model.palette_selected += 1;
            vec![Command::None]
        }

        Message::PaletteSelect => handle_palette_select(model),

        // -- Theme --
        Message::SetTheme(choice) => {
            model.theme = match choice {
                ThemeChoice::Dark => {
                    model.debug_logs.push("Theme changed to Dark".to_string());
                    Theme::dark()
                }
                ThemeChoice::Light => {
                    model.debug_logs.push("Theme changed to Light".to_string());
                    Theme::light()
                }
            };
            model.update_syntax_theme();
            vec![Command::None]
        }

        // -- Debug --
        Message::ToggleDebugPanel => {
            if model.debug_mode {
                model.debug_panel_open = !model.debug_panel_open;
                let status = if model.debug_panel_open {
                    "opened"
                } else {
                    "closed"
                };
                model.debug_logs.push(format!("🔍 Debug panel {}", status));
            }
            vec![Command::None]
        }

        // -- Agent lifecycle --
        Message::AgentTextDelta(text) => {
            model.streaming_text.push_str(&text);
            model.is_streaming = true;
            // Auto-scroll to bottom when streaming
            model.scroll_offset = 0;
            vec![Command::None]
        }

        Message::AgentTextDone => {
            // Move streaming text to messages
            if !model.streaming_text.is_empty() {
                let text = std::mem::take(&mut model.streaming_text);
                model.messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: text,
                });
            }
            model.is_streaming = false;
            model.scroll_offset = 0;
            vec![Command::None]
        }

        Message::AgentToolStart { name } => {
            if model.debug_mode {
                model.debug_logs.push(format!("🔧 Tool started: {}", name));
            }
            vec![Command::None]
        }

        Message::AgentToolDone { name, success } => {
            if model.debug_mode {
                let status = if success { "✅" } else { "❌" };
                model
                    .debug_logs
                    .push(format!("{} Tool finished: {}", status, name));
            }
            vec![Command::None]
        }

        Message::AgentToolDenied { name, reason } => {
            if model.debug_mode {
                model
                    .debug_logs
                    .push(format!("🚫 Tool denied: {} - {}", name, reason));
            }
            vec![Command::None]
        }

        Message::AgentToolCancelled { name } => {
            if model.debug_mode {
                model
                    .debug_logs
                    .push(format!("🛑 Tool cancelled: {}", name));
            }
            vec![Command::None]
        }

        Message::AgentFormatted { message } => {
            if model.debug_mode {
                model.debug_logs.push(format!("Formatted: {}", message));
            }
            vec![Command::None]
        }

        Message::AgentDiagnostics { summary, count } => {
            if model.debug_mode {
                model
                    .debug_logs
                    .push(format!("LSP: {} diagnostic(s)", count));
            }
            // Show diagnostics in chat so user sees errors/warnings.
            if count > 0 {
                model.messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: summary,
                });
                model.scroll_offset = 0;
            }
            vec![Command::None]
        }

        Message::AgentError(error) => {
            if model.debug_mode {
                model.debug_logs.push(format!("💥 Error: {}", error));
            }
            // Also show error in chat
            model.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: format!("Error: {}", error),
            });
            model.is_streaming = false;
            model.streaming_text.clear();
            vec![Command::None]
        }

        Message::ContextUpdate {
            tokens,
            limit,
            percent: _,
        } => {
            model.context_tokens = tokens;
            model.context_limit = limit;
            vec![Command::None]
        }

        Message::AgentDone => {
            model.is_streaming = false;
            vec![Command::None]
        }

        // -- Provider connect modal --
        Message::OpenProviderModal => {
            model.provider_modal = ProviderModalState::SelectProvider;
            model.provider_modal_selected = 0;
            vec![Command::None]
        }

        Message::CloseProviderModal => {
            // Only allow closing if we have a provider (not in needs_provider state)
            if model.needs_provider {
                // Can't close - must authenticate first
                vec![Command::None]
            } else {
                model.provider_modal = ProviderModalState::Closed;
                model.api_key_input.clear();
                vec![Command::None]
            }
        }

        Message::ProviderModalUp => {
            match &model.provider_modal {
                ProviderModalState::SelectProvider => {
                    if model.provider_modal_selected > 0 {
                        model.provider_modal_selected -= 1;
                    }
                }
                ProviderModalState::SelectAuthMethod { .. } => {
                    if model.auth_method_selected > 0 {
                        model.auth_method_selected -= 1;
                    }
                }
                _ => {}
            }
            vec![Command::None]
        }

        Message::ProviderModalDown => {
            match &model.provider_modal {
                ProviderModalState::SelectProvider => {
                    model.provider_modal_selected += 1;
                    // Clamp in view layer
                }
                ProviderModalState::SelectAuthMethod { .. } => {
                    model.auth_method_selected += 1;
                }
                _ => {}
            }
            vec![Command::None]
        }

        Message::ProviderModalSelect => handle_provider_modal_select(model),

        Message::ProviderModalApiKeyInput(ch) => {
            // Works for both API key input and OAuth code input
            if matches!(
                model.provider_modal,
                ProviderModalState::EnterApiKey { .. } | ProviderModalState::EnterOAuthCode { .. }
            ) {
                model.api_key_input.push(ch);
            }
            vec![Command::None]
        }

        Message::ProviderModalApiKeyBackspace => {
            if matches!(
                model.provider_modal,
                ProviderModalState::EnterApiKey { .. } | ProviderModalState::EnterOAuthCode { .. }
            ) {
                model.api_key_input.pop();
            }
            vec![Command::None]
        }

        Message::ProviderModalApiKeySubmit => {
            if let ProviderModalState::EnterApiKey { ref provider } = model.provider_modal {
                if !model.api_key_input.is_empty() {
                    let provider = provider.clone();
                    let api_key = model.api_key_input.clone();
                    model.api_key_input.clear();
                    model.provider_modal = ProviderModalState::AuthSuccess {
                        provider: provider.clone(),
                    };
                    return vec![Command::StoreApiKey { provider, api_key }];
                }
            }
            vec![Command::None]
        }

        Message::ProviderModalOAuthCodeSubmit => {
            if let ProviderModalState::EnterOAuthCode { ref provider, .. } = model.provider_modal {
                if !model.api_key_input.is_empty() {
                    let provider = provider.clone();
                    let code = model.api_key_input.clone();
                    model.api_key_input.clear();
                    model.provider_modal = ProviderModalState::OAuthInProgress {
                        provider: provider.clone(),
                    };
                    return vec![Command::ExchangeOAuthCode { provider, code }];
                }
            }
            vec![Command::None]
        }

        Message::OAuthCodePasteReady { provider, auth_url } => {
            model.provider_modal = ProviderModalState::EnterOAuthCode { provider, auth_url };
            model.api_key_input.clear();
            vec![Command::None]
        }

        Message::OAuthComplete { provider, token } => {
            // Store the token via command and transition to success state
            model.provider_modal = ProviderModalState::AuthSuccess {
                provider: provider.clone(),
            };
            vec![Command::StoreApiKey {
                provider,
                api_key: token,
            }]
        }

        Message::OAuthError {
            provider,
            ref error,
        } => {
            if model.debug_mode {
                model
                    .debug_logs
                    .push(format!("AUTH ERROR [{}]: {}", provider, error));
            }
            let error = error.clone();
            model.provider_modal = ProviderModalState::AuthError { provider, error };
            vec![Command::None]
        }

        Message::ProviderModalBack => {
            match &model.provider_modal {
                ProviderModalState::SelectAuthMethod { .. }
                | ProviderModalState::EnterApiKey { .. }
                | ProviderModalState::EnterOAuthCode { .. }
                | ProviderModalState::AuthError { .. } => {
                    model.provider_modal = ProviderModalState::SelectProvider;
                    model.api_key_input.clear();
                    model.provider_modal_selected = 0;
                }
                ProviderModalState::AuthSuccess { .. } => {
                    // After success, go to model selection
                    model.provider_modal = ProviderModalState::Closed;
                }
                _ => {}
            }
            vec![Command::None]
        }

        // -- Model selection modal --
        Message::OpenModelModal => {
            model.model_modal = ModelModalState::SelectModel;
            model.model_modal_selected = 0;
            // Start with static fallback, then fetch dynamic list
            model.model_options = models_for_provider(&model.provider);
            let provider = model.provider.clone();
            vec![Command::FetchModels { provider }]
        }

        Message::CloseModelModal => {
            model.model_modal = ModelModalState::Closed;
            vec![Command::None]
        }

        Message::ModelModalUp => {
            if model.model_modal_selected > 0 {
                model.model_modal_selected -= 1;
            }
            vec![Command::None]
        }

        Message::ModelModalDown => {
            model.model_modal_selected += 1;
            vec![Command::None]
        }

        Message::ModelModalSelect => handle_model_modal_select(model),

        Message::ModelsFetched { provider, models } => {
            if model.debug_mode {
                let ids: Vec<&str> = models.iter().map(|(id, _)| id.as_str()).collect();
                model.debug_logs.push(format!(
                    "MODELS [{}]: {} models fetched: {}",
                    provider,
                    models.len(),
                    ids.join(", ")
                ));
            }
            // Only update if the modal is still showing the same provider
            if model.model_modal != ModelModalState::Closed {
                model.model_options = models
                    .into_iter()
                    .map(|(id, display_name)| ModelOption {
                        id,
                        display_name,
                        provider: provider.clone(),
                    })
                    .collect();
                // Reset selection if it's out of bounds
                if model.model_modal_selected >= model.model_options.len() {
                    model.model_modal_selected = 0;
                }
            }
            vec![Command::None]
        }

        // -- Provider switching --
        Message::SwitchProvider {
            provider,
            model: model_name,
            api_key,
        } => {
            model.provider = provider.clone();
            model.model_name = model_name.clone();
            model.needs_provider = false;
            model.provider_modal = ProviderModalState::Closed;
            model.model_modal = ModelModalState::Closed;
            vec![Command::SwitchProvider {
                provider,
                model: model_name,
                api_key,
            }]
        }

        // -- System --
        Message::Tick => {
            // Check for leader sequence timeout
            if model.leader_state.check_timeout() {
                // Leader sequence timed out - no need to update UI, just cancel
            }
            // Periodic update - could be used for animations, etc.
            vec![Command::None]
        }

        Message::Resize(width, height) => {
            // Terminal resize - no model changes needed, just re-render
            // Could log this if needed
            if model.debug_mode {
                model
                    .debug_logs
                    .push(format!("📐 Resize: {}x{}", width, height));
            }
            vec![Command::None]
        }

        Message::Quit => {
            model.should_quit = true;
            vec![Command::Quit]
        }

        // -- Keybinds --
        Message::StartLeaderSequence => {
            model.leader_state.start_sequence();
            vec![]
        }

        Message::ExecuteLeaderAction(key) => {
            model.leader_state.cancel_sequence();
            if let Some(action) = model.keybinds.get_leader_action(key) {
                // Convert action to a message and recursively handle it
                let msg = match action {
                    crate::keybinds::KeyAction::Message(msg) => msg.clone(),
                    crate::keybinds::KeyAction::OpenPalette => Message::OpenPalette,
                    crate::keybinds::KeyAction::ToggleMode => Message::ToggleMode,
                    crate::keybinds::KeyAction::ToggleDebug => Message::ToggleDebugPanel,
                    crate::keybinds::KeyAction::ToggleTheme => {
                        let new_theme = if model.theme.is_dark() {
                            model
                                .debug_logs
                                .push("Theme toggle: Dark -> Light".to_string());
                            ThemeChoice::Light
                        } else {
                            model
                                .debug_logs
                                .push("Theme toggle: Light -> Dark".to_string());
                            ThemeChoice::Dark
                        };
                        Message::SetTheme(new_theme)
                    }
                    crate::keybinds::KeyAction::Quit => Message::Quit,
                    crate::keybinds::KeyAction::None => return vec![],
                };
                update(model, msg)
            } else {
                vec![]
            }
        }

        Message::CancelLeaderSequence => {
            model.leader_state.cancel_sequence();
            vec![]
        }
    }
}

/// Handle backspace in the input field
fn handle_input_backspace(model: &mut Model) -> Vec<Command> {
    if model.cursor_pos > 0 {
        model.cursor_pos -= 1;
        model.input.remove(model.cursor_pos);
    }
    vec![Command::None]
}

/// Handle backspace in the command palette
fn handle_palette_backspace(model: &mut Model) -> Vec<Command> {
    if !model.palette_query.is_empty() {
        model.palette_query.pop();
        model.palette_selected = 0;
    }
    vec![Command::None]
}

/// Handle submitting user input
fn handle_input_submit(model: &mut Model) -> Vec<Command> {
    if model.input.trim().is_empty() {
        return vec![Command::None];
    }

    // If no provider is connected, open the provider modal instead
    if model.needs_provider {
        model.provider_modal = ProviderModalState::SelectProvider;
        model.provider_modal_selected = 0;
        return vec![Command::None];
    }

    // Add user message to chat
    let user_input = std::mem::take(&mut model.input);
    model.messages.push(ChatMessage {
        role: ChatRole::User,
        content: user_input.clone(),
    });

    // Reset input state
    model.cursor_pos = 0;
    model.scroll_offset = 0;

    // Send to agent for processing
    vec![Command::send_to_agent(user_input)]
}

/// Dispatch the appropriate action for an auth method
fn dispatch_auth_method(model: &mut Model, method: &AuthMethod, provider: String) -> Vec<Command> {
    match method {
        AuthMethod::OAuth => {
            model.provider_modal = ProviderModalState::OAuthInProgress {
                provider: provider.clone(),
            };
            vec![Command::StartOAuth { provider }]
        }
        AuthMethod::OAuthCodePaste => {
            // Start code-paste flow: runtime will generate URL and send it back
            vec![Command::StartOAuthCodePaste {
                provider: provider.clone(),
            }]
        }
        AuthMethod::ApiKey => {
            model.provider_modal = ProviderModalState::EnterApiKey { provider };
            vec![Command::None]
        }
    }
}

/// Handle selecting an item in the provider connect modal
fn handle_provider_modal_select(model: &mut Model) -> Vec<Command> {
    match model.provider_modal.clone() {
        ProviderModalState::SelectProvider => {
            let idx = model
                .provider_modal_selected
                .min(model.provider_options.len().saturating_sub(1));
            // Clone needed data before mutably borrowing model
            let option_data = model
                .provider_options
                .get(idx)
                .map(|o| (o.id.clone(), o.auth_methods.clone(), o.is_authenticated));
            if let Some((provider, auth_methods, is_authenticated)) = option_data {
                if auth_methods.is_empty() {
                    // No auth needed (e.g. Ollama) - go straight to model selection
                    model.provider_modal = ProviderModalState::Closed;
                    model.model_options = models_for_provider(&provider);
                    model.model_modal = ModelModalState::SelectModel;
                    model.model_modal_selected = 0;
                    return vec![Command::FetchModels {
                        provider: provider.clone(),
                    }];
                }

                if is_authenticated {
                    // Already authenticated - go straight to model selection
                    model.provider_modal = ProviderModalState::Closed;
                    model.model_options = models_for_provider(&provider);
                    model.model_modal = ModelModalState::SelectModel;
                    model.model_modal_selected = 0;
                    return vec![Command::FetchModels {
                        provider: provider.clone(),
                    }];
                }

                if auth_methods.len() == 1 {
                    // Only one auth method, use it directly
                    return dispatch_auth_method(model, &auth_methods[0], provider);
                } else {
                    // Multiple auth methods, show selection
                    model.provider_modal = ProviderModalState::SelectAuthMethod { provider };
                    model.auth_method_selected = 0;
                }
            }
            vec![Command::None]
        }
        ProviderModalState::SelectAuthMethod { ref provider } => {
            let provider = provider.clone();
            let idx = model.auth_method_selected;
            // Clone method before mutable borrow
            let method = model
                .provider_options
                .iter()
                .find(|o| o.id == provider)
                .and_then(|o| o.auth_methods.get(idx).cloned());
            if let Some(method) = method {
                return dispatch_auth_method(model, &method, provider);
            }
            vec![Command::None]
        }
        ProviderModalState::AuthSuccess { ref provider } => {
            // After auth success, open model selection
            let provider = provider.clone();
            model.provider_modal = ProviderModalState::Closed;
            model.model_options = models_for_provider(&provider);
            model.model_modal = ModelModalState::SelectModel;
            model.model_modal_selected = 0;
            vec![Command::FetchModels {
                provider: provider.clone(),
            }]
        }
        ProviderModalState::AuthError { .. } => {
            // Go back to provider selection
            model.provider_modal = ProviderModalState::SelectProvider;
            model.provider_modal_selected = 0;
            vec![Command::None]
        }
        _ => vec![Command::None],
    }
}

/// Handle selecting a model in the model selection modal
fn handle_model_modal_select(model: &mut Model) -> Vec<Command> {
    let idx = model
        .model_modal_selected
        .min(model.model_options.len().saturating_sub(1));
    if let Some(selected) = model.model_options.get(idx) {
        let provider = selected.provider.clone();
        let model_id = selected.id.clone();
        model.provider = provider.clone();
        model.model_name = model_id.clone();
        model.model_modal = ModelModalState::Closed;
        model.needs_provider = false;
        // The runtime will handle the actual provider switch
        return vec![Command::SwitchProvider {
            provider,
            model: model_id,
            api_key: String::new(), // Already stored
        }];
    }
    vec![Command::None]
}

/// Get the available models for a provider
fn models_for_provider(provider: &str) -> Vec<ModelOption> {
    match provider {
        "anthropic" => vec![
            ModelOption {
                id: "claude-sonnet-4-20250514".to_string(),
                display_name: "Claude Sonnet 4".to_string(),
                provider: "anthropic".to_string(),
            },
            ModelOption {
                id: "claude-haiku-4-20250414".to_string(),
                display_name: "Claude Haiku 4".to_string(),
                provider: "anthropic".to_string(),
            },
            ModelOption {
                id: "claude-opus-4-20250514".to_string(),
                display_name: "Claude Opus 4".to_string(),
                provider: "anthropic".to_string(),
            },
        ],
        "openai" => vec![
            ModelOption {
                id: "o3".to_string(),
                display_name: "o3".to_string(),
                provider: "openai".to_string(),
            },
            ModelOption {
                id: "o4-mini".to_string(),
                display_name: "o4-mini".to_string(),
                provider: "openai".to_string(),
            },
            ModelOption {
                id: "gpt-4.1".to_string(),
                display_name: "GPT-4.1".to_string(),
                provider: "openai".to_string(),
            },
            ModelOption {
                id: "gpt-4.1-mini".to_string(),
                display_name: "GPT-4.1 Mini".to_string(),
                provider: "openai".to_string(),
            },
            ModelOption {
                id: "gpt-4.1-nano".to_string(),
                display_name: "GPT-4.1 Nano".to_string(),
                provider: "openai".to_string(),
            },
            ModelOption {
                id: "gpt-4o".to_string(),
                display_name: "GPT-4o".to_string(),
                provider: "openai".to_string(),
            },
            ModelOption {
                id: "gpt-4o-mini".to_string(),
                display_name: "GPT-4o Mini".to_string(),
                provider: "openai".to_string(),
            },
            ModelOption {
                id: "o3-mini".to_string(),
                display_name: "o3-mini".to_string(),
                provider: "openai".to_string(),
            },
        ],
        "gemini" => vec![
            ModelOption {
                id: "gemini-2.5-flash".to_string(),
                display_name: "Gemini 2.5 Flash".to_string(),
                provider: "gemini".to_string(),
            },
            ModelOption {
                id: "gemini-2.5-pro".to_string(),
                display_name: "Gemini 2.5 Pro".to_string(),
                provider: "gemini".to_string(),
            },
        ],
        "ollama" => vec![ModelOption {
            id: "llama3.1".to_string(),
            display_name: "Llama 3.1".to_string(),
            provider: "ollama".to_string(),
        }],
        _ => vec![],
    }
}

/// Handle selecting a command palette item
fn handle_palette_select(model: &mut Model) -> Vec<Command> {
    model.palette_open = false;

    // Get filtered commands and find selected one
    let commands = palette_command_ids();
    let q = model.palette_query.to_lowercase();
    let filtered: Vec<&str> = if q.is_empty() {
        commands.iter().copied().collect()
    } else {
        commands
            .iter()
            .filter(|id| id.to_lowercase().contains(&q))
            .copied()
            .collect()
    };

    let selected_idx = model.palette_selected.min(filtered.len().saturating_sub(1));
    let selected_cmd = filtered.get(selected_idx).copied().unwrap_or("");

    match selected_cmd {
        "quit" => {
            model.should_quit = true;
            vec![Command::Quit]
        }
        "clear" => {
            model.messages.clear();
            model.streaming_text.clear();
            model.scroll_offset = 0;
            vec![Command::None]
        }
        "dark" => {
            model.theme = Theme::dark();
            model.update_syntax_theme();
            vec![Command::None]
        }
        "light" => {
            model.theme = Theme::light();
            model.update_syntax_theme();
            vec![Command::None]
        }
        "connect" => {
            model.provider_modal = ProviderModalState::SelectProvider;
            model.provider_modal_selected = 0;
            vec![Command::None]
        }
        "switch_model" => {
            let provider = model.provider.clone();
            model.model_options = models_for_provider(&provider);
            model.model_modal = ModelModalState::SelectModel;
            model.model_modal_selected = 0;
            vec![Command::FetchModels { provider }]
        }
        _ => vec![Command::None],
    }
}

/// Palette command IDs (used for selection logic)
fn palette_command_ids() -> Vec<&'static str> {
    vec!["connect", "switch_model", "clear", "dark", "light", "quit"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_input() {
        let mut model = Model::new(false);
        let commands = update(&mut model, Message::KeyInput('h'));

        assert_eq!(model.input, "h");
        assert_eq!(model.cursor_pos, 1);
        assert!(commands.iter().all(|cmd| cmd.is_none()));
    }

    #[test]
    fn test_backspace() {
        let mut model = Model::new(false);
        model.input = "hello".to_string();
        model.cursor_pos = 5;

        let commands = update(&mut model, Message::Backspace);

        assert_eq!(model.input, "hell");
        assert_eq!(model.cursor_pos, 4);
        assert!(commands.iter().all(|cmd| cmd.is_none()));
    }

    #[test]
    fn test_submit_empty_input() {
        let mut model = Model::new(false);
        let commands = update(&mut model, Message::Submit);

        assert!(commands.iter().all(|cmd| cmd.is_none()));
        assert_eq!(model.messages.len(), 0);
    }

    #[test]
    fn test_submit_with_input() {
        let mut model = Model::new(false);
        model.input = "Hello agent".to_string();

        let commands = update(&mut model, Message::Submit);

        assert_eq!(model.messages.len(), 1);
        assert_eq!(model.messages[0].content, "Hello agent");
        assert!(model.input.is_empty());
        assert_eq!(model.cursor_pos, 0);

        // Should generate SendToAgent command
        assert_eq!(commands.len(), 1);
        assert!(matches!(commands[0], Command::SendToAgent(_)));
    }

    #[test]
    fn test_toggle_mode() {
        let mut model = Model::new(false);
        assert_eq!(model.mode, "Build");

        update(&mut model, Message::ToggleMode);
        assert_eq!(model.mode, "Plan");

        update(&mut model, Message::ToggleMode);
        assert_eq!(model.mode, "Build");
    }

    #[test]
    fn test_streaming() {
        let mut model = Model::new(false);

        update(&mut model, Message::AgentTextDelta("Hello".to_string()));
        assert!(model.is_streaming);
        assert_eq!(model.streaming_text, "Hello");

        update(&mut model, Message::AgentTextDelta(" world".to_string()));
        assert_eq!(model.streaming_text, "Hello world");

        update(&mut model, Message::AgentTextDone);
        assert!(!model.is_streaming);
        assert!(model.streaming_text.is_empty());
        assert_eq!(model.messages.len(), 1);
        assert_eq!(model.messages[0].content, "Hello world");
    }
}
