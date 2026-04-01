use crate::AuthToken;
use anyhow::{Context, Result};

/// Secure storage for authentication tokens
pub struct TokenStorage {
    service_name: String,
}

impl TokenStorage {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }

    /// Store a token securely
    pub async fn store(&self, token: &AuthToken) -> Result<()> {
        let key = format!("{}-{}", self.service_name, token.provider);
        let value = serde_json::to_string(token)?;

        #[cfg(target_os = "macos")]
        {
            self.store_macos(&key, &value)?;
        }

        #[cfg(target_os = "linux")]
        {
            self.store_linux(&key, &value)?;
        }

        #[cfg(target_os = "windows")]
        {
            self.store_windows(&key, &value)?;
        }

        Ok(())
    }

    /// Retrieve a token
    pub async fn get(&self, provider: &str) -> Result<Option<AuthToken>> {
        let key = format!("{}-{}", self.service_name, provider);

        #[cfg(target_os = "macos")]
        let value = self.get_macos(&key)?;

        #[cfg(target_os = "linux")]
        let value = self.get_linux(&key)?;

        #[cfg(target_os = "windows")]
        let value = self.get_windows(&key)?;

        match value {
            Some(json) => {
                let token = serde_json::from_str(&json).context("Failed to deserialize token")?;
                Ok(Some(token))
            }
            None => Ok(None),
        }
    }

    /// Delete a token
    pub async fn delete(&self, provider: &str) -> Result<()> {
        let key = format!("{}-{}", self.service_name, provider);

        #[cfg(target_os = "macos")]
        self.delete_macos(&key)?;

        #[cfg(target_os = "linux")]
        self.delete_linux(&key)?;

        #[cfg(target_os = "windows")]
        self.delete_windows(&key)?;

        Ok(())
    }
}

// macOS implementation using Keychain
#[cfg(target_os = "macos")]
impl TokenStorage {
    fn store_macos(&self, key: &str, value: &str) -> Result<()> {
        use security_framework::passwords::{delete_generic_password, set_generic_password};

        // Delete existing entry if any
        let _ = delete_generic_password(&self.service_name, key);

        set_generic_password(&self.service_name, key, value.as_bytes())
            .context("Failed to store token in macOS Keychain")?;

        Ok(())
    }

    fn get_macos(&self, key: &str) -> Result<Option<String>> {
        use security_framework::passwords::get_generic_password;

        match get_generic_password(&self.service_name, key) {
            Ok(password) => {
                let value = String::from_utf8(password).context("Invalid UTF-8 in stored token")?;
                Ok(Some(value))
            }
            Err(_) => Ok(None),
        }
    }

    fn delete_macos(&self, key: &str) -> Result<()> {
        use security_framework::passwords::delete_generic_password;

        // Ignore errors as the entry might not exist
        let _ = delete_generic_password(&self.service_name, key);
        Ok(())
    }
}

// Linux implementation using keyring
#[cfg(target_os = "linux")]
impl TokenStorage {
    fn store_linux(&self, key: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key)?;
        entry
            .set_password(value)
            .context("Failed to store token in Linux keyring")?;
        Ok(())
    }

    fn get_linux(&self, key: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(&self.service_name, key)?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e).context("Failed to retrieve token from Linux keyring"),
        }
    }

    fn delete_linux(&self, key: &str) -> Result<()> {
        let entry = keyring::Entry::new(&self.service_name, key)?;
        match entry.delete_credential() {
            Ok(_) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already deleted
            Err(e) => Err(e).context("Failed to delete token from Linux keyring"),
        }
    }
}

// Windows implementation (simplified - should use Windows Credential Manager)
#[cfg(target_os = "windows")]
impl TokenStorage {
    fn store_windows(&self, key: &str, value: &str) -> Result<()> {
        // For now, use environment variable as fallback
        // TODO: Implement proper Windows Credential Manager support
        std::env::set_var(format!("KODO_TOKEN_{}", key.to_uppercase()), value);
        Ok(())
    }

    fn get_windows(&self, key: &str) -> Result<Option<String>> {
        Ok(std::env::var(format!("KODO_TOKEN_{}", key.to_uppercase())).ok())
    }

    fn delete_windows(&self, key: &str) -> Result<()> {
        std::env::remove_var(format!("KODO_TOKEN_{}", key.to_uppercase()));
        Ok(())
    }
}
