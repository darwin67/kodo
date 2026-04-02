use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    extract::{Query, State},
    response::{Html, Redirect},
    routing::get,
    Router,
};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::{oneshot, Mutex};
use tower_http::cors::CorsLayer;
use url::Url;

use crate::{AuthConfig, AuthProvider, AuthToken, OAuthFlowType};

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

/// Holds the PKCE verifier and state for a pending code-paste OAuth flow.
/// The TUI shows the URL to the user, they paste the code back,
/// and then we call `exchange_code` with that code.
pub struct PendingCodePaste {
    /// The URL to open in the browser
    pub auth_url: String,
    /// PKCE code verifier (needed for token exchange)
    verifier: String,
    /// CSRF state parameter
    _state: String,
    /// The auth config (for token exchange)
    config: AuthConfig,
    /// HTTP client
    http_client: reqwest::Client,
}

impl PendingCodePaste {
    /// Exchange the user-pasted authorization code for a token.
    pub async fn exchange_code(&self, code: &str) -> Result<AuthToken> {
        let token_params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", self.config.client_id.as_str()),
            ("redirect_uri", self.config.redirect_uri.as_str()),
            ("code_verifier", self.verifier.as_str()),
        ];

        let response = self
            .http_client
            .post(&self.config.token_url)
            .form(&token_params)
            .send()
            .await
            .context("Failed to exchange authorization code")?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "(failed to read response body)".to_string());
            tracing::error!(
                "Token exchange failed: status={}, url={}, body={}",
                status,
                self.config.token_url,
                body
            );

            // Try to parse as structured error, fall back to raw body
            if let Ok(error) = serde_json::from_str::<OAuthError>(&body) {
                anyhow::bail!(
                    "OAuth token exchange failed ({}): {} - {}",
                    status,
                    error.error,
                    error.error_description.unwrap_or_default()
                );
            } else {
                anyhow::bail!("OAuth token exchange failed ({}): {}", status, body);
            }
        }

        let body = response
            .text()
            .await
            .context("Failed to read token response body")?;
        tracing::debug!("Token response body: {}", body);

        let token_response: TokenResponse = serde_json::from_str(&body)
            .context(format!("Failed to parse token response: {}", body))?;

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
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

        let mut rng = rand::thread_rng();
        let mut verifier_bytes = [0u8; 32];
        rng.fill_bytes(&mut verifier_bytes);
        let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);

        let mut hasher = Sha256::new();
        hasher.update(&verifier);
        let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

        (verifier, challenge)
    }

    /// Generate random state for CSRF protection
    fn generate_state() -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

        let mut rng = rand::thread_rng();
        let mut state_bytes = [0u8; 32];
        rng.fill_bytes(&mut state_bytes);
        URL_SAFE_NO_PAD.encode(state_bytes)
    }

    /// Build the authorization URL with PKCE parameters.
    /// Returns (url, verifier, state).
    fn build_auth_url(&self) -> Result<(Url, String, String)> {
        let state = Self::generate_state();
        let (verifier, challenge) = Self::generate_pkce();

        let mut auth_url = Url::parse(&self.config.auth_url)?;
        auth_url
            .query_pairs_mut()
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &self.config.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");

        // For code-paste flows, add code=true to indicate the server
        // should show the code to the user instead of redirecting
        if self.config.flow_type == OAuthFlowType::CodePaste {
            auth_url.query_pairs_mut().append_pair("code", "true");
        }

        if !self.config.scopes.is_empty() {
            auth_url
                .query_pairs_mut()
                .append_pair("scope", &self.config.scopes.join(" "));
        }

        Ok((auth_url, verifier, state))
    }

    /// Start a code-paste OAuth flow.
    ///
    /// Returns a `PendingCodePaste` that contains the URL for the user
    /// to open in their browser, and a method to exchange the code once
    /// the user pastes it back.
    pub fn start_code_paste_flow(&self) -> Result<PendingCodePaste> {
        let (auth_url, verifier, state) = self.build_auth_url()?;

        Ok(PendingCodePaste {
            auth_url: auth_url.to_string(),
            verifier,
            _state: state,
            config: self.config.clone(),
            http_client: self.http_client.clone(),
        })
    }

    /// Exchange code for token (shared logic)
    async fn exchange_code(&self, code: &str, verifier: &str) -> Result<AuthToken> {
        let token_params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", self.config.client_id.as_str()),
            ("redirect_uri", self.config.redirect_uri.as_str()),
            ("code_verifier", verifier),
        ];

        tracing::debug!(
            "Exchanging code: token_url={}, client_id={}, redirect_uri={}, code_len={}, verifier_len={}",
            self.config.token_url,
            self.config.client_id,
            self.config.redirect_uri,
            code.len(),
            verifier.len()
        );

        let response = self
            .http_client
            .post(&self.config.token_url)
            .form(&token_params)
            .send()
            .await
            .context("Failed to exchange authorization code")?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "(failed to read response body)".to_string());
            tracing::error!(
                "Token exchange failed: status={}, url={}, body={}",
                status,
                self.config.token_url,
                body
            );

            if let Ok(error) = serde_json::from_str::<OAuthError>(&body) {
                anyhow::bail!(
                    "OAuth token exchange failed ({}): {} - {}",
                    status,
                    error.error,
                    error.error_description.unwrap_or_default()
                );
            } else {
                anyhow::bail!("OAuth token exchange failed ({}): {}", status, body);
            }
        }

        let body = response
            .text()
            .await
            .context("Failed to read token response body")?;
        tracing::debug!("Token response body: {}", body);

        let token_response: TokenResponse = serde_json::from_str(&body)
            .context(format!("Failed to parse token response: {}", body))?;

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

    /// Auto-redirect login: starts localhost callback server, opens browser,
    /// waits for the redirect callback with the auth code.
    async fn login_auto_redirect(&self) -> Result<AuthToken> {
        let (auth_url, verifier, state) = self.build_auth_url()?;

        // Create callback channel
        let (tx, rx) = oneshot::channel();
        let callback_state = Arc::new(Mutex::new(CallbackState {
            sender: Some(tx),
            expected_state: state,
        }));

        // Start local server for callback
        let app = Router::new()
            .route("/callback", get(handle_callback))
            .route("/auth/callback", get(handle_callback))
            .route("/success", get(handle_success))
            .route("/error", get(handle_error))
            .layer(CorsLayer::permissive())
            .with_state(callback_state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:8899")
            .await
            .context("Failed to bind OAuth callback server")?;

        let server_handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Open browser
        webbrowser::open(auth_url.as_ref())?;

        // Wait for callback with timeout
        let code = tokio::time::timeout(Duration::from_secs(300), rx)
            .await
            .context("OAuth callback timeout")?
            .context("OAuth callback channel closed")??;

        server_handle.abort();

        // Exchange code for token
        self.exchange_code(&code, &verifier).await
    }
}

#[async_trait::async_trait]
impl AuthProvider for OAuthProvider {
    async fn login(&self) -> Result<AuthToken> {
        match self.config.flow_type {
            OAuthFlowType::AutoRedirect => self.login_auto_redirect().await,
            OAuthFlowType::CodePaste => {
                // For CLI (non-TUI) usage: start code-paste flow,
                // prompt user to paste code from stdin
                let pending = self.start_code_paste_flow()?;
                println!("Opening browser for authentication...");
                println!("URL: {}", pending.auth_url);
                webbrowser::open(&pending.auth_url)?;

                println!();
                println!("After authenticating, paste the authorization code below:");
                let mut code = String::new();
                std::io::stdin()
                    .read_line(&mut code)
                    .context("Failed to read authorization code")?;
                let code = code.trim();

                pending.exchange_code(code).await
            }
        }
    }

    async fn refresh(&self, token: &AuthToken) -> Result<AuthToken> {
        let refresh_token = token
            .refresh_token
            .as_ref()
            .context("No refresh token available")?;

        let token_params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", self.config.client_id.as_str()),
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
        if let Some(expires_at) = token.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;

            if now >= expires_at {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

// OAuth callback handlers (for auto-redirect flow)
async fn handle_callback(
    Query(params): Query<AuthCallback>,
    State(state): State<Arc<Mutex<CallbackState>>>,
) -> Result<Redirect, Redirect> {
    let mut state_lock = state.lock().await;

    if params.state.as_ref() != Some(&state_lock.expected_state) {
        if let Some(sender) = state_lock.sender.take() {
            let _ = sender.send(Err(anyhow::anyhow!("Invalid state parameter")));
        }
        return Err(Redirect::to("/error"));
    }

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
            <div class="success">&#10003;</div>
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
            <div class="error">&#10007;</div>
            <h1>Authentication Failed</h1>
            <p>Please try again or check your credentials.</p>
            <script>setTimeout(() => window.close(), 3000);</script>
        </body>
        </html>
    "#,
    )
}
