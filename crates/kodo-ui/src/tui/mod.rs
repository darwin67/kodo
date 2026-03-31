/// TUI (Terminal User Interface) implementation using ratatui.
///
/// This module contains all ratatui-specific rendering and terminal management code.
/// When adding support for other UI frameworks (like iced for GUI), this module
/// can be swapped out while keeping all the Elm Architecture core modules
/// (model, message, update, command) unchanged.
///
/// The tui module provides:
/// - view: Pure rendering functions that convert Model -> ratatui widgets
/// - terminal: Terminal initialization and cleanup utilities
///
/// For a GUI implementation, create a separate `gui` module with equivalent
/// functionality using the target GUI framework.
pub mod terminal;
pub mod view;

// Re-export commonly used items for convenience
pub use terminal::{Tui, init_terminal, restore_terminal};
pub use view::view;
