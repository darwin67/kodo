// Elm Architecture modules - core MVU pattern (framework-agnostic)
pub mod command;
pub mod message;
pub mod model;
pub mod update;

// Infrastructure modules - runtime and support
pub mod event;
pub mod keybinds;
pub mod skills;
pub mod slash;
pub mod syntax;
pub mod theme;

// UI implementation modules - framework-specific
pub mod tui; // ratatui implementation

// For backward compatibility, re-export commonly used TUI items
pub use tui::{Tui, init_terminal, restore_terminal, view};
