use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use tokio::sync::mpsc;

use crate::keybinds::KeyAction;
use crate::message::{Message, ThemeChoice};
use crate::model::{Model, ModelModalState, ProviderModalState};

/// Application events.
#[derive(Debug)]
pub enum Event {
    /// A key press event.
    Key(KeyEvent),
    /// Terminal resize.
    Resize(u16, u16),
    /// Periodic tick for UI updates (e.g. spinner, heartbeat).
    Tick,
}

/// Event handler that polls crossterm events on a background thread.
pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<Event>,
    // Keep the handle alive so the task doesn't get dropped.
    _tx: mpsc::UnboundedSender<Event>,
}

impl EventHandler {
    /// Create a new event handler with the given tick rate.
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let _tx = tx.clone();

        // Spawn a blocking thread for crossterm event polling.
        // crossterm::event::poll is blocking and can't be used with tokio directly.
        std::thread::spawn(move || {
            loop {
                if event::poll(tick_rate).unwrap_or(false) {
                    match event::read() {
                        Ok(CrosstermEvent::Key(key)) => {
                            // Only handle key press events (not release/repeat).
                            if key.kind == KeyEventKind::Press && tx.send(Event::Key(key)).is_err()
                            {
                                break;
                            }
                        }
                        Ok(CrosstermEvent::Resize(w, h)) => {
                            if tx.send(Event::Resize(w, h)).is_err() {
                                break;
                            }
                        }
                        _ => {}
                    }
                } else {
                    // No event within tick_rate — send a tick.
                    if tx.send(Event::Tick).is_err() {
                        break;
                    }
                }
            }
        });

        Self { rx, _tx }
    }

    /// Receive the next event (async).
    pub async fn next(&mut self) -> Result<Event> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("event channel closed"))
    }
}

/// Map crossterm events to application Messages following the Elm Architecture.
///
/// This function is context-aware - it reads the current model state to decide
/// which Message to produce. For example, key presses are handled differently
/// when the command palette is open vs. when typing in the input field.
///
/// Returns None for events that should be ignored in the current state.
pub fn map_event(event: &Event, model: &Model) -> Option<Message> {
    match event {
        Event::Key(key_event) => map_key_event(key_event, model),
        Event::Resize(width, height) => Some(Message::Resize(*width, *height)),
        Event::Tick => Some(Message::Tick),
    }
}

/// Map keyboard input to Messages based on current application state
fn map_key_event(key: &KeyEvent, model: &Model) -> Option<Message> {
    // Provider modal takes priority over everything except Ctrl+C
    if model.provider_modal != ProviderModalState::Closed {
        // Always allow Ctrl+C to quit
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            return Some(Message::Quit);
        }
        return map_provider_modal_input(key, model);
    }

    // Model modal takes priority
    if model.model_modal != ModelModalState::Closed {
        if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
            return Some(Message::Quit);
        }
        return map_model_modal_input(key, model);
    }

    // Check for leader key sequences first
    if model.leader_state.waiting_for_sequence {
        return handle_leader_sequence(key, model);
    }

    // Check if this is a leader key
    if model.keybinds.is_leader_key(key) {
        return Some(Message::StartLeaderSequence);
    }

    // Check for global keybind actions first (they work even in palette)
    if let Some(action) = model.keybinds.get_action(key) {
        return keybind_action_to_message(action, model);
    }

    // Handle Escape - context sensitive
    if matches!(key.code, KeyCode::Esc) && key.modifiers == KeyModifiers::NONE {
        if model.palette_open {
            return Some(Message::ClosePalette);
        } else {
            return None; // Ignore in other contexts
        }
    }

    // Command palette input handling (for non-global keys)
    if model.palette_open {
        return map_palette_input(key);
    }

    // Input field handling (when not in palette and not streaming)
    if !model.is_streaming {
        return map_input_events(key);
    }

    // Ignore all input while streaming
    None
}

/// Map key events when the provider connect modal is open
fn map_provider_modal_input(key: &KeyEvent, model: &Model) -> Option<Message> {
    match &model.provider_modal {
        ProviderModalState::EnterApiKey { .. } => {
            // API key input mode
            match key.code {
                KeyCode::Char(ch) => Some(Message::ProviderModalApiKeyInput(ch)),
                KeyCode::Backspace => Some(Message::ProviderModalApiKeyBackspace),
                KeyCode::Enter => Some(Message::ProviderModalApiKeySubmit),
                KeyCode::Esc => Some(Message::ProviderModalBack),
                _ => None,
            }
        }
        ProviderModalState::OAuthInProgress { .. } => {
            // Only allow escape/back while waiting for OAuth
            match key.code {
                KeyCode::Esc => Some(Message::ProviderModalBack),
                _ => None,
            }
        }
        ProviderModalState::AuthSuccess { .. } => {
            // Press enter to continue to model selection
            match key.code {
                KeyCode::Enter => Some(Message::ProviderModalSelect),
                KeyCode::Esc => Some(Message::ProviderModalBack),
                _ => None,
            }
        }
        ProviderModalState::AuthError { .. } => {
            // Press enter to go back and retry
            match key.code {
                KeyCode::Enter => Some(Message::ProviderModalSelect),
                KeyCode::Esc => Some(Message::ProviderModalBack),
                _ => None,
            }
        }
        _ => {
            // SelectProvider or SelectAuthMethod - list navigation
            match key.code {
                KeyCode::Up => Some(Message::ProviderModalUp),
                KeyCode::Down => Some(Message::ProviderModalDown),
                KeyCode::Enter => Some(Message::ProviderModalSelect),
                KeyCode::Esc => Some(Message::CloseProviderModal),
                _ => None,
            }
        }
    }
}

/// Map key events when the model selection modal is open
fn map_model_modal_input(key: &KeyEvent, _model: &Model) -> Option<Message> {
    match key.code {
        KeyCode::Up => Some(Message::ModelModalUp),
        KeyCode::Down => Some(Message::ModelModalDown),
        KeyCode::Enter => Some(Message::ModelModalSelect),
        KeyCode::Esc => Some(Message::CloseModelModal),
        _ => None,
    }
}

/// Handle leader key sequences
fn handle_leader_sequence(key: &KeyEvent, model: &Model) -> Option<Message> {
    if let Some(_action) = model.keybinds.get_leader_action(key.code) {
        // Execute the leader action and cancel the sequence
        Some(Message::ExecuteLeaderAction(key.code))
    } else {
        // Invalid sequence - cancel it
        Some(Message::CancelLeaderSequence)
    }
}

/// Convert a KeyAction to a Message
fn keybind_action_to_message(action: &KeyAction, model: &Model) -> Option<Message> {
    match action {
        KeyAction::Message(msg) => Some(msg.clone()),
        KeyAction::OpenPalette => {
            if model.palette_open {
                Some(Message::ClosePalette)
            } else {
                Some(Message::OpenPalette)
            }
        }
        KeyAction::ToggleMode => Some(Message::ToggleMode),
        KeyAction::ToggleDebug => Some(Message::ToggleDebugPanel),
        KeyAction::ToggleTheme => {
            // Toggle between dark and light theme based on current theme
            let new_theme = if model.theme.is_dark() {
                ThemeChoice::Light
            } else {
                ThemeChoice::Dark
            };
            Some(Message::SetTheme(new_theme))
        }
        KeyAction::Quit => Some(Message::Quit),
        KeyAction::None => None,
    }
}

/// Map key events when the command palette is open
fn map_palette_input(key: &KeyEvent) -> Option<Message> {
    match key.code {
        KeyCode::Char(ch) => Some(Message::PaletteInput(ch)),
        KeyCode::Backspace => Some(Message::PaletteBackspace),
        KeyCode::Enter => Some(Message::PaletteSelect),
        KeyCode::Up => Some(Message::PaletteUp),
        KeyCode::Down => Some(Message::PaletteDown),
        _ => None,
    }
}

/// Map key events for the main input field
fn map_input_events(key: &KeyEvent) -> Option<Message> {
    match key.code {
        KeyCode::Char(ch) => Some(Message::KeyInput(ch)),
        KeyCode::Backspace => Some(Message::Backspace),
        KeyCode::Delete => Some(Message::Delete),
        KeyCode::Left => Some(Message::CursorLeft),
        KeyCode::Right => Some(Message::CursorRight),
        KeyCode::Home => Some(Message::CursorHome),
        KeyCode::End => Some(Message::CursorEnd),
        KeyCode::Enter => Some(Message::Submit),
        KeyCode::PageUp => Some(Message::ScrollUp(10)),
        KeyCode::PageDown => Some(Message::ScrollDown(10)),
        KeyCode::Up => Some(Message::ScrollUp(1)),
        KeyCode::Down => Some(Message::ScrollDown(1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_model() -> Model {
        Model::new(false)
    }

    #[test]
    fn test_quit_shortcut() {
        let model = test_model();
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let event = Event::Key(key);

        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::Quit)));
    }

    #[test]
    fn test_palette_toggle() {
        let mut model = test_model();
        let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL);
        let event = Event::Key(key);

        // Should open palette when closed
        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::OpenPalette)));

        // Should close palette when open
        model.palette_open = true;
        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::ClosePalette)));
    }

    #[test]
    fn test_input_while_streaming() {
        let mut model = test_model();
        model.is_streaming = true;

        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE);
        let event = Event::Key(key);

        // Should ignore input while streaming
        let message = map_event(&event, &model);
        assert!(message.is_none());
    }

    #[test]
    fn test_palette_input() {
        let mut model = test_model();
        model.palette_open = true;

        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let event = Event::Key(key);

        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::PaletteInput('q'))));
    }

    #[test]
    fn test_regular_input() {
        let model = test_model();
        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE);
        let event = Event::Key(key);

        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::KeyInput('h'))));
    }
}
