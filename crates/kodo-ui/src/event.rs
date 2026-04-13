use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use tokio::sync::mpsc;

use crate::keybinds::KeyAction;
use crate::message::{Message, ThemeChoice};
use crate::model::Model;

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
    _tx: mpsc::UnboundedSender<Event>,
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl EventHandler {
    /// Create a new event handler with the given tick rate.
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let _tx = tx.clone();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();

        let handle = std::thread::spawn(move || {
            loop {
                if stop_rx.try_recv().is_ok() {
                    break;
                }

                if event::poll(tick_rate).unwrap_or(false) {
                    match event::read() {
                        Ok(CrosstermEvent::Key(key)) => {
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
                } else if tx.send(Event::Tick).is_err() {
                    break;
                }
            }
        });

        Self {
            rx,
            _tx,
            stop_tx: Some(stop_tx),
            handle: Some(handle),
        }
    }

    /// Receive the next event (async).
    pub async fn next(&mut self) -> Result<Event> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("event channel closed"))
    }

    pub fn shutdown(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }

        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Map crossterm events to application Messages following the Elm Architecture.
pub fn map_event(event: &Event, model: &Model) -> Option<Message> {
    match event {
        Event::Key(key_event) => map_key_event(key_event, model),
        Event::Resize(width, height) => Some(Message::Resize(*width, *height)),
        Event::Tick => Some(Message::Tick),
    }
}

/// Map keyboard input to Messages based on current application state.
fn map_key_event(key: &KeyEvent, model: &Model) -> Option<Message> {
    if model.leader_state.waiting_for_sequence {
        return handle_leader_sequence(key, model);
    }

    if !model.palette_open
        && model.slash_is_active()
        && let Some(message) = map_slash_input(key)
    {
        return Some(message);
    }

    if model.keybinds.is_leader_key(key) {
        return Some(Message::StartLeaderSequence);
    }

    if let Some(action) = model.keybinds.get_action(key) {
        return keybind_action_to_message(action, model);
    }

    if matches!(key.code, KeyCode::Esc) && key.modifiers == KeyModifiers::NONE {
        if model.palette_open {
            return Some(Message::ClosePalette);
        }
        return None;
    }

    if model.palette_open {
        return map_palette_input(key);
    }

    if !model.is_streaming {
        return map_input_events(key);
    }

    None
}

fn handle_leader_sequence(key: &KeyEvent, model: &Model) -> Option<Message> {
    if model.keybinds.get_leader_action(key.code).is_some() {
        Some(Message::ExecuteLeaderAction(key.code))
    } else {
        Some(Message::CancelLeaderSequence)
    }
}

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

fn map_slash_input(key: &KeyEvent) -> Option<Message> {
    match (key.code, key.modifiers) {
        (KeyCode::Tab, KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
            Some(Message::SlashNav(1))
        }
        (KeyCode::BackTab, _) | (KeyCode::Up, KeyModifiers::NONE) => Some(Message::SlashNav(-1)),
        (KeyCode::Esc, KeyModifiers::NONE) => Some(Message::SlashCancel),
        (KeyCode::Enter, KeyModifiers::NONE) => Some(Message::SlashExecute),
        _ => map_input_events(key),
    }
}

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

        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::OpenPalette)));

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
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let event = Event::Key(key);

        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::KeyInput('q'))));
    }

    #[test]
    fn test_slash_tab_navigates_instead_of_toggling_mode() {
        let mut model = test_model();
        model.input = "/".to_string();
        model.slash_state = crate::slash::state_for_input(&model.input, &model.commands);

        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        let event = Event::Key(key);

        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::SlashNav(1))));
    }

    #[test]
    fn test_slash_enter_executes_command() {
        let mut model = test_model();
        model.input = "/help".to_string();
        model.slash_state = crate::slash::state_for_input(&model.input, &model.commands);

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let event = Event::Key(key);

        let message = map_event(&event, &model);
        assert!(matches!(message, Some(Message::SlashExecute)));
    }
}
