use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{
    model::{AuthMethod, ChatRole, Model, ModelModalState, ProviderModalState},
    syntax::{MarkdownParser, SyntaxHighlighter},
};

// Global syntax highlighter - initialized once
use std::sync::OnceLock;
static SYNTAX_HIGHLIGHTER: OnceLock<SyntaxHighlighter> = OnceLock::new();

/// Main view function following the Elm Architecture.
/// This is a PURE function that takes the model and renders it to the terminal.
/// No side effects, no I/O - just converts model state to visual representation.
///
/// This is the ONLY ratatui-specific code in the entire application.
/// When building a GUI version, only this module gets replaced -
/// everything else (model, message, update, command) stays the same.
pub fn view(frame: &mut Frame, model: &Model) {
    let area = frame.area();

    // Split screen for debug panel if needed
    let (main_area, debug_area) = if model.debug_mode && model.debug_panel_open {
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(area);
        (h_chunks[0], Some(h_chunks[1]))
    } else {
        (area, None)
    };

    // Main layout: status bar (1 line) at top, input (3 lines) at bottom, output fills middle
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Status bar
            Constraint::Min(1),    // Output/chat area
            Constraint::Length(3), // Input area
        ])
        .split(main_area);

    render_status_bar(frame, model, chunks[0]);
    render_output(frame, model, chunks[1]);
    render_input(frame, model, chunks[2]);

    // Debug side panel
    if let Some(debug_area) = debug_area {
        render_debug_panel(frame, model, debug_area);
    }

    // Command palette overlay (modal)
    if model.palette_open {
        render_palette(frame, model, area);
    }

    // Provider connect modal overlay (takes priority)
    if model.provider_modal != ProviderModalState::Closed {
        render_provider_modal(frame, model, area);
    }

    // Model selection modal overlay
    if model.model_modal != ModelModalState::Closed {
        render_model_modal(frame, model, area);
    }
}

/// Render the status bar showing mode, provider, model, and token counts
fn render_status_bar(frame: &mut Frame, model: &Model, area: Rect) {
    let mode_indicator = format!(" {} ", model.mode.to_uppercase());
    let provider_model = format!(" {} / {} ", model.provider, model.model_name);
    let tokens = format!(" {}i/{}o ", model.input_tokens, model.output_tokens);

    // Context window usage
    let context_info = if model.context_limit > 0 {
        let percent = (model.context_tokens as f32 / model.context_limit as f32 * 100.0).min(100.0);
        format!(
            " {}/{} ({:.0}%) ",
            model.context_tokens, model.context_limit, percent
        )
    } else {
        String::new()
    };

    let palette_hint = " Ctrl+K ";

    let mut spans =
        vec![
            Span::styled(
                mode_indicator,
                model.theme.status_style().add_modifier(Modifier::BOLD).fg(
                    if model.mode == "Plan" {
                        model.theme.accent
                    } else {
                        model.theme.success
                    },
                ),
            ),
            Span::styled(" | ", model.theme.status_style()),
            Span::styled(provider_model, model.theme.status_style()),
            Span::styled(" | ", model.theme.status_style()),
            Span::styled(tokens, model.theme.status_style()),
        ];

    // Add context info if available
    if !context_info.is_empty() {
        spans.push(Span::styled(" | ", model.theme.status_style()));

        // Color code based on usage
        let percent = (model.context_tokens as f32 / model.context_limit as f32 * 100.0).min(100.0);
        let context_color = if percent >= 80.0 {
            model.theme.error
        } else if percent >= 60.0 {
            Color::Yellow // warning color
        } else {
            model.theme.fg
        };

        spans.push(Span::styled(
            context_info,
            model.theme.status_style().fg(context_color),
        ));
    }

    // Calculate used width
    let used_width: usize = spans.iter().map(|s| s.content.len()).sum();
    let palette_width = palette_hint.len();
    let remaining = area
        .width
        .saturating_sub((used_width + palette_width) as u16) as usize;

    // Fill remaining space
    spans.push(Span::styled(
        " ".repeat(remaining),
        model.theme.status_style(),
    ));
    spans.push(Span::styled(
        palette_hint,
        model.theme.status_style().fg(model.theme.muted),
    ));

    let status = Line::from(spans);

    let bar = Paragraph::new(status).style(model.theme.status_style());
    frame.render_widget(bar, area);
}

/// Render the main chat output area with conversation history and streaming text
fn render_output(frame: &mut Frame, model: &Model, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    // Show welcome/status message when no messages yet
    if model.messages.is_empty() && !model.is_streaming {
        if model.needs_provider {
            // No provider connected
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  Welcome to kodo!",
                model.theme.text_style().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  No providers are configured.",
                model.theme.muted_style(),
            ));
            lines.push(Line::styled(
                "  Connect a provider in the modal above to get started.",
                model.theme.muted_style(),
            ));
        } else {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  Welcome to kodo!",
                model.theme.text_style().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  Provider: {} / {}", model.provider, model.model_name),
                model.theme.muted_style(),
            ));
            lines.push(Line::styled(
                "  Type a message below to start chatting.",
                model.theme.muted_style(),
            ));
            lines.push(Line::styled(
                "  Press Ctrl+K to open command palette.",
                model.theme.muted_style(),
            ));
        }
    }

    // Get or initialize the global syntax highlighter
    let highlighter = SYNTAX_HIGHLIGHTER.get_or_init(|| {
        let mut h = SyntaxHighlighter::new();
        h.set_theme(model.theme.is_dark());
        h
    });

    // Render existing messages
    for msg in &model.messages {
        let (prefix, style) = match msg.role {
            ChatRole::User => ("you> ", model.theme.user_style()),
            ChatRole::Assistant => ("kodo> ", model.theme.assistant_style()),
            ChatRole::Tool => ("  [tool] ", model.theme.tool_style()),
            ChatRole::System => ("", model.theme.muted_style()),
        };

        // Parse message content with syntax highlighting for assistant messages
        let content_lines = if matches!(msg.role, ChatRole::Assistant) {
            MarkdownParser::parse_with_syntax(&msg.content, highlighter)
        } else {
            // For non-assistant messages, use simple line splitting
            msg.content
                .lines()
                .map(|line| Line::from(line.to_string()))
                .collect()
        };

        // Add prefix to first line and indent subsequent lines
        for (i, content_line) in content_lines.into_iter().enumerate() {
            if i == 0 {
                // First line gets the role prefix
                let mut spans = vec![Span::styled(prefix, style.add_modifier(Modifier::BOLD))];
                spans.extend(content_line.spans);
                lines.push(Line::from(spans));
            } else {
                // Subsequent lines get indented
                let indent = " ".repeat(prefix.len());
                let mut spans = vec![Span::raw(indent)];
                spans.extend(content_line.spans);
                lines.push(Line::from(spans));
            }
        }
        lines.push(Line::raw("")); // Blank line between messages
    }

    // Show streaming text if active
    if model.is_streaming && !model.streaming_text.is_empty() {
        let style = model.theme.assistant_style();
        let prefix = "kodo> ";

        // Parse streaming text with syntax highlighting
        let content_lines = MarkdownParser::parse_with_syntax(&model.streaming_text, highlighter);

        for (i, content_line) in content_lines.into_iter().enumerate() {
            if i == 0 {
                let mut spans = vec![Span::styled(prefix, style.add_modifier(Modifier::BOLD))];
                spans.extend(content_line.spans);
                lines.push(Line::from(spans));
            } else {
                let indent = " ".repeat(prefix.len());
                let mut spans = vec![Span::raw(indent)];
                spans.extend(content_line.spans);
                lines.push(Line::from(spans));
            }
        }

        // Blinking cursor indicator
        lines.push(Line::from(Span::styled(
            "      ...",
            model.theme.muted_style(),
        )));
    }

    // Handle scrolling
    let total_lines = lines.len() as u16;
    let visible_height = area.height;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = max_scroll.saturating_sub(model.scroll_offset);

    let output = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    frame.render_widget(output, area);
}

/// Render the input area where users type messages
fn render_input(frame: &mut Frame, model: &Model, area: Rect) {
    let input_text = if model.needs_provider {
        " Connect a provider to start (see modal above)"
    } else if model.is_streaming {
        " (streaming...)"
    } else {
        &model.input
    };

    let input = Paragraph::new(input_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(model.theme.input_border_style())
                .title(" Message "),
        )
        .style(model.theme.text_style());

    frame.render_widget(input, area);

    // Position cursor inside the input box
    if !model.is_streaming {
        frame.set_cursor_position((area.x + model.cursor_pos as u16 + 1, area.y + 1));
    }
}

/// Render the debug panel showing debug logs
fn render_debug_panel(frame: &mut Frame, model: &Model, area: Rect) {
    let mut lines: Vec<Line> = model
        .debug_logs
        .iter()
        .map(|log| Line::styled(log.as_str(), model.theme.muted_style()))
        .collect();

    if lines.is_empty() {
        lines.push(Line::styled(
            "No debug logs yet.",
            model.theme.muted_style(),
        ));
    }

    // Handle scrolling for debug panel
    let total = lines.len() as u16;
    let visible = area.height.saturating_sub(2); // Account for border
    let max_scroll = total.saturating_sub(visible);
    let scroll = max_scroll.saturating_sub(model.debug_scroll);

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(model.theme.muted_style())
                .title(" Debug "),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    frame.render_widget(panel, area);
}

/// Render the command palette modal overlay
fn render_palette(frame: &mut Frame, model: &Model, area: Rect) {
    // Center the palette on screen
    let width = area.width.min(60);
    let height = area.height.min(20);
    let x = (area.width - width) / 2;
    let y = (area.height - height) / 2;
    let palette_area = Rect::new(x, y, width, height);

    // Get available commands and filter by query
    let commands = palette_commands();
    let filtered: Vec<&(&str, &str)> = if model.palette_query.is_empty() {
        commands.iter().collect()
    } else {
        let q = model.palette_query.to_lowercase();
        commands
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(&q))
            .collect()
    };

    // Render command list with selection highlighting
    let mut lines = Vec::new();
    let selected_index = model.palette_selected.min(filtered.len().saturating_sub(1));
    for (i, (name, desc)) in filtered.iter().enumerate() {
        let style = if i == selected_index {
            model.theme.accent_style().add_modifier(Modifier::REVERSED)
        } else {
            model.theme.text_style()
        };
        lines.push(Line::styled(format!("  {name:<25} {desc}"), style));
    }

    // Show search query in title
    let search_line = format!(" > {} ", model.palette_query);
    let palette = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(model.theme.accent_style())
            .title(search_line),
    );

    // Clear the area behind the palette (for proper modal appearance)
    let clear = Paragraph::new("").style(model.theme.text_style());
    frame.render_widget(clear, palette_area);
    frame.render_widget(palette, palette_area);
}

/// Render the provider connect modal
fn render_provider_modal(frame: &mut Frame, model: &Model, area: Rect) {
    let width = area.width.min(55);
    let height = area.height.min(18);
    let x = (area.width - width) / 2;
    let y = (area.height - height) / 2;
    let modal_area = Rect::new(x, y, width, height);

    // Clear background
    let clear = Paragraph::new("").style(model.theme.text_style());
    frame.render_widget(clear, modal_area);

    match &model.provider_modal {
        ProviderModalState::SelectProvider => {
            let mut lines = Vec::new();
            lines.push(Line::raw(""));

            if model.needs_provider {
                lines.push(Line::styled(
                    "  No providers authenticated.",
                    model.theme.text_style().fg(model.theme.error),
                ));
                lines.push(Line::styled(
                    "  Connect a provider to get started.",
                    model.theme.muted_style(),
                ));
                lines.push(Line::raw(""));
            }

            let selected_index = model
                .provider_modal_selected
                .min(model.provider_options.len().saturating_sub(1));
            for (i, option) in model.provider_options.iter().enumerate() {
                let indicator = if i == selected_index { "> " } else { "  " };
                let status = if option.is_authenticated {
                    " (connected)"
                } else {
                    ""
                };
                let style = if i == selected_index {
                    model.theme.accent_style().add_modifier(Modifier::BOLD)
                } else {
                    model.theme.text_style()
                };
                lines.push(Line::styled(
                    format!("{}{}{}", indicator, option.display_name, status),
                    style,
                ));
            }

            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  [Enter] Select  [Esc] Cancel",
                model.theme.muted_style(),
            ));

            let title = if model.needs_provider {
                " Connect Provider "
            } else {
                " Connect Provider "
            };
            let panel = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(model.theme.accent_style())
                    .title(title),
            );
            frame.render_widget(panel, modal_area);
        }

        ProviderModalState::SelectAuthMethod { provider } => {
            let mut lines = Vec::new();
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  How would you like to authenticate with {}?", provider),
                model.theme.text_style(),
            ));
            lines.push(Line::raw(""));

            if let Some(option) = model.provider_options.iter().find(|o| o.id == *provider) {
                for (i, method) in option.auth_methods.iter().enumerate() {
                    let indicator = if i == model.auth_method_selected {
                        "> "
                    } else {
                        "  "
                    };
                    let label = match method {
                        AuthMethod::OAuth => "Browser Login (auto-redirect)",
                        AuthMethod::OAuthCodePaste => "Browser Login (paste code)",
                        AuthMethod::ApiKey => "Enter API Key",
                    };
                    let style = if i == model.auth_method_selected {
                        model.theme.accent_style().add_modifier(Modifier::BOLD)
                    } else {
                        model.theme.text_style()
                    };
                    lines.push(Line::styled(format!("{}{}", indicator, label), style));
                }
            }

            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  [Enter] Select  [Esc] Back",
                model.theme.muted_style(),
            ));

            let panel = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(model.theme.accent_style())
                    .title(format!(" {} - Auth Method ", provider)),
            );
            frame.render_widget(panel, modal_area);
        }

        ProviderModalState::EnterApiKey { provider } => {
            let mut lines = Vec::new();
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  Enter your {} API key:", provider),
                model.theme.text_style(),
            ));
            lines.push(Line::raw(""));

            // Show masked key input
            let masked: String = if model.api_key_input.is_empty() {
                "  (paste or type your key)".to_string()
            } else {
                let len = model.api_key_input.len();
                if len <= 8 {
                    format!("  {}", "*".repeat(len))
                } else {
                    format!(
                        "  {}...{}",
                        &model.api_key_input[..4],
                        "*".repeat(len.min(20) - 4)
                    )
                }
            };
            let key_style = if model.api_key_input.is_empty() {
                model.theme.muted_style()
            } else {
                model.theme.text_style()
            };
            lines.push(Line::styled(masked, key_style));

            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  [Enter] Submit  [Esc] Back",
                model.theme.muted_style(),
            ));

            let panel = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(model.theme.accent_style())
                    .title(format!(" {} - API Key ", provider)),
            );
            frame.render_widget(panel, modal_area);
        }

        ProviderModalState::EnterOAuthCode { provider, auth_url } => {
            let mut lines = Vec::new();
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  1. Open this URL in your browser:",
                model.theme.text_style(),
            ));
            lines.push(Line::raw(""));
            // Truncate URL for display if needed
            let display_url = if auth_url.len() > (width as usize - 6) {
                format!("  {}...", &auth_url[..width as usize - 9])
            } else {
                format!("  {}", auth_url)
            };
            lines.push(Line::styled(display_url, model.theme.accent_style()));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  2. Sign in and authorize kodo",
                model.theme.text_style(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  3. Paste the authorization code below:",
                model.theme.text_style(),
            ));
            lines.push(Line::raw(""));

            // Show code input
            let code_display = if model.api_key_input.is_empty() {
                "  > (paste authorization code here)".to_string()
            } else {
                format!("  > {}", model.api_key_input)
            };
            let code_style = if model.api_key_input.is_empty() {
                model.theme.muted_style()
            } else {
                model.theme.text_style().add_modifier(Modifier::BOLD)
            };
            lines.push(Line::styled(code_display, code_style));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  [Enter] Submit  [Esc] Back",
                model.theme.muted_style(),
            ));

            let panel = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(model.theme.accent_style())
                    .title(format!(" {} - Paste Auth Code ", provider)),
            );
            frame.render_widget(panel, modal_area);
        }

        ProviderModalState::OAuthInProgress { provider } => {
            let mut lines = Vec::new();
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  Exchanging authorization code...",
                model.theme.text_style(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  Waiting for {} to respond...", provider),
                model.theme.muted_style(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled("  [Esc] Cancel", model.theme.muted_style()));

            let panel = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(model.theme.accent_style())
                    .title(format!(" {} - Authenticating... ", provider)),
            );
            frame.render_widget(panel, modal_area);
        }

        ProviderModalState::AuthSuccess { provider } => {
            let mut lines = Vec::new();
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  Successfully connected to {}!", provider),
                model.theme.text_style().fg(model.theme.success),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  Press Enter to choose a model.",
                model.theme.text_style(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  [Enter] Continue",
                model.theme.muted_style(),
            ));

            let panel = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(model.theme.text_style().fg(model.theme.success))
                    .title(format!(" {} - Connected ", provider)),
            );
            frame.render_widget(panel, modal_area);
        }

        ProviderModalState::AuthError { provider, error } => {
            let mut lines = Vec::new();
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  Authentication failed for {}", provider),
                model.theme.text_style().fg(model.theme.error),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("  Error: {}", error),
                model.theme.muted_style(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "  [Enter] Try Again  [Esc] Back",
                model.theme.muted_style(),
            ));

            let panel = Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(model.theme.text_style().fg(model.theme.error))
                    .title(format!(" {} - Error ", provider)),
            );
            frame.render_widget(panel, modal_area);
        }

        ProviderModalState::Closed => {} // Not rendered
    }
}

/// Render the model selection modal
fn render_model_modal(frame: &mut Frame, model: &Model, area: Rect) {
    let width = area.width.min(50);
    let height = area.height.min(16);
    let x = (area.width - width) / 2;
    let y = (area.height - height) / 2;
    let modal_area = Rect::new(x, y, width, height);

    // Clear background
    let clear = Paragraph::new("").style(model.theme.text_style());
    frame.render_widget(clear, modal_area);

    let mut lines = Vec::new();
    lines.push(Line::raw(""));
    lines.push(Line::styled("  Select a model:", model.theme.text_style()));
    lines.push(Line::raw(""));

    let selected_index = model
        .model_modal_selected
        .min(model.model_options.len().saturating_sub(1));
    for (i, option) in model.model_options.iter().enumerate() {
        let indicator = if i == selected_index { "> " } else { "  " };
        let style = if i == selected_index {
            model.theme.accent_style().add_modifier(Modifier::BOLD)
        } else {
            model.theme.text_style()
        };
        lines.push(Line::styled(
            format!("{}{}  ({})", indicator, option.display_name, option.id),
            style,
        ));
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  [Enter] Select  [Esc] Cancel",
        model.theme.muted_style(),
    ));

    let panel = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(model.theme.accent_style())
            .title(" Select Model "),
    );
    frame.render_widget(panel, modal_area);
}

/// Available commands for the command palette.
pub fn palette_commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("connect", "Connect a provider (OAuth / API key)"),
        ("switch_model", "Change the active model"),
        ("clear", "Clear conversation history"),
        ("dark", "Switch to dark theme"),
        ("light", "Switch to light theme"),
        ("quit", "Exit kodo"),
    ]
}
