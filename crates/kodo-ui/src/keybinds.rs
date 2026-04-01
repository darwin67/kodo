use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

use crate::message::Message;

/// A key combination that can trigger a command
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct KeyBind {
    pub key: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBind {
    pub fn new(key: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { key, modifiers }
    }

    /// Create a key bind with no modifiers
    pub fn key(key: KeyCode) -> Self {
        Self::new(key, KeyModifiers::NONE)
    }

    /// Create a key bind with Ctrl modifier
    pub fn ctrl(key: KeyCode) -> Self {
        Self::new(key, KeyModifiers::CONTROL)
    }

    /// Create a key bind with Shift modifier
    pub fn shift(key: KeyCode) -> Self {
        Self::new(key, KeyModifiers::SHIFT)
    }

    /// Create a key bind with Alt modifier
    pub fn alt(key: KeyCode) -> Self {
        Self::new(key, KeyModifiers::ALT)
    }
}

impl From<KeyEvent> for KeyBind {
    fn from(event: KeyEvent) -> Self {
        Self {
            key: event.code,
            modifiers: event.modifiers,
        }
    }
}

/// Action that can be triggered by a keybind
#[derive(Debug, Clone, PartialEq)]
pub enum KeyAction {
    /// Send a specific message
    Message(Message),
    /// Open command palette
    OpenPalette,
    /// Toggle between Plan and Build modes
    ToggleMode,
    /// Toggle debug panel
    ToggleDebug,
    /// Toggle between dark and light theme
    ToggleTheme,
    /// Quit application
    Quit,
    /// No action (for disabling keys)
    None,
}

/// Registry of keybinds that can be customized
#[derive(Debug)]
pub struct KeyBindRegistry {
    bindings: HashMap<KeyBind, KeyAction>,
    leader_key: Option<KeyCode>,
    leader_timeout_ms: u64,
    leader_bindings: HashMap<KeyCode, KeyAction>,
}

impl Default for KeyBindRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyBindRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            bindings: HashMap::new(),
            leader_key: None,
            leader_timeout_ms: 1000, // 1 second timeout for leader sequences
            leader_bindings: HashMap::new(),
        };
        registry.load_defaults();
        registry
    }

    /// Load default keybinds
    fn load_defaults(&mut self) {
        // Global keybinds (work in any mode) - all follow Ctrl+key pattern
        self.bind(KeyBind::ctrl(KeyCode::Char('c')), KeyAction::Quit);
        self.bind(KeyBind::ctrl(KeyCode::Char('k')), KeyAction::OpenPalette);
        self.bind(KeyBind::ctrl(KeyCode::Char('q')), KeyAction::Quit);
        self.bind(KeyBind::ctrl(KeyCode::Char('m')), KeyAction::ToggleMode);
        self.bind(KeyBind::ctrl(KeyCode::Char('t')), KeyAction::ToggleTheme);
        self.bind(KeyBind::key(KeyCode::F(12)), KeyAction::ToggleDebug);
        self.bind(KeyBind::key(KeyCode::Tab), KeyAction::ToggleMode);
    }

    /// Bind a key to an action
    pub fn bind(&mut self, key: KeyBind, action: KeyAction) {
        self.bindings.insert(key, action);
    }

    /// Set the leader key
    pub fn set_leader_key(&mut self, key: KeyCode) {
        self.leader_key = Some(key);
    }

    /// Bind a leader key sequence
    pub fn bind_leader(&mut self, key: KeyCode, action: KeyAction) {
        self.leader_bindings.insert(key, action);
    }

    /// Remove a keybind
    pub fn unbind(&mut self, key: &KeyBind) {
        self.bindings.remove(key);
    }

    /// Get the action for a key event
    pub fn get_action(&self, event: &KeyEvent) -> Option<&KeyAction> {
        let keybind = KeyBind::from(*event);
        self.bindings.get(&keybind)
    }

    /// Check if a key is the leader key
    pub fn is_leader_key(&self, event: &KeyEvent) -> bool {
        // Ctrl is the leader - any unbound Ctrl+key starts leader sequence
        if event.modifiers == KeyModifiers::CONTROL {
            let keybind = KeyBind::from(*event);
            !self.bindings.contains_key(&keybind)
        } else {
            false
        }
    }

    /// Get action for leader sequence
    pub fn get_leader_action(&self, key: KeyCode) -> Option<&KeyAction> {
        self.leader_bindings.get(&key)
    }

    /// Get all current bindings for display
    pub fn list_bindings(&self) -> Vec<(KeyBind, KeyAction)> {
        self.bindings
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Get all leader bindings for display
    pub fn list_leader_bindings(&self) -> Vec<(KeyCode, KeyAction)> {
        self.leader_bindings
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }

    /// Get the leader timeout in milliseconds
    pub fn leader_timeout_ms(&self) -> u64 {
        self.leader_timeout_ms
    }

    /// Format key for display
    pub fn format_key(keybind: &KeyBind) -> String {
        let mut parts = Vec::new();

        if keybind.modifiers.contains(KeyModifiers::CONTROL) {
            parts.push("Ctrl");
        }
        if keybind.modifiers.contains(KeyModifiers::ALT) {
            parts.push("Alt");
        }
        if keybind.modifiers.contains(KeyModifiers::SHIFT) {
            parts.push("Shift");
        }

        let key_str = match keybind.key {
            KeyCode::Char(c) => c.to_string(),
            KeyCode::F(n) => format!("F{}", n),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Backspace => "Backspace".to_string(),
            KeyCode::Delete => "Delete".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PageUp".to_string(),
            KeyCode::PageDown => "PageDown".to_string(),
            KeyCode::Up => "Up".to_string(),
            KeyCode::Down => "Down".to_string(),
            KeyCode::Left => "Left".to_string(),
            KeyCode::Right => "Right".to_string(),
            _ => "Unknown".to_string(),
        };

        if parts.is_empty() {
            key_str
        } else {
            format!("{}+{}", parts.join("+"), key_str)
        }
    }

    /// Format leader key sequence for display
    pub fn format_leader_key(&self, key: KeyCode) -> String {
        if let Some(leader) = self.leader_key {
            let leader_str = match leader {
                KeyCode::Char(c) => c.to_string(),
                other => Self::format_key(&KeyBind::key(other)),
            };
            let key_str = match key {
                KeyCode::Char(c) => c.to_string(),
                other => Self::format_key(&KeyBind::key(other)),
            };
            format!("{} {}", leader_str, key_str)
        } else {
            "No leader".to_string()
        }
    }
}

/// State for handling leader key sequences
#[derive(Debug)]
pub struct LeaderState {
    pub waiting_for_sequence: bool,
    pub started_at: Option<std::time::Instant>,
    pub timeout_ms: u64,
}

impl LeaderState {
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            waiting_for_sequence: false,
            started_at: None,
            timeout_ms,
        }
    }

    /// Start waiting for a leader sequence
    pub fn start_sequence(&mut self) {
        self.waiting_for_sequence = true;
        self.started_at = Some(std::time::Instant::now());
    }

    /// Cancel the current sequence
    pub fn cancel_sequence(&mut self) {
        self.waiting_for_sequence = false;
        self.started_at = None;
    }

    /// Check if the sequence has timed out
    pub fn is_timed_out(&self) -> bool {
        if let Some(start) = self.started_at {
            start.elapsed().as_millis() > self.timeout_ms as u128
        } else {
            false
        }
    }

    /// Check and handle timeout
    pub fn check_timeout(&mut self) -> bool {
        if self.waiting_for_sequence && self.is_timed_out() {
            self.cancel_sequence();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keybind_creation() {
        let kb = KeyBind::ctrl(KeyCode::Char('c'));
        assert_eq!(kb.key, KeyCode::Char('c'));
        assert!(kb.modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn test_default_bindings() {
        let registry = KeyBindRegistry::new();
        assert!(!registry.bindings.is_empty());
    }

    #[test]
    fn test_custom_binding() {
        let mut registry = KeyBindRegistry::new();
        let key = KeyBind::alt(KeyCode::Char('x'));
        registry.bind(key.clone(), KeyAction::Quit);

        assert!(registry.bindings.contains_key(&key));
    }

    #[test]
    fn test_direct_keybinds() {
        let registry = KeyBindRegistry::new();
        let action = registry.get_action(&crossterm::event::KeyEvent::new(
            KeyCode::Char('c'),
            crossterm::event::KeyModifiers::CONTROL,
        ));
        assert!(matches!(action, Some(KeyAction::Quit)));
    }

    #[test]
    fn test_key_formatting() {
        let key = KeyBind::ctrl(KeyCode::Char('c'));
        assert_eq!(KeyBindRegistry::format_key(&key), "Ctrl+c");

        let simple_key = KeyBind::key(KeyCode::Tab);
        assert_eq!(KeyBindRegistry::format_key(&simple_key), "Tab");
    }

    #[test]
    fn test_leader_state() {
        let mut state = LeaderState::new(1000);
        assert!(!state.waiting_for_sequence);

        state.start_sequence();
        assert!(state.waiting_for_sequence);

        state.cancel_sequence();
        assert!(!state.waiting_for_sequence);
    }
}
