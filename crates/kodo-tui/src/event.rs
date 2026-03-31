use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use tokio::sync::mpsc;

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
    match (key.code, key.modifiers) {
        // Global shortcuts (work regardless of state)
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(Message::Quit),
        (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
            if model.palette_open {
                Some(Message::ClosePalette)
            } else {
                Some(Message::OpenPalette)
            }
        }
        (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
            // Toggle theme
            let new_theme = if model.theme.name == "dark" {
                ThemeChoice::Light
            } else {
                ThemeChoice::Dark
            };
            Some(Message::SetTheme(new_theme))
        }
        (KeyCode::F(12), KeyModifiers::NONE) => Some(Message::ToggleDebugPanel),
        (KeyCode::Tab, KeyModifiers::NONE) => Some(Message::ToggleMode),

        // Handle Escape - context sensitive
        (KeyCode::Esc, KeyModifiers::NONE) => {
            if model.palette_open {
                Some(Message::ClosePalette)
            } else {
                None // Ignore in other contexts
            }
        }

        // Command palette input handling
        _ if model.palette_open => map_palette_input(key),

        // Input field handling (when not in palette)
        _ if !model.is_streaming => map_input_events(key),

        // Ignore all input while streaming
        _ => None,
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
    use crate::theme::Theme;

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
