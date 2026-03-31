use std::io;

use anyhow::Result;
use crossterm::{
    event::{KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::event::Event;
use crate::theme::Theme;

/// A message in the conversation display.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
    System,
}

/// Application state.
pub struct App {
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Current input buffer.
    pub input: String,
    /// Cursor position within the input.
    pub cursor_pos: usize,
    /// Chat history for display.
    pub messages: Vec<ChatMessage>,
    /// Scroll offset for the output panel (0 = bottom).
    pub scroll_offset: u16,
    /// Current mode display string.
    pub mode: String,
    /// Current provider name.
    pub provider: String,
    /// Current model name.
    pub model: String,
    /// Total input tokens used.
    pub input_tokens: u64,
    /// Total output tokens used.
    pub output_tokens: u64,
    /// Whether the assistant is currently streaming.
    pub is_streaming: bool,
    /// Current streaming text buffer.
    pub streaming_text: String,
    /// Color theme.
    pub theme: Theme,
    /// Whether the command palette is open.
    pub palette_open: bool,
    /// Command palette search query.
    pub palette_query: String,
    /// Command palette selected index.
    pub palette_selected: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            input: String::new(),
            cursor_pos: 0,
            messages: vec![ChatMessage {
                role: ChatRole::System,
                content: format!("kodo v{}", env!("CARGO_PKG_VERSION")),
            }],
            scroll_offset: 0,
            mode: "build".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-20250514".into(),
            input_tokens: 0,
            output_tokens: 0,
            is_streaming: false,
            streaming_text: String::new(),
            theme: Theme::dark(),
            palette_open: false,
            palette_query: String::new(),
            palette_selected: 0,
        }
    }

    /// Add a message to the chat history.
    pub fn push_message(&mut self, role: ChatRole, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            role,
            content: content.into(),
        });
        self.scroll_offset = 0; // Auto-scroll to bottom.
    }

    /// Append text to the current streaming buffer.
    pub fn append_streaming(&mut self, text: &str) {
        self.streaming_text.push_str(text);
    }

    /// Finalize streaming: move buffer to messages.
    pub fn finish_streaming(&mut self) {
        if !self.streaming_text.is_empty() {
            let text = std::mem::take(&mut self.streaming_text);
            self.push_message(ChatRole::Assistant, text);
        }
        self.is_streaming = false;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Terminal setup/teardown
// ---------------------------------------------------------------------------

pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Initialize the terminal for TUI mode.
pub fn init_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to normal mode.
pub fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// UI Rendering
// ---------------------------------------------------------------------------

/// Render the full application UI.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Layout: status bar (1 line) at top, input (3 lines) at bottom, output fills middle.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Status bar
            Constraint::Min(1),    // Output/chat area
            Constraint::Length(3), // Input area
        ])
        .split(area);

    render_status_bar(frame, app, chunks[0]);
    render_output(frame, app, chunks[1]);
    render_input(frame, app, chunks[2]);

    // Command palette overlay.
    if app.palette_open {
        render_palette(frame, app, area);
    }
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let mode_indicator = format!(" {} ", app.mode.to_uppercase());
    let provider_model = format!(" {} / {} ", app.provider, app.model);
    let tokens = format!(" {}i/{}o ", app.input_tokens, app.output_tokens);
    let palette_hint = " Ctrl+K ";

    let status = Line::from(vec![
        Span::styled(
            mode_indicator,
            app.theme
                .status_style()
                .add_modifier(Modifier::BOLD)
                .fg(if app.mode == "plan" {
                    app.theme.accent
                } else {
                    app.theme.success
                }),
        ),
        Span::styled(" | ", app.theme.status_style()),
        Span::styled(provider_model, app.theme.status_style()),
        Span::styled(" | ", app.theme.status_style()),
        Span::styled(tokens, app.theme.status_style()),
        // Fill remaining space.
        Span::styled(
            " ".repeat(area.width.saturating_sub(50) as usize),
            app.theme.status_style(),
        ),
        Span::styled(palette_hint, app.theme.status_style().fg(app.theme.muted)),
    ]);

    let bar = Paragraph::new(status).style(app.theme.status_style());
    frame.render_widget(bar, area);
}

fn render_output(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    for msg in &app.messages {
        let (prefix, style) = match msg.role {
            ChatRole::User => ("you> ", app.theme.user_style()),
            ChatRole::Assistant => ("kodo> ", app.theme.assistant_style()),
            ChatRole::Tool => ("  [tool] ", app.theme.tool_style()),
            ChatRole::System => ("", app.theme.muted_style()),
        };

        for (i, line) in msg.content.lines().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
                    Span::styled(line, style),
                ]));
            } else {
                let indent = " ".repeat(prefix.len());
                lines.push(Line::from(vec![
                    Span::raw(indent),
                    Span::styled(line, style),
                ]));
            }
        }
        lines.push(Line::raw("")); // Blank line between messages.
    }

    // Show streaming text if active.
    if app.is_streaming && !app.streaming_text.is_empty() {
        let style = app.theme.assistant_style();
        for (i, line) in app.streaming_text.lines().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled("kodo> ", style.add_modifier(Modifier::BOLD)),
                    Span::styled(line, style),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(line, style),
                ]));
            }
        }
        // Blinking cursor indicator.
        lines.push(Line::from(Span::styled(
            "      ...",
            app.theme.muted_style(),
        )));
    }

    let total_lines = lines.len() as u16;
    let visible_height = area.height;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = max_scroll.saturating_sub(app.scroll_offset);

    let output = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    frame.render_widget(output, area);
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let input_text = if app.is_streaming {
        " (streaming...)"
    } else {
        &app.input
    };

    let input = Paragraph::new(input_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(app.theme.input_border_style())
                .title(" Message "),
        )
        .style(app.theme.text_style());

    frame.render_widget(input, area);

    // Position cursor inside the input box.
    if !app.is_streaming {
        frame.set_cursor_position((area.x + app.cursor_pos as u16 + 1, area.y + 1));
    }
}

fn render_palette(frame: &mut Frame, app: &App, area: Rect) {
    // Center the palette.
    let width = area.width.min(60);
    let height = area.height.min(20);
    let x = (area.width - width) / 2;
    let y = (area.height - height) / 2;
    let palette_area = Rect::new(x, y, width, height);

    // Commands list.
    let commands = palette_commands();
    let filtered: Vec<&(&str, &str)> = if app.palette_query.is_empty() {
        commands.iter().collect()
    } else {
        let q = app.palette_query.to_lowercase();
        commands
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(&q))
            .collect()
    };

    let mut lines = Vec::new();
    for (i, (name, desc)) in filtered.iter().enumerate() {
        let style = if i == app.palette_selected {
            app.theme.accent_style().add_modifier(Modifier::REVERSED)
        } else {
            app.theme.text_style()
        };
        lines.push(Line::styled(format!("  {name:<25} {desc}"), style));
    }

    let search_line = format!(" > {} ", app.palette_query);
    let palette = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(app.theme.accent_style())
            .title(search_line),
    );

    // Clear the area behind the palette.
    let clear = Paragraph::new("").style(app.theme.text_style());
    frame.render_widget(clear, palette_area);
    frame.render_widget(palette, palette_area);
}

/// Available commands for the palette.
pub fn palette_commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Switch Model", "Change the active model"),
        ("Switch Provider", "Change the LLM provider"),
        ("Plan Mode", "Switch to read-only mode"),
        ("Build Mode", "Switch to full execution mode"),
        ("Dark Theme", "Switch to dark theme"),
        ("Light Theme", "Switch to light theme"),
        ("Undo Last Edit", "Revert last file change"),
        ("Show Tools", "List registered tools"),
        ("New Session", "Start a new session"),
        ("Quit", "Exit kodo"),
    ]
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

/// The result of handling a key event.
pub enum Action {
    /// No action needed.
    None,
    /// User submitted input — process this message.
    Submit(String),
    /// User requested quit.
    Quit,
    /// Palette command was selected.
    PaletteCommand(String),
}

/// Handle a key event and return the resulting action.
pub fn handle_key(app: &mut App, event: &Event) -> Action {
    if let Event::Key(key) = event {
        // Command palette handling.
        if app.palette_open {
            return handle_palette_key(app, key);
        }

        // Global keybinds.
        match (key.modifiers, key.code) {
            // Ctrl+K: open command palette.
            (KeyModifiers::CONTROL, KeyCode::Char('k')) => {
                app.palette_open = true;
                app.palette_query.clear();
                app.palette_selected = 0;
                return Action::None;
            }
            // Ctrl+C or Ctrl+D: quit.
            (KeyModifiers::CONTROL, KeyCode::Char('c' | 'd')) => {
                return Action::Quit;
            }
            _ => {}
        }

        // Input handling (when not streaming).
        if !app.is_streaming {
            match key.code {
                KeyCode::Enter => {
                    let input = app.input.trim().to_string();
                    if !input.is_empty() {
                        app.input.clear();
                        app.cursor_pos = 0;
                        return Action::Submit(input);
                    }
                }
                KeyCode::Char(c) => {
                    app.input.insert(app.cursor_pos, c);
                    app.cursor_pos += 1;
                }
                KeyCode::Backspace => {
                    if app.cursor_pos > 0 {
                        app.cursor_pos -= 1;
                        app.input.remove(app.cursor_pos);
                    }
                }
                KeyCode::Delete => {
                    if app.cursor_pos < app.input.len() {
                        app.input.remove(app.cursor_pos);
                    }
                }
                KeyCode::Left => {
                    app.cursor_pos = app.cursor_pos.saturating_sub(1);
                }
                KeyCode::Right => {
                    if app.cursor_pos < app.input.len() {
                        app.cursor_pos += 1;
                    }
                }
                KeyCode::Home => {
                    app.cursor_pos = 0;
                }
                KeyCode::End => {
                    app.cursor_pos = app.input.len();
                }
                KeyCode::PageUp => {
                    app.scroll_offset = app.scroll_offset.saturating_add(10);
                }
                KeyCode::PageDown => {
                    app.scroll_offset = app.scroll_offset.saturating_sub(10);
                }
                _ => {}
            }
        }
    }

    Action::None
}

fn handle_palette_key(app: &mut App, key: &crossterm::event::KeyEvent) -> Action {
    let commands = palette_commands();
    let q = app.palette_query.to_lowercase();
    let filtered_count = if q.is_empty() {
        commands.len()
    } else {
        commands
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(&q))
            .count()
    };

    match key.code {
        KeyCode::Esc => {
            app.palette_open = false;
            Action::None
        }
        KeyCode::Enter => {
            let filtered: Vec<&(&str, &str)> = if q.is_empty() {
                commands.iter().collect()
            } else {
                commands
                    .iter()
                    .filter(|(name, _)| name.to_lowercase().contains(&q))
                    .collect()
            };

            if let Some((name, _)) = filtered.get(app.palette_selected) {
                let cmd = name.to_string();
                app.palette_open = false;
                Action::PaletteCommand(cmd)
            } else {
                app.palette_open = false;
                Action::None
            }
        }
        KeyCode::Up => {
            app.palette_selected = app.palette_selected.saturating_sub(1);
            Action::None
        }
        KeyCode::Down => {
            if app.palette_selected + 1 < filtered_count {
                app.palette_selected += 1;
            }
            Action::None
        }
        KeyCode::Char(c) => {
            app.palette_query.push(c);
            app.palette_selected = 0;
            Action::None
        }
        KeyCode::Backspace => {
            app.palette_query.pop();
            app.palette_selected = 0;
            Action::None
        }
        _ => Action::None,
    }
}
