use std::io::{self, BufRead, Write};

use anyhow::Result;

/// Simple readline-style input for Phase 1.
/// Will be replaced by ratatui TUI in Phase 6.
pub fn read_user_input(prompt: &str) -> Result<Option<String>> {
    print!("{prompt}");
    io::stdout().flush()?;

    let mut line = String::new();
    let bytes_read = io::stdin().lock().read_line(&mut line)?;

    if bytes_read == 0 {
        // EOF
        return Ok(None);
    }

    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }

    Ok(Some(trimmed))
}
