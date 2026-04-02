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

/// The type of OAuth callback flow
#[derive(Debug, Clone, PartialEq)]
pub enum OAuthFlowType {
    /// Auto-redirect: starts a localhost callback server,
    /// browser redirects back automatically after auth.
    /// Used by OpenAI.
    AutoRedirect,
    /// Code-paste: browser shows a code after auth,
    /// user copies it and pastes it back into the TUI.
    /// Used by Anthropic (via claude.ai).
    CodePaste,
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
    /// OAuth redirect URI
    pub redirect_uri: String,
    /// Required OAuth scopes
    pub scopes: Vec<String>,
    /// The type of OAuth callback flow
    pub flow_type: OAuthFlowType,
}

impl AuthConfig {
    /// Create config for Anthropic OAuth.
    ///
    /// Uses claude.ai OAuth with PKCE (code-paste flow).
    /// The user authenticates in the browser, then the browser
    /// displays an authorization code that the user pastes back
    /// into kodo. The code is exchanged for an API key.
    pub fn anthropic() -> Self {
        Self {
            provider: "anthropic".to_string(),
            client_id: std::env::var("ANTHROPIC_CLIENT_ID")
                .unwrap_or_else(|_| "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_string()),
            auth_url: "https://claude.ai/oauth/authorize".to_string(),
            token_url: "https://claude.ai/oauth/token".to_string(),
            redirect_uri: "https://console.anthropic.com/oauth/code/callback".to_string(),
            scopes: vec![
                "org:create_api_key".to_string(),
                "user:profile".to_string(),
                "user:inference".to_string(),
            ],
            flow_type: OAuthFlowType::CodePaste,
        }
    }

    /// Create config for OpenAI OAuth.
    ///
    /// Uses OpenAI's OIDC endpoint with PKCE (auto-redirect flow).
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
            flow_type: OAuthFlowType::AutoRedirect,
        }
    }

    /// Check if this provider supports OAuth (has valid endpoints configured)
    pub fn supports_oauth(&self) -> bool {
        !self.auth_url.is_empty() && !self.token_url.is_empty()
    }

    /// Check if this is a code-paste flow (user pastes code from browser)
    pub fn is_code_paste(&self) -> bool {
        self.flow_type == OAuthFlowType::CodePaste
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
