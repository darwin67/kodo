use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use tracing::{debug, info};

use crate::config::Config;

/// Config file names to search for
const CONFIG_NAMES: &[&str] = &[
    "kodo.toml",
    "kodo.yaml",
    "kodo.yml",
    ".kodo.toml",
    ".kodo.yaml",
    ".kodo.yml",
];

/// Load configuration from a specific file
pub fn load_config<P: AsRef<Path>>(path: P) -> Result<Config> {
    let path = path.as_ref();

    if !path.exists() {
        anyhow::bail!("Config file not found: {}", path.display());
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;

    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");

    let config = match extension {
        "toml" => toml::from_str(&content)
            .with_context(|| format!("Failed to parse TOML config: {}", path.display()))?,
        "yaml" | "yml" => serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse YAML config: {}", path.display()))?,
        _ => anyhow::bail!("Unsupported config format: {}", extension),
    };

    info!("Loaded config from: {}", path.display());
    Ok(config)
}

/// Find config file in the current directory or parent directories
pub fn find_config_file() -> Option<PathBuf> {
    find_config_in_dir(&std::env::current_dir().ok()?)
}

/// Find config file starting from a specific directory
pub fn find_config_in_dir(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();

    loop {
        // Check each config name in current directory
        for name in CONFIG_NAMES {
            let path = current.join(name);
            if path.exists() {
                debug!("Found config file: {}", path.display());
                return Some(path);
            }
        }

        // Move to parent directory
        if !current.pop() {
            break;
        }
    }

    // Check user config directory
    if let Some(config_dir) = dirs::config_dir() {
        let kodo_dir = config_dir.join("kodo");
        for name in CONFIG_NAMES {
            let path = kodo_dir.join(name);
            if path.exists() {
                debug!("Found config in user dir: {}", path.display());
                return Some(path);
            }
        }
    }

    None
}

/// Load config from default locations or return default config
pub fn load_or_default() -> Config {
    if let Some(path) = find_config_file() {
        match load_config(&path) {
            Ok(config) => {
                info!("Using config from: {}", path.display());
                return config;
            }
            Err(e) => {
                eprintln!(
                    "Warning: Failed to load config from {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    info!("Using default configuration");
    Config::default()
}

/// Merge two configs, with the second one taking precedence
pub fn merge_configs(_base: Config, override_config: Config) -> Config {
    // For now, just return the override config
    // TODO: Implement proper deep merging
    override_config
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_toml_config() {
        let mut file = NamedTempFile::with_suffix(".toml").unwrap();
        writeln!(
            file,
            r#"
[general]
default-provider = "openai"
default-model = "gpt-4"
debug = true
"#
        )
        .unwrap();

        let config = load_config(file.path()).unwrap();
        assert_eq!(config.general.default_provider, "openai");
        assert_eq!(config.general.default_model, "gpt-4");
        assert!(config.general.debug);
    }

    #[test]
    fn test_load_yaml_config() {
        let mut file = NamedTempFile::with_suffix(".yaml").unwrap();
        writeln!(
            file,
            r#"
general:
  default-provider: gemini
  default-model: gemini-1.5-pro
  max-subagents: 10
"#
        )
        .unwrap();

        let config = load_config(file.path()).unwrap();
        assert_eq!(config.general.default_provider, "gemini");
        assert_eq!(config.general.default_model, "gemini-1.5-pro");
        assert_eq!(config.general.max_subagents, 10);
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.general.default_provider, "anthropic");
        assert_eq!(config.general.default_mode, "build");
        assert!(!config.general.debug);
    }
}
