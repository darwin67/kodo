use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info};

use crate::config::Config;
use crate::loader::load_config;

/// Configuration change event
#[derive(Debug, Clone)]
pub enum ConfigChange {
    /// Config file was modified
    Modified(Box<Config>),
    /// Config file was removed
    Removed,
    /// Error loading config
    Error(String),
}

/// Watches configuration file for changes
pub struct ConfigWatcher {
    config_path: PathBuf,
    current_config: Arc<RwLock<Config>>,
    tx: mpsc::UnboundedSender<ConfigChange>,
    _watcher: RecommendedWatcher,
}

impl ConfigWatcher {
    /// Create a new config watcher
    pub fn new<P: AsRef<Path>>(
        config_path: P,
        initial_config: Config,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ConfigChange>)> {
        let config_path = config_path.as_ref().to_path_buf();
        let current_config = Arc::new(RwLock::new(initial_config));
        let (tx, rx) = mpsc::unbounded_channel();

        let tx_clone = tx.clone();
        let config_path_clone = config_path.clone();
        let current_config_clone = current_config.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    if matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                    ) {
                        handle_event(event, &config_path_clone, &current_config_clone, &tx_clone);
                    }
                }
                Err(e) => error!("Watch error: {:?}", e),
            },
            NotifyConfig::default()
                .with_poll_interval(Duration::from_secs(1))
                .with_compare_contents(true),
        )?;

        // Watch the config file
        watcher.watch(&config_path, RecursiveMode::NonRecursive)?;

        info!("Watching config file: {}", config_path.display());

        Ok((
            Self {
                config_path,
                current_config,
                tx,
                _watcher: watcher,
            },
            rx,
        ))
    }

    /// Get the current configuration
    pub async fn current(&self) -> Config {
        self.current_config.read().await.clone()
    }

    /// Reload configuration manually
    pub async fn reload(&self) -> Result<Config> {
        match load_config(&self.config_path) {
            Ok(new_config) => {
                *self.current_config.write().await = new_config.clone();
                let _ = self
                    .tx
                    .send(ConfigChange::Modified(Box::new(new_config.clone())));
                Ok(new_config)
            }
            Err(e) => {
                let _ = self.tx.send(ConfigChange::Error(e.to_string()));
                Err(e)
            }
        }
    }
}

fn handle_event(
    event: Event,
    config_path: &Path,
    current_config: &Arc<RwLock<Config>>,
    tx: &mpsc::UnboundedSender<ConfigChange>,
) {
    // Check if the event is for our config file
    if !event.paths.iter().any(|p| p == config_path) {
        return;
    }

    debug!("Config file event: {:?}", event.kind);

    match event.kind {
        EventKind::Remove(_) => {
            let _ = tx.send(ConfigChange::Removed);
        }
        EventKind::Modify(_) | EventKind::Create(_) => {
            // Small delay to ensure file write is complete
            std::thread::sleep(Duration::from_millis(100));

            match load_config(config_path) {
                Ok(new_config) => {
                    // Update current config
                    let rt = tokio::runtime::Handle::try_current();
                    if let Ok(handle) = rt {
                        handle.block_on(async {
                            *current_config.write().await = new_config.clone();
                        });
                    }

                    info!("Config reloaded from: {}", config_path.display());
                    let _ = tx.send(ConfigChange::Modified(Box::new(new_config)));
                }
                Err(e) => {
                    error!("Failed to reload config: {}", e);
                    let _ = tx.send(ConfigChange::Error(e.to_string()));
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_config_watcher_creation() {
        let file = NamedTempFile::with_suffix(".toml").unwrap();
        let config = Config::default();

        let (watcher, mut rx) = ConfigWatcher::new(file.path(), config.clone()).unwrap();

        // Check initial config
        let current = watcher.current().await;
        assert_eq!(
            current.general.default_provider,
            config.general.default_provider
        );

        // No events should be pending
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_manual_reload() {
        let mut file = NamedTempFile::with_suffix(".toml").unwrap();
        writeln!(
            file,
            r#"
[general]
default-provider = "openai"
"#
        )
        .unwrap();
        file.flush().unwrap();

        let config = Config::default();
        let (watcher, mut rx) = ConfigWatcher::new(file.path(), config).unwrap();

        // Manually reload
        let new_config = watcher.reload().await.unwrap();
        assert_eq!(new_config.general.default_provider, "openai");

        // Should receive change event
        match rx.recv().await {
            Some(ConfigChange::Modified(config)) => {
                assert_eq!(config.general.default_provider, "openai");
            }
            _ => panic!("Expected Modified event"),
        }
    }
}
