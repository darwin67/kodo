pub mod config;
pub mod loader;
pub mod watcher;

pub use config::{Config, FormatterConfig, LspConfig};
pub use loader::{find_config_file, load_config};
pub use watcher::{ConfigChange, ConfigWatcher};
