use crate::command::Command;
use crate::message::{Message, ThemeChoice};
use crate::model::{ChatMessage, ChatRole, Model};
use crate::skills;
use crate::slash::{self, CommandSource};
use crate::theme::Theme;

/// The core update function following the Elm Architecture.
pub fn update(model: &mut Model, message: Message) -> Vec<Command> {
    match message {
        Message::KeyInput(ch) => {
            model.input.insert(model.cursor_pos, ch);
            model.cursor_pos += 1;
            sync_slash_state(model);
            vec![Command::None]
        }

        Message::Backspace => handle_input_backspace(model),

        Message::Delete => {
            if model.cursor_pos < model.input.len() {
                model.input.remove(model.cursor_pos);
                sync_slash_state(model);
            }
            vec![Command::None]
        }

        Message::CursorLeft => {
            if model.cursor_pos > 0 {
                model.cursor_pos -= 1;
            }
            vec![Command::None]
        }

        Message::CursorRight => {
            if model.cursor_pos < model.input.len() {
                model.cursor_pos += 1;
            }
            vec![Command::None]
        }

        Message::CursorHome => {
            model.cursor_pos = 0;
            vec![Command::None]
        }

        Message::CursorEnd => {
            model.cursor_pos = model.input.len();
            vec![Command::None]
        }

        Message::Submit => {
            if model.slash_is_active() {
                handle_slash_execute(model)
            } else {
                handle_input_submit(model)
            }
        }

        Message::SlashNav(delta) => handle_slash_nav(model, delta),
        Message::SlashExecute => handle_slash_execute(model),
        Message::SlashCancel => {
            model.slash_state = None;
            vec![Command::None]
        }

        Message::ScrollUp(lines) => {
            model.scroll_offset = model.scroll_offset.saturating_add(lines);
            vec![Command::None]
        }

        Message::ScrollDown(lines) => {
            model.scroll_offset = model.scroll_offset.saturating_sub(lines);
            vec![Command::None]
        }

        Message::ToggleMode => {
            let new_mode = if model.mode == "Plan" {
                "Build".to_string()
            } else {
                "Plan".to_string()
            };
            model.mode = new_mode.clone();
            push_debug_message(model, format!("Mode toggled to {new_mode}"));

            vec![Command::None]
        }

        Message::SetTheme(choice) => {
            model.theme = match choice {
                ThemeChoice::Dark => Theme::dark(),
                ThemeChoice::Light => Theme::light(),
            };
            model.update_syntax_theme();
            push_debug_message(model, format!("Theme changed to {:?}", choice));
            vec![Command::None]
        }

        Message::AgentTextDelta(text) => {
            model.streaming_text.push_str(&text);
            model.is_streaming = true;
            model.scroll_offset = 0;
            vec![Command::None]
        }

        Message::AgentTextDone => {
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
            push_debug_message(model, format!("Tool started: {name}"));
            vec![Command::None]
        }

        Message::AgentToolDone { name, success } => {
            let status = if success { "ok" } else { "failed" };
            push_debug_message(model, format!("Tool {status}: {name}"));
            vec![Command::None]
        }

        Message::AgentToolDenied { name, reason } => {
            push_debug_message(model, format!("Tool denied: {name} - {reason}"));
            vec![Command::None]
        }

        Message::AgentToolCancelled { name } => {
            push_debug_message(model, format!("Tool cancelled: {name}"));
            vec![Command::None]
        }

        Message::AgentFormatted { message } => {
            push_debug_message(model, format!("Formatted: {message}"));
            vec![Command::None]
        }

        Message::AgentDiagnostics { summary, count } => {
            push_debug_message(model, format!("LSP: {count} diagnostic(s)"));
            if count > 0 {
                push_system_message(model, summary);
            }
            vec![Command::None]
        }

        Message::AgentError(error) => {
            push_debug_message(model, format!("Error: {error}"));
            model.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: format!("Error: {error}"),
            });
            model.is_streaming = false;
            model.streaming_text.clear();
            vec![Command::None]
        }

        Message::AgentDone => {
            model.is_streaming = false;
            vec![Command::None]
        }

        Message::Notice(message) => {
            push_system_message(model, message);
            vec![Command::None]
        }

        Message::ModelsListed {
            current_model,
            models,
        } => {
            let mut content = format!(
                "Current model: `{}` / `{}`.\nAvailable coding models:",
                model.provider, current_model
            );
            if models.is_empty() {
                content.push_str("\n- none");
            } else {
                for model_id in models {
                    content.push_str(&format!("\n- `{model_id}`"));
                }
            }
            content.push_str("\nUse `/model <id>` to switch.");
            push_system_message(model, content);
            vec![Command::None]
        }

        Message::ProvidersListed(providers) => {
            if providers.is_empty() {
                push_system_message(model, "No connected providers found.".to_string());
            } else {
                let providers = providers
                    .into_iter()
                    .map(|provider| format!("- {provider}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                push_system_message(model, format!("Connected providers:\n{providers}"));
            }
            vec![Command::None]
        }

        Message::ModelChanged(model_id) => {
            model.model_name = model_id.clone();
            push_system_message(model, format!("Switched model to `{model_id}`."));
            vec![Command::None]
        }

        Message::LoginComplete { account_id, name } => {
            let mut message = format!("Logged in `{account_id}`.");
            if let Some(name) = name {
                message.push_str(&format!(
                    " Account labels are not persisted yet, so `{name}` is only acknowledged for now."
                ));
            }
            push_system_message(model, message);
            vec![Command::None]
        }

        Message::LogoutComplete(account_id) => {
            push_system_message(model, format!("Logged out `{account_id}`."));
            vec![Command::None]
        }

        Message::Tick => {
            let _ = model.leader_state.check_timeout();
            vec![Command::None]
        }

        Message::Resize(width, height) => {
            push_debug_message(model, format!("Resize: {width}x{height}"));
            vec![Command::None]
        }

        Message::Quit => {
            model.should_quit = true;
            vec![Command::Quit]
        }

        Message::StartLeaderSequence => {
            model.leader_state.start_sequence();
            vec![]
        }

        Message::ExecuteLeaderAction(key) => {
            model.leader_state.cancel_sequence();
            if let Some(action) = model.keybinds.get_leader_action(key) {
                let msg = match action {
                    crate::keybinds::KeyAction::Message(msg) => msg.clone(),
                    crate::keybinds::KeyAction::ToggleMode => Message::ToggleMode,
                    crate::keybinds::KeyAction::ToggleTheme => {
                        let new_theme = if model.theme.is_dark() {
                            ThemeChoice::Light
                        } else {
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

fn handle_input_backspace(model: &mut Model) -> Vec<Command> {
    if model.cursor_pos > 0 {
        model.cursor_pos -= 1;
        model.input.remove(model.cursor_pos);
    }
    sync_slash_state(model);
    vec![Command::None]
}

fn handle_input_submit(model: &mut Model) -> Vec<Command> {
    if model.input.trim().is_empty() && model.pending_skill_injection.is_none() {
        return vec![Command::None];
    }

    let user_input = std::mem::take(&mut model.input);
    if !user_input.trim().is_empty() {
        model.messages.push(ChatMessage {
            role: ChatRole::User,
            content: user_input.clone(),
        });
    }

    model.cursor_pos = 0;
    model.scroll_offset = 0;
    model.slash_state = None;

    let outbound_message = if let Some(injection) = model.pending_skill_injection.take() {
        if user_input.trim().is_empty() {
            injection
        } else {
            format!("{injection}\n\n{user_input}")
        }
    } else {
        user_input
    };

    vec![Command::send_to_agent(outbound_message)]
}

fn handle_slash_nav(model: &mut Model, delta: i32) -> Vec<Command> {
    let Some(state) = model.slash_state.as_mut() else {
        return vec![Command::None];
    };

    if state.completions.is_empty() {
        return vec![Command::None];
    }

    let len = state.completions.len() as i32;
    let next = (state.selected as i32 + delta).rem_euclid(len);
    state.selected = next as usize;
    vec![Command::None]
}

fn handle_slash_execute(model: &mut Model) -> Vec<Command> {
    if model.input.trim().is_empty() {
        return vec![Command::None];
    }

    let parsed = slash::parse(&model.input);
    let selected_command = model
        .slash_state
        .as_ref()
        .and_then(|state| state.completions.get(state.selected))
        .and_then(|command_index| model.commands.get(*command_index))
        .cloned();

    let exact_command = slash::find_user_command(&model.commands, &parsed.name).cloned();
    let command = exact_command.or(selected_command);

    model.input.clear();
    model.cursor_pos = 0;
    model.slash_state = None;

    let Some(command) = command else {
        push_system_message(model, format!("Unknown slash command `/{}`.", parsed.name));
        return vec![Command::None];
    };

    match command.source {
        CommandSource::Skill(skill) => {
            model.pending_skill_injection = Some(build_skill_injection(&skill, &parsed.args_raw));
            vec![Command::None]
        }
        CommandSource::Builtin => match command.name.as_str() {
            "help" => {
                push_system_message(model, slash::format_help(&model.commands));
                vec![Command::None]
            }
            "clear" => {
                model.messages.clear();
                model.streaming_text.clear();
                model.scroll_offset = 0;
                model.pending_skill_injection = None;
                vec![Command::ClearConversation]
            }
            "compact" => {
                push_system_message(
                    model,
                    "Context compaction is not implemented yet.".to_string(),
                );
                vec![Command::None]
            }
            "debug" => {
                let next_mode = match parsed.args.first().map(|arg| arg.to_ascii_lowercase()) {
                    Some(arg) if arg == "on" => true,
                    Some(arg) if arg == "off" => false,
                    Some(arg) => {
                        push_system_message(model, format!("Usage: /debug [on|off] (got `{arg}`)"));
                        return vec![Command::None];
                    }
                    None => !model.debug_mode,
                };
                model.debug_mode = next_mode;
                let status = if model.debug_mode {
                    "enabled"
                } else {
                    "disabled"
                };
                push_system_message(model, format!("Debug logging {status}."));
                vec![Command::None]
            }
            "model" => {
                if let Some(model_id) = parsed.args.first() {
                    vec![Command::SetModel(model_id.clone())]
                } else {
                    push_system_message(
                        model,
                        format!("Loading available models for `{}`...", model.provider),
                    );
                    vec![Command::ListModels]
                }
            }
            "providers" => vec![Command::ListProviders],
            "login" => {
                if let Some(provider) = parsed.args.first() {
                    let name = parsed.args.get(1).cloned();
                    vec![Command::LoginProvider {
                        provider: provider.clone(),
                        name,
                    }]
                } else {
                    push_system_message(model, "Usage: /login <provider> [name]".to_string());
                    vec![Command::None]
                }
            }
            "logout" => {
                if let Some(account_id) = parsed.args.first() {
                    vec![Command::LogoutProvider(account_id.clone())]
                } else {
                    push_system_message(model, "Usage: /logout <account_id>".to_string());
                    vec![Command::None]
                }
            }
            other => {
                push_system_message(model, format!("Unknown slash command `/{other}`."));
                vec![Command::None]
            }
        },
    }
}

fn sync_slash_state(model: &mut Model) {
    model.slash_state = slash::state_for_input(&model.input, &model.commands);
}

fn push_system_message(model: &mut Model, content: String) {
    model.messages.push(ChatMessage {
        role: ChatRole::System,
        content,
    });
    model.scroll_offset = 0;
}

fn push_debug_message(model: &mut Model, content: String) {
    if model.debug_mode {
        push_system_message(model, format!("[debug] {content}"));
    }
}

fn build_skill_injection(skill: &skills::SkillDef, raw_args: &str) -> String {
    let mut injection = skills::render_body(&skill.body, raw_args);

    if let Some(manifest) = skills::format_resource_manifest(skill) {
        if !injection.ends_with('\n') && !injection.is_empty() {
            injection.push('\n');
        }
        if !injection.is_empty() {
            injection.push('\n');
        }
        injection.push_str(&manifest);
    }

    // Per-directory read allowlisting does not exist yet, so base_dir is only
    // surfaced in the injection manifest for now.
    injection
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::skills::{SkillDef, SkillResources};

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

    #[test]
    fn test_slash_input_opens_completion_state() {
        let mut model = Model::new(false);

        update(&mut model, Message::KeyInput('/'));

        let state = model.slash_state.as_ref().unwrap();
        assert_eq!(state.completions.len(), model.commands.len());
    }

    #[test]
    fn test_slash_backspace_clears_state() {
        let mut model = Model::new(false);
        model.input = "/".to_string();
        model.cursor_pos = 1;
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        update(&mut model, Message::Backspace);

        assert!(model.slash_state.is_none());
        assert!(model.input.is_empty());
    }

    #[test]
    fn test_slash_help_appends_formatted_commands() {
        let mut model = Model::new(false);
        model.input = "/help".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        let commands = update(&mut model, Message::SlashExecute);

        assert!(commands.iter().all(|command| command.is_none()));
        assert!(
            model
                .messages
                .last()
                .unwrap()
                .content
                .contains("Available slash commands:")
        );
    }

    #[test]
    fn test_slash_model_without_arg_dispatches_runtime_list() {
        let mut model = Model::new(false);
        model.provider = "openai".to_string();
        model.model_name = "gpt-4o".to_string();
        model.input = "/model".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        let commands = update(&mut model, Message::SlashExecute);

        assert!(matches!(commands.as_slice(), [Command::ListModels]));
        assert!(model.messages.last().unwrap().content.contains("Loading available models"));
    }

    #[test]
    fn test_slash_model_with_arg_dispatches_set_model() {
        let mut model = Model::new(false);
        model.provider = "openai".to_string();
        model.input = "/model gpt-4o-mini".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        let commands = update(&mut model, Message::SlashExecute);

        assert_eq!(commands.len(), 1);
        assert!(matches!(&commands[0], Command::SetModel(model) if model == "gpt-4o-mini"));
    }

    #[test]
    fn test_models_listed_message_formats_runtime_results() {
        let mut model = Model::new(false);
        model.provider = "openai".to_string();

        update(
            &mut model,
            Message::ModelsListed {
                current_model: "gpt-5".to_string(),
                models: vec!["gpt-5".to_string(), "o4-mini".to_string()],
            },
        );

        let content = &model.messages.last().unwrap().content;
        assert!(content.contains("Current model: `openai` / `gpt-5`."));
        assert!(content.contains("- `gpt-5`"));
        assert!(content.contains("- `o4-mini`"));
    }

    #[test]
    fn test_model_changed_updates_current_selection() {
        let mut model = Model::new(false);
        model.model_name = "gpt-4o".to_string();

        update(&mut model, Message::ModelChanged("gpt-5".to_string()));

        assert_eq!(model.model_name, "gpt-5");
        assert!(
            model
                .messages
                .last()
                .unwrap()
                .content
                .contains("Switched model to `gpt-5`.")
        );
    }

    #[test]
    fn test_slash_providers_dispatches_runtime_command() {
        let mut model = Model::new(false);
        model.input = "/providers".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        let commands = update(&mut model, Message::SlashExecute);

        assert!(matches!(commands.as_slice(), [Command::ListProviders]));
    }

    #[test]
    fn test_slash_login_dispatches_runtime_command() {
        let mut model = Model::new(false);
        model.input = "/login openai Work".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        let commands = update(&mut model, Message::SlashExecute);

        assert!(matches!(
            commands.as_slice(),
            [Command::LoginProvider { provider, name }]
            if provider == "openai" && name.as_deref() == Some("Work")
        ));
    }

    #[test]
    fn test_slash_debug_toggles_logging() {
        let mut model = Model::new(false);
        model.input = "/debug".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        let commands = update(&mut model, Message::SlashExecute);

        assert!(commands.iter().all(|command| command.is_none()));
        assert!(model.debug_mode);
        assert!(
            model
                .messages
                .last()
                .unwrap()
                .content
                .contains("Debug logging enabled")
        );
    }

    #[test]
    fn test_debug_messages_are_sent_to_chat_when_enabled() {
        let mut model = Model::new(true);

        update(
            &mut model,
            Message::AgentToolDone {
                name: "shell".to_string(),
                success: true,
            },
        );

        assert!(
            model
                .messages
                .last()
                .unwrap()
                .content
                .contains("[debug] Tool ok: shell")
        );
    }

    #[test]
    fn test_slash_logout_dispatches_runtime_command() {
        let mut model = Model::new(false);
        model.input = "/logout openai".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        let commands = update(&mut model, Message::SlashExecute);

        assert!(
            matches!(commands.as_slice(), [Command::LogoutProvider(account)] if account == "openai")
        );
    }

    #[test]
    fn test_skill_execution_stores_pending_injection() {
        let mut model = Model::new(false);
        model.commands = slash::merge_commands(vec![SkillDef {
            name: "greet".to_string(),
            description: "Greet somebody".to_string(),
            argument_hint: Some("[name]".to_string()),
            disable_model_invocation: false,
            user_invocable: true,
            body: "Hello, $ARGUMENTS!".to_string(),
            base_dir: PathBuf::from("/tmp/greet"),
            resources: SkillResources::default(),
        }]);
        model.input = "/greet Alice".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        let commands = update(&mut model, Message::SlashExecute);

        assert!(commands.iter().all(|command| command.is_none()));
        assert_eq!(
            model.pending_skill_injection.as_deref(),
            Some("Hello, Alice!")
        );
    }

    #[test]
    fn test_pending_skill_injection_is_prepended_on_submit() {
        let mut model = Model::new(false);
        model.pending_skill_injection = Some("System skill context".to_string());
        model.input = "Do the task".to_string();

        let commands = update(&mut model, Message::Submit);

        assert!(matches!(
            commands.as_slice(),
            [Command::SendToAgent(outbound)]
            if outbound == "System skill context\n\nDo the task"
        ));
        assert!(model.pending_skill_injection.is_none());
    }

    #[test]
    fn test_slash_unknown_command_appends_error() {
        let mut model = Model::new(false);
        model.input = "/wat".to_string();
        model.cursor_pos = model.input.len();
        model.slash_state = slash::state_for_input(&model.input, &model.commands);

        update(&mut model, Message::SlashExecute);

        assert!(
            model
                .messages
                .last()
                .unwrap()
                .content
                .contains("Unknown slash command")
        );
    }
}
