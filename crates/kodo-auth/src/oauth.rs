use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Query, State},
    response::{Html, Redirect},
    routing::get,
};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, oneshot};
use tower_http::cors::CorsLayer;
use url::Url;

use crate::{AuthConfig, AuthProvider, AuthToken};

/// OAuth authorization code response
#[derive(Debug, Deserialize)]
struct AuthCallback {
    code: String,
    state: Option<String>,
}

/// OAuth token response
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

/// OAuth error response
#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    error_description: Option<String>,
}

/// Shared state for OAuth callback
struct CallbackState {
    sender: Option<oneshot::Sender<Result<String>>>,
    expected_state: String,
}

/// OAuth2 authentication provider
pub struct OAuthProvider {
    config: AuthConfig,
    http_client: reqwest::Client,
}

impl OAuthProvider {
    pub fn new(config: AuthConfig) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
        }
    }

    /// Generate PKCE challenge
    fn generate_pkce() -> (String, String) {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

        let mut verifier_bytes = [0u8; 32];
        rand::thread_rng().fill(&mut verifier_bytes);
        let verifier = URL_SAFE_NO_PAD.encode(&verifier_bytes);

        let mut hasher = Sha256::new();
        hasher.update(&verifier);
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        (verifier, challenge)
    }

    /// Generate random state for CSRF protection
    fn generate_state() -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

        let mut state_bytes = [0u8; 16];
        rand::thread_rng().fill(&mut state_bytes);
        URL_SAFE_NO_PAD.encode(&state_bytes)
    }
}

#[async_trait::async_trait]
impl AuthProvider for OAuthProvider {
    async fn login(&self) -> Result<AuthToken> {
        let state = Self::generate_state();
        let (verifier, challenge) = Self::generate_pkce();

        // Build authorization URL
        let mut auth_url = Url::parse(&self.config.auth_url)?;
        auth_url
            .query_pairs_mut()
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &self.config.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");

        if !self.config.scopes.is_empty() {
            auth_url
                .query_pairs_mut()
                .append_pair("scope", &self.config.scopes.join(" "));
        }

        // Create callback channel
        let (tx, rx) = oneshot::channel();
        let callback_state = Arc::new(Mutex::new(CallbackState {
            sender: Some(tx),
            expected_state: state.clone(),
        }));

        // Start local server for callback
        let app = Router::new()
            .route("/callback", get(handle_callback))
            .route("/success", get(handle_success))
            .route("/error", get(handle_error))
            .layer(CorsLayer::permissive())
            .with_state(callback_state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:8899")
            .await
            .context("Failed to bind OAuth callback server")?;

        // Spawn server in background
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Open browser
        println!("Opening browser for authentication...");
        webbrowser::open(&auth_url.to_string())?;

        // Wait for callback with timeout
        let code = tokio::time::timeout(Duration::from_secs(300), rx)
            .await
            .context("OAuth callback timeout")?
            .context("OAuth callback channel closed")??;

        // Shutdown server
        server_handle.abort();

        // Exchange code for token
        let token_params = [
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &self.config.client_id),
            ("redirect_uri", &self.config.redirect_uri),
            ("code_verifier", &verifier),
        ];

        let response = self
            .http_client
            .post(&self.config.token_url)
            .form(&token_params)
            .send()
            .await
            .context("Failed to exchange authorization code")?;

        if !response.status().is_success() {
            let error: OAuthError = response.json().await?;
            anyhow::bail!(
                "OAuth token exchange failed: {} - {}",
                error.error,
                error.error_description.unwrap_or_default()
            );
        }

        let token_response: TokenResponse = response.json().await?;

        Ok(AuthToken {
            provider: self.config.provider.clone(),
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            expires_at: token_response.expires_in.map(|exp| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
                    + exp
            }),
        })
    }

    async fn refresh(&self, token: &AuthToken) -> Result<AuthToken> {
        let refresh_token = token
            .refresh_token
            .as_ref()
            .context("No refresh token available")?;

        let token_params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &self.config.client_id),
        ];

        let response = self
            .http_client
            .post(&self.config.token_url)
            .form(&token_params)
            .send()
            .await
            .context("Failed to refresh token")?;

        if !response.status().is_success() {
            let error: OAuthError = response.json().await?;
            anyhow::bail!(
                "OAuth token refresh failed: {} - {}",
                error.error,
                error.error_description.unwrap_or_default()
            );
        }

        let token_response: TokenResponse = response.json().await?;

        Ok(AuthToken {
            provider: self.config.provider.clone(),
            access_token: token_response.access_token,
            refresh_token: token_response
                .refresh_token
                .or_else(|| token.refresh_token.clone()),
            expires_at: token_response.expires_in.map(|exp| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
                    + exp
            }),
        })
    }

    async fn validate(&self, token: &AuthToken) -> Result<bool> {
        // Check expiration if available
        if let Some(expires_at) = token.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            if now >= expires_at {
                return Ok(false);
            }
        }

        // Provider-specific validation could be added here
        // For now, assume token is valid if not expired
        Ok(true)
    }
}

// OAuth callback handlers
async fn handle_callback(
    Query(params): Query<AuthCallback>,
    State(state): State<Arc<Mutex<CallbackState>>>,
) -> Result<Redirect, Redirect> {
    let mut state_lock = state.lock().await;

    // Verify state parameter
    if params.state.as_ref() != Some(&state_lock.expected_state) {
        if let Some(sender) = state_lock.sender.take() {
            let _ = sender.send(Err(anyhow::anyhow!("Invalid state parameter")));
        }
        return Err(Redirect::to("/error"));
    }

    // Send code to waiting task
    if let Some(sender) = state_lock.sender.take() {
        let _ = sender.send(Ok(params.code));
    }

    Ok(Redirect::to("/success"))
}

async fn handle_success() -> Html<&'static str> {
    Html(
        r#"
        <!DOCTYPE html>
        <html>
        <head>
            <title>Authentication Successful</title>
            <style>
                body { font-family: system-ui, sans-serif; text-align: center; padding: 50px; }
                .success { color: #10b981; font-size: 48px; }
            </style>
        </head>
        <body>
            <div class="success">✓</div>
            <h1>Authentication Successful!</h1>
            <p>You can close this window and return to Kodo.</p>
            <script>setTimeout(() => window.close(), 3000);</script>
        </body>
        </html>
    "#,
    )
}

async fn handle_error() -> Html<&'static str> {
    Html(
        r#"
        <!DOCTYPE html>
        <html>
        <head>
            <title>Authentication Failed</title>
            <style>
                body { font-family: system-ui, sans-serif; text-align: center; padding: 50px; }
                .error { color: #ef4444; font-size: 48px; }
            </style>
        </head>
        <body>
            <div class="error">✗</div>
            <h1>Authentication Failed</h1>
            <p>Please try again or check your credentials.</p>
            <script>setTimeout(() => window.close(), 3000);</script>
        </body>
        </html>
    "#,
    )
}
