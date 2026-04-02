use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{Query, State},
    response::Html,
    routing::get,
};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, oneshot};
use tower_http::cors::CorsLayer;
use url::Url;

use crate::{AuthConfig, AuthProvider, AuthToken, OAuthFlowType, TokenRequestFormat};

/// OAuth authorization code response (or error)
#[derive(Debug, Deserialize)]
struct AuthCallback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// OAuth token response
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    /// OIDC id_token (used by OpenAI for token exchange to get API key)
    #[serde(default)]
    id_token: Option<String>,
}

/// Standard OAuth error response (OpenAI style)
#[derive(Debug, Deserialize)]
struct StandardOAuthError {
    error: Option<String>,
    error_description: Option<String>,
}

/// Anthropic-style nested error: {"error": {"type": "...", "message": "..."}}
#[derive(Debug, Deserialize)]
struct AnthropicErrorWrapper {
    error: AnthropicErrorInner,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorInner {
    #[serde(rename = "type")]
    error_type: Option<String>,
    message: Option<String>,
}

/// Try to extract a human-readable error message from the response body.
fn parse_oauth_error(body: &str) -> String {
    // Try Anthropic nested format first: {"error": {"type": ..., "message": ...}}
    if let Ok(err) = serde_json::from_str::<AnthropicErrorWrapper>(body) {
        let error_type = err.error.error_type.as_deref().unwrap_or("error");
        let message = err.error.message.as_deref().unwrap_or("");
        return format!("{}: {}", error_type, message);
    }
    // Try standard OAuth format: {"error": "...", "error_description": "..."}
    if let Ok(err) = serde_json::from_str::<StandardOAuthError>(body) {
        let error = err.error.as_deref().unwrap_or("error");
        let desc = err.error_description.as_deref().unwrap_or("");
        return format!("{} - {}", error, desc);
    }
    // Fall back to raw body (truncated)
    if body.len() > 200 {
        format!("{}...", &body[..200])
    } else {
        body.to_string()
    }
}

/// Shared state for OAuth callback
struct CallbackState {
    sender: Option<oneshot::Sender<Result<String>>>,
    expected_state: String,
}

/// Send a token request with the correct format (JSON or form-encoded)
/// and required headers based on the provider config.
async fn send_token_request(
    client: &reqwest::Client,
    config: &AuthConfig,
    params: &serde_json::Value,
) -> Result<AuthToken> {
    tracing::debug!(
        "Token request: url={}, format={:?}, params={}",
        config.token_url,
        config.token_request_format,
        params
    );

    let mut req = client.post(&config.token_url);

    // Anthropic requires User-Agent: anthropic
    if config.provider == "anthropic" {
        req = req.header("User-Agent", "anthropic");
    }

    // Send as JSON or form-encoded based on config
    req = match config.token_request_format {
        TokenRequestFormat::Json => req
            .header("Content-Type", "application/json")
            .body(params.to_string()),
        TokenRequestFormat::FormEncoded => {
            // Convert JSON object to form params
            let form_params: Vec<(String, String)> = params
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            req.form(&form_params)
        }
    };

    let response = req.send().await.context("Failed to send token request")?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "(failed to read response body)".to_string());
        tracing::error!(
            "Token request failed: status={}, url={}, body={}",
            status,
            config.token_url,
            body
        );

        anyhow::bail!(
            "OAuth token request failed ({}): {}",
            status,
            parse_oauth_error(&body)
        );
    }

    let body = response
        .text()
        .await
        .context("Failed to read token response body")?;
    tracing::debug!("Token response: {}", body);

    let token_response: TokenResponse =
        serde_json::from_str(&body).context(format!("Failed to parse token response: {}", body))?;

    // For OpenAI: exchange the id_token for an API key via RFC 8693 token exchange.
    // The OAuth access_token alone doesn't have API permissions; we need an actual API key.
    if config.provider == "openai" {
        if let Some(id_token) = &token_response.id_token {
            tracing::debug!("OpenAI: exchanging id_token for API key via token exchange");
            let api_key = obtain_openai_api_key(client, config, id_token).await?;
            return Ok(AuthToken {
                provider: config.provider.clone(),
                access_token: api_key,
                refresh_token: token_response.refresh_token,
                expires_at: token_response.expires_in.map(|exp| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64
                        + exp
                }),
            });
        } else {
            tracing::warn!("OpenAI: no id_token in response, using access_token directly");
        }
    }

    Ok(AuthToken {
        provider: config.provider.clone(),
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

/// Exchange an OIDC id_token for an OpenAI API key via RFC 8693 Token Exchange.
/// This is what the Codex CLI does to obtain a key with full API permissions
/// (including api.responses.write) from the OAuth login flow.
async fn obtain_openai_api_key(
    client: &reqwest::Client,
    config: &AuthConfig,
    id_token: &str,
) -> Result<String> {
    let body = format!(
        "grant_type={}&client_id={}&requested_token={}&subject_token={}&subject_token_type={}",
        urlencoding::encode("urn:ietf:params:oauth:grant-type:token-exchange"),
        urlencoding::encode(&config.client_id),
        urlencoding::encode("openai-api-key"),
        urlencoding::encode(id_token),
        urlencoding::encode("urn:ietf:params:oauth:token-type:id_token"),
    );

    tracing::debug!("OpenAI token exchange: url={}", config.token_url);

    let response = client
        .post(&config.token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .context("Failed to send OpenAI token exchange request")?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "(failed to read response body)".to_string());
        tracing::error!(
            "OpenAI token exchange failed: status={}, body={}",
            status,
            body
        );
        anyhow::bail!(
            "OpenAI token exchange failed ({}): {}",
            status,
            parse_oauth_error(&body)
        );
    }

    let resp_body = response
        .text()
        .await
        .context("Failed to read token exchange response")?;
    tracing::debug!("OpenAI token exchange response received");

    let token_resp: TokenResponse = serde_json::from_str(&resp_body).context(format!(
        "Failed to parse token exchange response: {}",
        resp_body
    ))?;

    Ok(token_resp.access_token)
}

/// Holds the PKCE verifier and state for a pending code-paste OAuth flow.
/// The TUI shows the URL to the user, they paste the code back,
/// and then we call `exchange_code` with that code.
pub struct PendingCodePaste {
    /// The URL to open in the browser
    pub auth_url: String,
    /// PKCE code verifier (needed for token exchange)
    verifier: String,
    /// State parameter (may equal verifier for Anthropic)
    state: String,
    /// The auth config (for token exchange)
    config: AuthConfig,
    /// HTTP client
    http_client: reqwest::Client,
}

impl PendingCodePaste {
    /// Exchange the user-pasted authorization code for a token.
    pub async fn exchange_code(&self, code: &str) -> Result<AuthToken> {
        let mut params = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "client_id": self.config.client_id,
            "redirect_uri": self.config.redirect_uri,
            "code_verifier": self.verifier,
        });

        // Include state if the flow requires it
        if self.config.state_equals_verifier {
            params["state"] = serde_json::Value::String(self.state.clone());
        }

        send_token_request(&self.http_client, &self.config, &params).await
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

    /// Generate PKCE challenge.
    /// Returns (verifier, challenge) where verifier is a 43-char base64url string.
    fn generate_pkce() -> (String, String) {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

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
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

        let mut rng = rand::thread_rng();
        let mut state_bytes = [0u8; 32];
        rng.fill_bytes(&mut state_bytes);
        URL_SAFE_NO_PAD.encode(state_bytes)
    }

    /// Build the authorization URL with PKCE parameters.
    /// Returns (url, verifier, state).
    fn build_auth_url(&self) -> Result<(Url, String, String)> {
        let (verifier, challenge) = Self::generate_pkce();

        // Per the Anthropic spec, state must equal the code_verifier
        let state = if self.config.state_equals_verifier {
            verifier.clone()
        } else {
            Self::generate_state()
        };

        let mut auth_url = Url::parse(&self.config.auth_url)?;

        // For code-paste flows (Anthropic), add code=true first
        if self.config.flow_type == OAuthFlowType::CodePaste {
            auth_url.query_pairs_mut().append_pair("code", "true");
        }

        auth_url
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &self.config.redirect_uri)
            .append_pair("scope", &self.config.scopes.join(" "))
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);

        // Add audience if configured
        if let Some(ref audience) = self.config.audience {
            auth_url.query_pairs_mut().append_pair("audience", audience);
        }

        // OpenAI-specific extra params for the Codex simplified flow
        if self.config.provider == "openai" {
            auth_url
                .query_pairs_mut()
                .append_pair("id_token_add_organizations", "true")
                .append_pair("codex_cli_simplified_flow", "true")
                .append_pair("originator", "kodo");
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
            state,
            config: self.config.clone(),
            http_client: self.http_client.clone(),
        })
    }

    /// Exchange code for token (used by auto-redirect flow)
    async fn exchange_code(&self, code: &str, verifier: &str, state: &str) -> Result<AuthToken> {
        let mut params = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "client_id": self.config.client_id,
            "redirect_uri": self.config.redirect_uri,
            "code_verifier": verifier,
        });

        if self.config.state_equals_verifier {
            params["state"] = serde_json::Value::String(state.to_string());
        }

        send_token_request(&self.http_client, &self.config, &params).await
    }

    /// Auto-redirect login: starts localhost callback server, opens browser,
    /// waits for the redirect callback with the auth code.
    async fn login_auto_redirect(&self) -> Result<AuthToken> {
        let (auth_url, verifier, state) = self.build_auth_url()?;

        // Create callback channel
        let (tx, rx) = oneshot::channel();
        let callback_state = Arc::new(Mutex::new(CallbackState {
            sender: Some(tx),
            expected_state: state.clone(),
        }));

        // Start local server for callback
        let app = Router::new()
            .route("/callback", get(handle_callback))
            .route("/auth/callback", get(handle_callback))
            .layer(CorsLayer::permissive())
            .with_state(callback_state);

        // Extract port from redirect_uri, bind to all interfaces so both
        // localhost (which may resolve to ::1 on macOS) and 127.0.0.1 work.
        let redirect_url = Url::parse(&self.config.redirect_uri).context("Invalid redirect_uri")?;
        let port = redirect_url.port().unwrap_or(8899);
        let bind_addr = format!("0.0.0.0:{}", port);

        tracing::debug!("Binding OAuth callback server to {}", bind_addr);
        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .context(format!(
                "Failed to bind OAuth callback server on {}",
                bind_addr
            ))?;

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

        // Give the server a moment to flush the success HTML to the browser
        tokio::time::sleep(Duration::from_millis(500)).await;
        server_handle.abort();

        // Exchange code for token
        self.exchange_code(&code, &verifier, &state).await
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

        let params = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": self.config.client_id,
        });

        send_token_request(&self.http_client, &self.config, &params).await
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
// Returns HTML directly instead of redirecting, because the server
// is aborted immediately after receiving the code.
async fn handle_callback(
    Query(params): Query<AuthCallback>,
    State(state): State<Arc<Mutex<CallbackState>>>,
) -> Html<&'static str> {
    let mut state_lock = state.lock().await;

    // Check for OAuth error response (e.g. access_denied, invalid_scope)
    if let Some(error) = &params.error {
        let desc = params
            .error_description
            .as_deref()
            .unwrap_or("Unknown error");
        tracing::error!("OAuth callback error: {} - {}", error, desc);
        if let Some(sender) = state_lock.sender.take() {
            let _ = sender.send(Err(anyhow::anyhow!("OAuth error: {} - {}", error, desc)));
        }
        return Html(ERROR_HTML);
    }

    // Validate state
    if params.state.as_ref() != Some(&state_lock.expected_state) {
        if let Some(sender) = state_lock.sender.take() {
            let _ = sender.send(Err(anyhow::anyhow!("Invalid state parameter")));
        }
        return Html(ERROR_HTML);
    }

    // Extract code
    let Some(code) = params.code else {
        if let Some(sender) = state_lock.sender.take() {
            let _ = sender.send(Err(anyhow::anyhow!("No authorization code in callback")));
        }
        return Html(ERROR_HTML);
    };

    if let Some(sender) = state_lock.sender.take() {
        let _ = sender.send(Ok(code));
    }

    Html(SUCCESS_HTML)
}

const SUCCESS_HTML: &str = r#"
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
    <p>You can close this window and return to kodo.</p>
    <script>setTimeout(() => window.close(), 2000);</script>
</body>
</html>
"#;

const ERROR_HTML: &str = r#"
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
</body>
</html>
"#;
