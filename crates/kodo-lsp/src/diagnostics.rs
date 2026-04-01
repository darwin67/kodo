use serde::{Deserialize, Serialize};

/// A simplified diagnostic from an LSP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
    Hint,
}

impl std::fmt::Display for DiagnosticSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => write!(f, "error"),
            Self::Warning => write!(f, "warning"),
            Self::Info => write!(f, "info"),
            Self::Hint => write!(f, "hint"),
        }
    }
}

/// Parse an LSP `textDocument/publishDiagnostics` params into our Diagnostic type.
pub fn parse_diagnostics(params: &serde_json::Value) -> Vec<Diagnostic> {
    let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("");

    let file = uri_to_path(uri);

    let diagnostics = params
        .get("diagnostics")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    diagnostics
        .iter()
        .filter_map(|d| {
            let range = d.get("range")?;
            let start = range.get("start")?;
            let line = start.get("line")?.as_u64()? as u32;
            let column = start.get("character")?.as_u64()? as u32;

            let severity_num = d.get("severity").and_then(|v| v.as_u64()).unwrap_or(1);

            let severity = match severity_num {
                1 => DiagnosticSeverity::Error,
                2 => DiagnosticSeverity::Warning,
                3 => DiagnosticSeverity::Info,
                4 => DiagnosticSeverity::Hint,
                _ => DiagnosticSeverity::Error,
            };

            let message = d
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let source = d
                .get("source")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            Some(Diagnostic {
                file: file.clone(),
                line: line + 1, // LSP is 0-indexed, display as 1-indexed.
                column: column + 1,
                severity,
                message,
                source,
            })
        })
        .collect()
}

/// Format diagnostics as a string for LLM context injection.
pub fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "No diagnostics.".to_string();
    }

    let mut output = format!("{} diagnostic(s):\n", diagnostics.len());
    for d in diagnostics {
        let source = d
            .source
            .as_deref()
            .map(|s| format!(" [{s}]"))
            .unwrap_or_default();
        output.push_str(&format!(
            "  {}:{}:{}: {}: {}{}\n",
            d.file, d.line, d.column, d.severity, d.message, source
        ));
    }
    output
}

/// Convert a file:// URI to a filesystem path.
fn uri_to_path(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_diagnostics() {
        let params = serde_json::json!({
            "uri": "file:///tmp/test.rs",
            "diagnostics": []
        });
        let diags = parse_diagnostics(&params);
        assert!(diags.is_empty());
    }

    #[test]
    fn parse_error_diagnostic() {
        let params = serde_json::json!({
            "uri": "file:///tmp/test.rs",
            "diagnostics": [{
                "range": {
                    "start": {"line": 5, "character": 10},
                    "end": {"line": 5, "character": 15}
                },
                "severity": 1,
                "message": "expected `;`",
                "source": "rust-analyzer"
            }]
        });
        let diags = parse_diagnostics(&params);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file, "/tmp/test.rs");
        assert_eq!(diags[0].line, 6); // 1-indexed
        assert_eq!(diags[0].column, 11);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diags[0].message, "expected `;`");
        assert_eq!(diags[0].source, Some("rust-analyzer".into()));
    }

    #[test]
    fn parse_warning_diagnostic() {
        let params = serde_json::json!({
            "uri": "file:///tmp/test.rs",
            "diagnostics": [{
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 5}
                },
                "severity": 2,
                "message": "unused variable"
            }]
        });
        let diags = parse_diagnostics(&params);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Warning);
        assert!(diags[0].source.is_none());
    }

    #[test]
    fn parse_multiple_diagnostics() {
        let params = serde_json::json!({
            "uri": "file:///tmp/test.py",
            "diagnostics": [
                {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 5}}, "severity": 1, "message": "error1"},
                {"range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 5}}, "severity": 2, "message": "warn1"},
                {"range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 5}}, "severity": 3, "message": "info1"}
            ]
        });
        let diags = parse_diagnostics(&params);
        assert_eq!(diags.len(), 3);
    }

    #[test]
    fn format_empty() {
        assert_eq!(format_diagnostics(&[]), "No diagnostics.");
    }

    #[test]
    fn format_single_diagnostic() {
        let diags = vec![Diagnostic {
            file: "src/main.rs".into(),
            line: 10,
            column: 5,
            severity: DiagnosticSeverity::Error,
            message: "type mismatch".into(),
            source: Some("rust-analyzer".into()),
        }];
        let output = format_diagnostics(&diags);
        assert!(output.contains("1 diagnostic"));
        assert!(output.contains("src/main.rs:10:5"));
        assert!(output.contains("error"));
        assert!(output.contains("type mismatch"));
        assert!(output.contains("[rust-analyzer]"));
    }

    #[test]
    fn uri_to_path_strips_prefix() {
        assert_eq!(uri_to_path("file:///tmp/test.rs"), "/tmp/test.rs");
    }

    #[test]
    fn uri_to_path_no_prefix() {
        assert_eq!(uri_to_path("/tmp/test.rs"), "/tmp/test.rs");
    }

    #[test]
    fn severity_display() {
        assert_eq!(DiagnosticSeverity::Error.to_string(), "error");
        assert_eq!(DiagnosticSeverity::Warning.to_string(), "warning");
        assert_eq!(DiagnosticSeverity::Info.to_string(), "info");
        assert_eq!(DiagnosticSeverity::Hint.to_string(), "hint");
    }
}
