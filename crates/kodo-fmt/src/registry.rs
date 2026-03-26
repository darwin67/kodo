use std::collections::HashMap;
use std::path::Path;

/// Configuration for a single formatter.
#[derive(Debug, Clone)]
pub struct FormatterConfig {
    /// Display name.
    pub name: String,
    /// Command to run. `$FILE` will be replaced with the file path.
    pub command: Vec<String>,
    /// File extensions this formatter handles (e.g. ["rs"]).
    pub extensions: Vec<String>,
}

/// Registry of formatters, mapped by file extension.
pub struct FormatterRegistry {
    /// Extension (without dot) -> formatter config.
    formatters: HashMap<String, FormatterConfig>,
}

impl FormatterRegistry {
    pub fn new() -> Self {
        Self {
            formatters: HashMap::new(),
        }
    }

    /// Create a registry with all built-in formatters pre-registered.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register_builtins();
        registry
    }

    /// Register a formatter for its extensions.
    pub fn register(&mut self, config: FormatterConfig) {
        for ext in &config.extensions {
            self.formatters.insert(ext.clone(), config.clone());
        }
    }

    /// Look up the formatter for a given file path.
    pub fn formatter_for(&self, path: &Path) -> Option<&FormatterConfig> {
        let ext = path.extension()?.to_str()?;
        self.formatters.get(ext)
    }

    /// Register all built-in formatters.
    fn register_builtins(&mut self) {
        // Rust: cargo fmt
        self.register(FormatterConfig {
            name: "cargo fmt".into(),
            command: vec![
                "rustfmt".into(),
                "--edition".into(),
                "2024".into(),
                "$FILE".into(),
            ],
            extensions: vec!["rs".into()],
        });

        // Go: gofmt
        self.register(FormatterConfig {
            name: "gofmt".into(),
            command: vec!["gofmt".into(), "-w".into(), "$FILE".into()],
            extensions: vec!["go".into()],
        });

        // JavaScript/TypeScript: prettier
        self.register(FormatterConfig {
            name: "prettier".into(),
            command: vec!["prettier".into(), "--write".into(), "$FILE".into()],
            extensions: vec![
                "js".into(),
                "jsx".into(),
                "ts".into(),
                "tsx".into(),
                "css".into(),
                "html".into(),
                "json".into(),
                "md".into(),
                "yaml".into(),
                "yml".into(),
            ],
        });

        // Python: ruff
        self.register(FormatterConfig {
            name: "ruff".into(),
            command: vec!["ruff".into(), "format".into(), "$FILE".into()],
            extensions: vec!["py".into(), "pyi".into()],
        });
    }

    /// Number of registered extension mappings.
    pub fn len(&self) -> usize {
        self.formatters.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.formatters.is_empty()
    }
}

impl Default for FormatterRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builtins_registered() {
        let registry = FormatterRegistry::with_builtins();
        assert!(!registry.is_empty());
    }

    #[test]
    fn lookup_rust() {
        let registry = FormatterRegistry::with_builtins();
        let fmt = registry.formatter_for(&PathBuf::from("src/main.rs"));
        assert!(fmt.is_some());
        assert_eq!(fmt.unwrap().name, "cargo fmt");
    }

    #[test]
    fn lookup_go() {
        let registry = FormatterRegistry::with_builtins();
        let fmt = registry.formatter_for(&PathBuf::from("main.go"));
        assert!(fmt.is_some());
        assert_eq!(fmt.unwrap().name, "gofmt");
    }

    #[test]
    fn lookup_typescript() {
        let registry = FormatterRegistry::with_builtins();
        let fmt = registry.formatter_for(&PathBuf::from("app.tsx"));
        assert!(fmt.is_some());
        assert_eq!(fmt.unwrap().name, "prettier");
    }

    #[test]
    fn lookup_python() {
        let registry = FormatterRegistry::with_builtins();
        let fmt = registry.formatter_for(&PathBuf::from("script.py"));
        assert!(fmt.is_some());
        assert_eq!(fmt.unwrap().name, "ruff");
    }

    #[test]
    fn lookup_unknown_extension() {
        let registry = FormatterRegistry::with_builtins();
        let fmt = registry.formatter_for(&PathBuf::from("file.xyz"));
        assert!(fmt.is_none());
    }

    #[test]
    fn lookup_no_extension() {
        let registry = FormatterRegistry::with_builtins();
        let fmt = registry.formatter_for(&PathBuf::from("Makefile"));
        assert!(fmt.is_none());
    }

    #[test]
    fn empty_registry() {
        let registry = FormatterRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn custom_formatter() {
        let mut registry = FormatterRegistry::new();
        registry.register(FormatterConfig {
            name: "custom".into(),
            command: vec!["my-fmt".into(), "$FILE".into()],
            extensions: vec!["custom".into()],
        });

        let fmt = registry.formatter_for(&PathBuf::from("test.custom"));
        assert!(fmt.is_some());
        assert_eq!(fmt.unwrap().name, "custom");
    }
}
