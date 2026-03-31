use crate::command::Command;
use crate::message::{Message, ThemeChoice};
use crate::model::{ChatMessage, ChatRole, Model};
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
                ThemeChoice::Dark => Theme::dark(),
                ThemeChoice::Light => Theme::light(),
            };
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
                model.debug_logs.push(format!("🎨 Formatted: {}", message));
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

        Message::AgentDone => {
            model.is_streaming = false;
            vec![Command::None]
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
                    crate::keybinds::KeyAction::DarkTheme => Message::SetTheme(ThemeChoice::Dark),
                    crate::keybinds::KeyAction::LightTheme => Message::SetTheme(ThemeChoice::Light),
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

/// Handle selecting a command palette item
fn handle_palette_select(model: &mut Model) -> Vec<Command> {
    // This would need to be implemented based on available commands
    // For now, close the palette and handle common commands
    model.palette_open = false;

    // Simple command matching - in a real implementation this would
    // be more sophisticated with actual command definitions
    match model.palette_query.to_lowercase().as_str() {
        "quit" | "exit" => {
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
            vec![Command::None]
        }
        "light" => {
            model.theme = Theme::light();
            vec![Command::None]
        }
        _ => vec![Command::None],
    }
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
