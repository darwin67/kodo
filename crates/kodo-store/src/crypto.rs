use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, Result};
use tracing::debug;

const SERVICE_NAME: &str = "kodo";

/// Trait for secret storage backends.
pub trait SecretStore: Send + Sync {
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// OS keychain backend using the `keyring` crate.
pub struct KeychainStore;

impl SecretStore for KeychainStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        let entry =
            keyring::Entry::new(SERVICE_NAME, key).context("failed to create keychain entry")?;
        entry
            .set_password(value)
            .context("failed to store secret in keychain")?;
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        let entry =
            keyring::Entry::new(SERVICE_NAME, key).context("failed to create keychain entry")?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("failed to read from keychain: {e}")),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        let entry =
            keyring::Entry::new(SERVICE_NAME, key).context("failed to create keychain entry")?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("failed to delete from keychain: {e}")),
        }
    }
}

/// In-memory secret store for testing.
pub struct MemoryStore {
    data: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for MemoryStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.data
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.data.lock().unwrap().get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.data.lock().unwrap().remove(key);
        Ok(())
    }
}

/// Store a secret using the given backend.
pub fn set_secret(store: &dyn SecretStore, provider: &str, field: &str, value: &str) -> Result<()> {
    let key = format!("{provider}:{field}");
    debug!(key = %key, "storing secret");
    store.set(&key, value)
}

/// Retrieve a secret using the given backend.
pub fn get_secret(store: &dyn SecretStore, provider: &str, field: &str) -> Result<Option<String>> {
    let key = format!("{provider}:{field}");
    store.get(&key)
}

/// Delete a secret using the given backend.
pub fn delete_secret(store: &dyn SecretStore, provider: &str, field: &str) -> Result<()> {
    let key = format!("{provider}:{field}");
    debug!(key = %key, "deleting secret");
    store.delete(&key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> MemoryStore {
        MemoryStore::new()
    }

    #[test]
    fn set_and_get_secret_roundtrip() {
        let s = store();
        set_secret(&s, "anthropic", "token", "sk-test-12345").unwrap();
        let value = get_secret(&s, "anthropic", "token").unwrap();
        assert_eq!(value, Some("sk-test-12345".into()));
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let s = store();
        let value = get_secret(&s, "nope", "token").unwrap();
        assert!(value.is_none());
    }

    #[test]
    fn delete_existing_secret() {
        let s = store();
        set_secret(&s, "openai", "token", "to-delete").unwrap();
        delete_secret(&s, "openai", "token").unwrap();
        assert!(get_secret(&s, "openai", "token").unwrap().is_none());
    }

    #[test]
    fn delete_nonexistent_succeeds() {
        let s = store();
        delete_secret(&s, "nope", "nothing").unwrap();
    }

    #[test]
    fn overwrite_secret() {
        let s = store();
        set_secret(&s, "openai", "token", "first").unwrap();
        set_secret(&s, "openai", "token", "second").unwrap();
        let value = get_secret(&s, "openai", "token").unwrap();
        assert_eq!(value, Some("second".into()));
    }

    #[test]
    fn separate_fields_are_independent() {
        let s = store();
        set_secret(&s, "google", "token", "token-val").unwrap();
        set_secret(&s, "google", "refresh", "refresh-val").unwrap();

        assert_eq!(
            get_secret(&s, "google", "token").unwrap(),
            Some("token-val".into())
        );
        assert_eq!(
            get_secret(&s, "google", "refresh").unwrap(),
            Some("refresh-val".into())
        );
    }
}
