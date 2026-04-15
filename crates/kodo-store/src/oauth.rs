use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rand::RngCore;
use rand::rngs::OsRng;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tracing::warn;
use url::Url;

const CODE_VERIFIER_BYTES: usize = 64;
const STATE_BYTES: usize = 32;
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(120);
const OPENAI_ISSUER: &str = "https://auth.openai.com";
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_SCOPES: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const OPENAI_CALLBACK_ADDR: &str = "127.0.0.1:1455";
const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const OPENAI_ORIGINATOR: &str = "codex_cli_rs";

type CallbackResult = std::result::Result<(String, String), String>;
type CallbackSender = oneshot::Sender<CallbackResult>;
type SharedCallbackSender = Arc<Mutex<Option<CallbackSender>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderOAuthConfig {
    OpenAI { issuer: String, client_id: String },
}

impl ProviderOAuthConfig {
    pub fn openai_default() -> Self {
        Self::OpenAI {
            issuer: OPENAI_ISSUER.to_string(),
            client_id: OPENAI_CLIENT_ID.to_string(),
        }
    }

    pub fn issuer(&self) -> &str {
        match self {
            Self::OpenAI { issuer, .. } => issuer,
        }
    }

    pub fn client_id(&self) -> &str {
        match self {
            Self::OpenAI { client_id, .. } => client_id,
        }
    }

    fn authorize_url(&self) -> Result<Url> {
        let issuer = self.issuer().trim_end_matches('/');
        Url::parse(&format!("{issuer}/oauth/authorize")).context("invalid OAuth authorize URL")
    }

    pub fn token_url(&self) -> Result<Url> {
        let issuer = self.issuer().trim_end_matches('/');
        Url::parse(&format!("{issuer}/oauth/token")).context("invalid OAuth token URL")
    }

    fn scopes(&self) -> &'static str {
        match self {
            Self::OpenAI { .. } => OPENAI_SCOPES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ApiKeyExchangeResponse {
    access_token: String,
}

/// Generate a PKCE code verifier from 64 random bytes.
pub fn generate_code_verifier() -> String {
    random_base64url(CODE_VERIFIER_BYTES)
}

/// Compute the PKCE S256 challenge for a verifier.
pub fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// Generate a CSRF state token from 32 random bytes.
pub fn generate_state() -> String {
    random_base64url(STATE_BYTES)
}

pub async fn run_openai_oauth_flow(config: &ProviderOAuthConfig) -> Result<OAuthTokens> {
    run_openai_oauth_flow_with_notifier(config, |_| {}).await
}

pub async fn run_openai_oauth_flow_with_notifier<F>(
    config: &ProviderOAuthConfig,
    mut notify: F,
) -> Result<OAuthTokens>
where
    F: FnMut(String),
{
    let code_verifier = generate_code_verifier();
    let challenge = code_challenge(&code_verifier);
    let state = generate_state();
    let listener = TcpListener::bind(OPENAI_CALLBACK_ADDR)
        .await
        .with_context(|| {
            format!("failed to bind local OAuth callback listener on {OPENAI_CALLBACK_ADDR}")
        })?;
    let authorize_url = build_authorize_url(config, OPENAI_REDIRECT_URI, &challenge, &state)?;

    notify(format!(
        "Finish OpenAI login in your browser. If it does not open automatically, use this URL:\n{authorize_url}"
    ));

    if let Err(error) = open::that(authorize_url.as_str()) {
        warn!(%error, url = %authorize_url, "failed to open browser for OAuth login");
        notify(format!(
            "Failed to open your browser automatically. Open this URL to continue login:\n{authorize_url}"
        ));
    }

    let (code, returned_state) = wait_for_callback(listener).await?;
    if returned_state != state {
        bail!("OAuth state mismatch; login response may be forged");
    }

    notify("Browser login completed. Exchanging OAuth tokens...".to_string());

    let mut tokens =
        exchange_code_openai(config, &code, OPENAI_REDIRECT_URI, &code_verifier).await?;
    let id_token = tokens
        .id_token
        .as_deref()
        .context("OpenAI OAuth response did not include an id_token")?;
    notify("OAuth tokens received. Requesting OpenAI API key...".to_string());
    tokens.access_token = exchange_for_api_key(config, id_token)
        .await
        .context("ChatGPT OAuth succeeded, but OpenAI did not issue an API-key token. Kodo's OpenAI provider still requires an API-key-style credential, unlike Codex CLI")?;
    Ok(tokens)
}

fn build_authorize_url(
    config: &ProviderOAuthConfig,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
) -> Result<Url> {
    let mut url = config.authorize_url()?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", config.client_id())
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", config.scopes())
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", OPENAI_ORIGINATOR)
        .append_pair("state", state);
    Ok(url)
}

async fn wait_for_callback(listener: TcpListener) -> Result<(String, String)> {
    let (tx, rx) = oneshot::channel();
    let tx: SharedCallbackSender = Arc::new(Mutex::new(Some(tx)));

    let server = tokio::spawn({
        let tx = Arc::clone(&tx);
        async move {
            let (stream, _) = listener
                .accept()
                .await
                .context("failed to accept OAuth callback")?;
            let io = TokioIo::new(stream);
            http1::Builder::new()
                .keep_alive(false)
                .serve_connection(
                    io,
                    service_fn(move |request| handle_callback_request(request, Arc::clone(&tx))),
                )
                .await
                .context("failed to serve OAuth callback")?;
            Result::<()>::Ok(())
        }
    });

    let callback = match timeout(CALLBACK_TIMEOUT, rx).await {
        Ok(Ok(Ok(callback))) => callback,
        Ok(Ok(Err(error))) => return Err(anyhow!(error)),
        Ok(Err(_)) => bail!("OAuth callback channel closed before receiving a response"),
        Err(_) => {
            server.abort();
            bail!("Timed out waiting for OAuth callback after 120 seconds")
        }
    };

    server
        .await
        .context("OAuth callback server task failed")??;
    Ok(callback)
}

async fn handle_callback_request(
    request: Request<Incoming>,
    callback_tx: SharedCallbackSender,
) -> std::result::Result<Response<Full<Bytes>>, Infallible> {
    let response = match parse_callback_uri(request.uri()) {
        Ok(callback) => {
            if let Some(tx) = callback_tx.lock().expect("callback mutex poisoned").take() {
                let _ = tx.send(Ok(callback));
            }
            text_response(
                StatusCode::OK,
                "Login successful! You can close this window.",
            )
        }
        Err(error) => {
            if let Some(tx) = callback_tx.lock().expect("callback mutex poisoned").take() {
                let _ = tx.send(Err(error.to_string()));
            }
            text_response(StatusCode::BAD_REQUEST, &error.to_string())
        }
    };

    Ok(response)
}

fn parse_callback_uri(uri: &hyper::Uri) -> Result<(String, String)> {
    if uri.path() != "/auth/callback" {
        bail!("unexpected OAuth callback path: {}", uri.path());
    }

    let query = uri
        .query()
        .context("OAuth callback missing query parameters")?;
    let params: HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();

    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map_or("", std::string::String::as_str);
        if description.is_empty() {
            bail!("OAuth authorization failed: {error}");
        }
        bail!("OAuth authorization failed: {error} ({description})");
    }

    let code = params
        .get("code")
        .cloned()
        .context("OAuth callback missing authorization code")?;
    let state = params
        .get("state")
        .cloned()
        .context("OAuth callback missing state")?;
    Ok((code, state))
}

fn text_response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .header("connection", "close")
        .body(Full::new(Bytes::from(body.to_owned())))
        .expect("response should be valid")
}

pub async fn exchange_code_openai(
    config: &ProviderOAuthConfig,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<OAuthTokens> {
    exchange_code_openai_with_client(&Client::new(), config, code, redirect_uri, code_verifier)
        .await
}

async fn exchange_code_openai_with_client(
    client: &Client,
    config: &ProviderOAuthConfig,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<OAuthTokens> {
    let response = client
        .post(config.token_url()?)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", config.client_id()),
            ("code_verifier", code_verifier),
        ])
        .send()
        .await
        .context("failed to exchange OAuth authorization code")?;

    deserialize_oauth_response(response, "exchange authorization code").await
}

pub async fn exchange_for_api_key(config: &ProviderOAuthConfig, id_token: &str) -> Result<String> {
    exchange_for_api_key_with_client(&Client::new(), config, id_token).await
}

pub async fn refresh_openai_tokens(
    config: &ProviderOAuthConfig,
    refresh_token: &str,
) -> Result<OAuthTokens> {
    refresh_openai_tokens_with_client(&Client::new(), config, refresh_token).await
}

pub(crate) async fn refresh_openai_tokens_with_client(
    client: &Client,
    config: &ProviderOAuthConfig,
    refresh_token: &str,
) -> Result<OAuthTokens> {
    let response = client
        .post(config.token_url()?)
        .json(&serde_json::json!({
            "client_id": config.client_id(),
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .context("failed to refresh OpenAI OAuth token")?;

    deserialize_oauth_response(response, "refresh OAuth token").await
}

async fn exchange_for_api_key_with_client(
    client: &Client,
    config: &ProviderOAuthConfig,
    id_token: &str,
) -> Result<String> {
    let form = vec![
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:token-exchange".to_string(),
        ),
        ("client_id", config.client_id().to_string()),
        ("requested_token", "openai-api-key".to_string()),
        ("subject_token", id_token.to_string()),
        (
            "subject_token_type",
            "urn:ietf:params:oauth:token-type:id_token".to_string(),
        ),
    ];

    let response = client
        .post(config.token_url()?)
        .form(&form)
        .send()
        .await
        .context("failed to exchange id_token for OpenAI API key")?;

    let payload: ApiKeyExchangeResponse =
        deserialize_oauth_response(response, "exchange id_token for API key").await?;
    Ok(payload.access_token)
}

async fn deserialize_oauth_response<T>(response: reqwest::Response, action: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("failed to read OAuth response while attempting to {action}"))?;

    if !status.is_success() {
        bail!("OpenAI OAuth failed to {action} ({status}): {body}");
    }

    serde_json::from_str(&body).with_context(|| {
        format!("failed to parse OAuth JSON response while attempting to {action}")
    })
}

fn random_base64url(byte_len: usize) -> String {
    let mut bytes = vec![0_u8; byte_len];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    fn is_url_safe_base64(value: &str) -> bool {
        value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    }

    #[test]
    fn code_verifier_is_url_safe_and_long_enough() {
        let verifier = generate_code_verifier();

        assert!(verifier.len() >= 43);
        assert!(is_url_safe_base64(&verifier));
    }

    #[test]
    fn code_challenge_is_deterministic_and_distinct() {
        let verifier = "test-verifier";
        let challenge = code_challenge(verifier);

        assert_eq!(challenge, code_challenge(verifier));
        assert_ne!(challenge, verifier);
        assert!(is_url_safe_base64(&challenge));
    }

    #[test]
    fn state_is_url_safe_and_non_empty() {
        let state = generate_state();

        assert!(!state.is_empty());
        assert!(is_url_safe_base64(&state));
    }

    #[test]
    fn openai_default_matches_codex_constants() {
        let config = ProviderOAuthConfig::openai_default();

        assert_eq!(config.issuer(), OPENAI_ISSUER);
        assert_eq!(config.client_id(), OPENAI_CLIENT_ID);
        assert_eq!(
            config.token_url().unwrap().as_str(),
            "https://auth.openai.com/oauth/token"
        );
    }

    #[test]
    fn authorize_url_contains_pkce_and_state() {
        let config = ProviderOAuthConfig::openai_default();
        let url = build_authorize_url(&config, OPENAI_REDIRECT_URI, "challenge-123", "state-456")
            .unwrap();
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(url.path(), "/oauth/authorize");
        assert_eq!(url.host_str(), Some("auth.openai.com"));
        assert_eq!(
            params.get("response_type").map(String::as_str),
            Some("code")
        );
        assert_eq!(
            params.get("client_id").map(String::as_str),
            Some(OPENAI_CLIENT_ID)
        );
        assert_eq!(
            params.get("redirect_uri").map(String::as_str),
            Some(OPENAI_REDIRECT_URI)
        );
        assert_eq!(params.get("scope").map(String::as_str), Some(OPENAI_SCOPES));
        assert_eq!(
            params.get("code_challenge").map(String::as_str),
            Some("challenge-123")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            params.get("id_token_add_organizations").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            params.get("codex_cli_simplified_flow").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            params.get("originator").map(String::as_str),
            Some(OPENAI_ORIGINATOR)
        );
        assert_eq!(params.get("state").map(String::as_str), Some("state-456"));
    }

    #[tokio::test]
    async fn wait_for_callback_roundtrips_code_and_state() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let waiter = tokio::spawn(wait_for_callback(listener));

        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                b"GET /auth/callback?code=test-code&state=test-state HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();

        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();

        let (code, state) = waiter.await.unwrap().unwrap();
        assert_eq!(code, "test-code");
        assert_eq!(state, "test-state");
        assert!(response.contains("Login successful!"));
    }

    #[test]
    fn parse_callback_request_rejects_missing_state() {
        let uri: hyper::Uri = "/auth/callback?code=abc".parse().unwrap();
        let error = parse_callback_uri(&uri).unwrap_err();
        assert!(error.to_string().contains("missing state"));
    }

}
