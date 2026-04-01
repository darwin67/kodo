pub mod oauth;
pub mod storage;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Authentication token information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    /// Provider name (e.g., "anthropic", "openai")
    pub provider: String,
    /// Access token
    pub access_token: String,
    /// Refresh token (if available)
    pub refresh_token: Option<String>,
    /// Token expiration time (if available)
    pub expires_at: Option<i64>,
}

/// Authentication configuration for a provider
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Provider name
    pub provider: String,
    /// OAuth client ID
    pub client_id: String,
    /// OAuth authorization URL
    pub auth_url: String,
    /// OAuth token URL
    pub token_url: String,
    /// OAuth redirect URI (local server)
    pub redirect_uri: String,
    /// Required OAuth scopes
    pub scopes: Vec<String>,
}

impl AuthConfig {
    /// Create config for Anthropic OAuth
    pub fn anthropic() -> Self {
        Self {
            provider: "anthropic".to_string(),
            client_id: std::env::var("ANTHROPIC_CLIENT_ID")
                .unwrap_or_else(|_| "kodo-cli".to_string()),
            auth_url: "https://console.anthropic.com/oauth/authorize".to_string(),
            token_url: "https://console.anthropic.com/oauth/token".to_string(),
            redirect_uri: "http://localhost:8899/callback".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        }
    }

    /// Create config for OpenAI OAuth
    pub fn openai() -> Self {
        Self {
            provider: "openai".to_string(),
            client_id: std::env::var("OPENAI_CLIENT_ID").unwrap_or_else(|_| "kodo-cli".to_string()),
            auth_url: "https://platform.openai.com/oauth/authorize".to_string(),
            token_url: "https://platform.openai.com/oauth/token".to_string(),
            redirect_uri: "http://localhost:8899/callback".to_string(),
            scopes: vec!["api.read".to_string(), "api.write".to_string()],
        }
    }
}

/// Trait for authentication providers
#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    /// Perform OAuth login flow
    async fn login(&self) -> Result<AuthToken>;

    /// Refresh an existing token
    async fn refresh(&self, token: &AuthToken) -> Result<AuthToken>;

    /// Validate a token
    async fn validate(&self, token: &AuthToken) -> Result<bool>;
}
