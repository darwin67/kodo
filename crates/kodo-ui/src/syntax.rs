use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{Theme, ThemeSet},
    parsing::SyntaxSet,
};

/// Syntax highlighter for code blocks
#[derive(Debug)]
pub struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    theme: Theme,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let theme_set = ThemeSet::load_defaults();
        let theme = theme_set.themes["base16-ocean.dark"].clone();

        Self {
            syntax_set,
            theme_set,
            theme,
        }
    }

    /// Highlight a code block with the given language
    pub fn highlight_code(&self, code: &str, language: Option<&str>) -> Vec<Line<'static>> {
        let syntax = if let Some(lang) = language {
            self.syntax_set
                .find_syntax_by_token(lang)
                .or_else(|| self.syntax_set.find_syntax_by_extension(lang))
                .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text())
        } else {
            self.syntax_set.find_syntax_plain_text()
        };

        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let mut lines = Vec::new();

        for line in code.lines() {
            let highlighted = highlighter
                .highlight_line(line, &self.syntax_set)
                .unwrap_or_default();

            let spans: Vec<Span> = highlighted
                .into_iter()
                .map(|(style, text)| {
                    let color =
                        Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                    Span::styled(text.to_string(), Style::default().fg(color))
                })
                .collect();

            lines.push(Line::from(spans));
        }

        lines
    }

    /// Switch theme (for light/dark mode support)
    pub fn set_theme(&mut self, dark_mode: bool) {
        self.theme = if dark_mode {
            self.theme_set.themes["base16-ocean.dark"].clone()
        } else {
            self.theme_set.themes["base16-ocean.light"].clone()
        };
    }
}

/// Simple markdown parser focused on code blocks
pub struct MarkdownParser;

impl MarkdownParser {
    /// Parse markdown text and return structured content
    pub fn parse_with_syntax(text: &str, highlighter: &SyntaxHighlighter) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut in_code_block = false;
        let mut code_language: Option<String> = None;
        let mut code_content = String::new();

        for line in text.lines() {
            if line.starts_with("```") {
                if in_code_block {
                    // End of code block - highlight and add
                    let highlighted =
                        highlighter.highlight_code(&code_content, code_language.as_deref());
                    lines.extend(highlighted);

                    code_content.clear();
                    code_language = None;
                    in_code_block = false;
                } else {
                    // Start of code block
                    let lang = line.trim_start_matches("```").trim();
                    code_language = if lang.is_empty() {
                        None
                    } else {
                        Some(lang.to_string())
                    };
                    in_code_block = true;
                }
            } else if in_code_block {
                code_content.push_str(line);
                code_content.push('\n');
            } else {
                // Regular text line
                lines.push(Line::from(line.to_string()));
            }
        }

        // Handle unterminated code block
        if in_code_block && !code_content.is_empty() {
            let highlighted = highlighter.highlight_code(&code_content, code_language.as_deref());
            lines.extend(highlighted);
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_highlight_rust_code() {
        let highlighter = SyntaxHighlighter::new();
        let code = "fn main() {\n    println!(\"Hello, world!\");\n}";
        let lines = highlighter.highlight_code(code, Some("rust"));

        assert_eq!(lines.len(), 3);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_markdown_parsing() {
        let highlighter = SyntaxHighlighter::new();
        let markdown = "Some text\n```rust\nfn test() {}\n```\nMore text";
        let lines = MarkdownParser::parse_with_syntax(markdown, &highlighter);

        assert!(lines.len() >= 3); // Should have text + code + text
    }

    #[test]
    fn test_no_language_code_block() {
        let highlighter = SyntaxHighlighter::new();
        let markdown = "```\nsome code\n```";
        let lines = MarkdownParser::parse_with_syntax(markdown, &highlighter);

        assert!(!lines.is_empty());
    }
}
