use ratatui::style::{Color, Modifier, Style};

/// Color theme for the TUI.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    /// Background of the main area.
    pub bg: Color,
    /// Default text color.
    pub fg: Color,
    /// Muted/secondary text.
    pub muted: Color,
    /// Accent color for highlights, borders, prompts.
    pub accent: Color,
    /// Error/warning color.
    pub error: Color,
    /// Success color.
    pub success: Color,
    /// Status bar background.
    pub status_bg: Color,
    /// Status bar foreground.
    pub status_fg: Color,
    /// Input area border color.
    pub input_border: Color,
    /// User message color.
    pub user_msg: Color,
    /// Assistant message color.
    pub assistant_msg: Color,
    /// Tool call indicator color.
    pub tool_call: Color,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            name: "dark",
            bg: Color::Reset,
            fg: Color::White,
            muted: Color::DarkGray,
            accent: Color::Cyan,
            error: Color::Red,
            success: Color::Green,
            status_bg: Color::DarkGray,
            status_fg: Color::White,
            input_border: Color::Cyan,
            user_msg: Color::White,
            assistant_msg: Color::White,
            tool_call: Color::Yellow,
        }
    }

    pub fn light() -> Self {
        Self {
            name: "light",
            bg: Color::Reset,
            fg: Color::Black,
            muted: Color::Gray,
            accent: Color::Blue,
            error: Color::Red,
            success: Color::Green,
            status_bg: Color::Gray,
            status_fg: Color::Black,
            input_border: Color::Blue,
            user_msg: Color::Black,
            assistant_msg: Color::Black,
            tool_call: Color::Magenta,
        }
    }

    /// Style for normal text.
    pub fn text_style(&self) -> Style {
        Style::default().fg(self.fg)
    }

    /// Style for muted/secondary text.
    pub fn muted_style(&self) -> Style {
        Style::default().fg(self.muted)
    }

    /// Style for accent/highlighted text.
    pub fn accent_style(&self) -> Style {
        Style::default().fg(self.accent)
    }

    /// Style for the status bar.
    pub fn status_style(&self) -> Style {
        Style::default().bg(self.status_bg).fg(self.status_fg)
    }

    /// Style for input area border.
    pub fn input_border_style(&self) -> Style {
        Style::default().fg(self.input_border)
    }

    /// Style for user messages.
    pub fn user_style(&self) -> Style {
        Style::default()
            .fg(self.user_msg)
            .add_modifier(Modifier::BOLD)
    }

    /// Style for assistant messages.
    pub fn assistant_style(&self) -> Style {
        Style::default().fg(self.assistant_msg)
    }

    /// Style for tool call indicators.
    pub fn tool_style(&self) -> Style {
        Style::default().fg(self.tool_call)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}
