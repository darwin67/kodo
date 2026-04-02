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
    /// Create config for Anthropic.
    ///
    /// Anthropic does not offer a public third-party OAuth flow for API access.
    /// Authentication is done exclusively via API keys from console.anthropic.com.
    /// This config is kept as a placeholder; the TUI should direct users to enter
    /// an API key instead of launching a browser OAuth flow.
    pub fn anthropic() -> Self {
        Self {
            provider: "anthropic".to_string(),
            client_id: String::new(),
            auth_url: String::new(),
            token_url: String::new(),
            redirect_uri: String::new(),
            scopes: vec![],
        }
    }

    /// Create config for OpenAI OAuth.
    ///
    /// Uses OpenAI's OIDC endpoint with PKCE (Authorization Code flow).
    /// This is the same flow used by Codex CLI / OpenCode.
    /// Users authenticate via their OpenAI account in the browser;
    /// the callback server on localhost receives the auth code.
    pub fn openai() -> Self {
        Self {
            provider: "openai".to_string(),
            client_id: std::env::var("OPENAI_CLIENT_ID")
                .unwrap_or_else(|_| "app_EMoamEEZ73f0CkXaXp7hrann".to_string()),
            auth_url: "https://auth.openai.com/oauth/authorize".to_string(),
            token_url: "https://auth.openai.com/oauth/token".to_string(),
            redirect_uri: "http://localhost:8899/auth/callback".to_string(),
            scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
                "offline_access".to_string(),
            ],
        }
    }

    /// Check if this provider supports OAuth (has valid endpoints configured)
    pub fn supports_oauth(&self) -> bool {
        !self.auth_url.is_empty() && !self.token_url.is_empty()
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
