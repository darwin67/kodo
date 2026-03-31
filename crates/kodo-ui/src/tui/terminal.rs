use std::io;

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

/// Type alias for our specific terminal setup
pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Initialize the terminal for TUI mode.
///
/// This sets up the terminal for full-screen TUI application:
/// - Enables raw mode (disables line buffering, echo, etc.)
/// - Enters alternate screen (preserves terminal history)
/// - Creates a ratatui Terminal with crossterm backend
pub fn init_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to normal mode.
///
/// This cleans up the terminal state:
/// - Disables raw mode (restores normal terminal behavior)
/// - Leaves alternate screen (returns to original terminal content)
/// - Shows cursor (in case it was hidden)
///
/// Should always be called before the application exits to ensure
/// the terminal is left in a usable state.
pub fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
