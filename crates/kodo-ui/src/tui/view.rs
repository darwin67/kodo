use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{
    model::{ChatRole, Model},
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

    let slash_height = model
        .slash_state
        .as_ref()
        .map(|state| state.completions.len().min(7) as u16 + 2)
        .unwrap_or(0);

    // Main layout: status bar (1 line) at top, input (3 lines) at bottom, output fills middle
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                // Status bar
            Constraint::Min(1),                   // Output/chat area
            Constraint::Length(3 + slash_height), // Input area
        ])
        .split(area);

    render_status_bar(frame, model, chunks[0]);
    render_output(frame, model, chunks[1]);
    render_input(frame, model, chunks[2]);
}

/// Render the status bar showing mode, provider, model, and token counts
fn render_status_bar(frame: &mut Frame, model: &Model, area: Rect) {
    let mode_indicator = format!(" {} ", model.mode.to_uppercase());
    let provider_model = format!(" {} / {} ", model.provider, model.model_name);
    let tokens = format!(" {}i/{}o ", model.input_tokens, model.output_tokens);
    let slash_hint = " / commands ";

    let status =
        Line::from(vec![
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
            // Fill remaining space
            Span::styled(
                " ".repeat(area.width.saturating_sub(50) as usize),
                model.theme.status_style(),
            ),
            Span::styled(slash_hint, model.theme.status_style().fg(model.theme.muted)),
        ]);

    let bar = Paragraph::new(status).style(model.theme.status_style());
    frame.render_widget(bar, area);
}

/// Render the main chat output area with conversation history and streaming text
fn render_output(frame: &mut Frame, model: &Model, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

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
    let input_area = if let Some(state) = model.slash_state.as_ref() {
        let slash_rows = state.completions.len().min(7) as u16;
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(slash_rows + 2), Constraint::Length(3)])
            .split(area);
        render_slash_completions(frame, model, chunks[0]);
        chunks[1]
    } else {
        area
    };

    let input_text = if model.is_streaming {
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

    frame.render_widget(input, input_area);

    // Position cursor inside the input box
    if !model.is_streaming {
        frame.set_cursor_position((input_area.x + model.cursor_pos as u16 + 1, input_area.y + 1));
    }
}

fn render_slash_completions(frame: &mut Frame, model: &Model, area: Rect) {
    let Some(state) = model.slash_state.as_ref() else {
        return;
    };

    let selected = state
        .selected
        .min(state.completions.len().saturating_sub(1));
    let mut lines = Vec::new();

    for (index, command_index) in state.completions.iter().take(7).enumerate() {
        let Some(command) = model.commands.get(*command_index) else {
            continue;
        };
        let signature = command.signature();

        let style = if index == selected {
            model.theme.accent_style().add_modifier(Modifier::REVERSED)
        } else {
            model.theme.text_style()
        };

        lines.push(Line::styled(
            format!("  {signature:<26} {}", command.description),
            style,
        ));
    }

    let widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(model.theme.accent_style())
            .title(" Slash "),
    );

    frame.render_widget(widget, area);
}
