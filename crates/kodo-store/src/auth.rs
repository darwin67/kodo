use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::crypto::{self, SecretStore};
use crate::db::DbPool;
use crate::oauth::{self, ProviderOAuthConfig};

/// A stored auth token for a provider.
/// The actual secrets (token, refresh_token) live in the OS keychain.
/// The DB only tracks which providers have credentials and their expiry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub provider: String,
    pub token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
    pub account_id: Option<String>,
}

/// Store or update an auth token for a provider.
///
/// Secrets go to the OS keychain (or test MemoryStore).
/// Only the provider name and expiry are stored in the DB.
pub async fn save_token(
    pool: &DbPool,
    store: &dyn SecretStore,
    provider: &str,
    token: &str,
    refresh_token: Option<&str>,
    expires_at: Option<&str>,
    account_id: Option<&str>,
) -> Result<()> {
    debug!(provider, "saving auth token");

    // Store secrets in the keychain.
    crypto::set_secret(store, provider, "token", token)?;
    if let Some(rt) = refresh_token {
        crypto::set_secret(store, provider, "refresh_token", rt)?;
    } else {
        crypto::delete_secret(store, provider, "refresh_token")?;
    }
    if let Some(account_id) = account_id {
        crypto::set_secret(store, provider, "account_id", account_id)?;
    } else {
        crypto::delete_secret(store, provider, "account_id")?;
    }

    // Track provider metadata in DB (no secrets).
    sqlx::query(
        "INSERT INTO auth_providers (provider, expires_at) \
         VALUES (?, ?) \
         ON CONFLICT(provider) DO UPDATE SET \
           expires_at = excluded.expires_at, \
           updated_at = datetime('now')",
    )
    .bind(provider)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get the auth token for a provider.
///
/// Retrieves secrets from the keychain and metadata from the DB.
pub async fn get_token(
    pool: &DbPool,
    store: &dyn SecretStore,
    provider: &str,
) -> Result<Option<AuthToken>> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT provider, expires_at FROM auth_providers WHERE provider = ?")
            .bind(provider)
            .fetch_optional(pool)
            .await?;

    let (provider_name, expires_at) = match row {
        Some(r) => r,
        None => return Ok(None),
    };

    let token = match crypto::get_secret(store, &provider_name, "token")? {
        Some(t) => t,
        None => return Ok(None),
    };

    let refresh_token = crypto::get_secret(store, &provider_name, "refresh_token")?;
    let account_id = crypto::get_secret(store, &provider_name, "account_id")?;

    Ok(Some(AuthToken {
        provider: provider_name,
        token,
        refresh_token,
        expires_at,
        account_id,
    }))
}

/// Refresh an OAuth token if it is expired or near expiry.
///
/// Currently only OpenAI tokens participate in refresh; API keys are returned
/// as-is because they do not expire.
pub async fn refresh_if_needed(
    pool: &DbPool,
    store: &dyn SecretStore,
    account_id: &str,
) -> Result<Option<AuthToken>> {
    refresh_if_needed_with_client(pool, store, &Client::new(), account_id).await
}

async fn refresh_if_needed_with_client(
    pool: &DbPool,
    store: &dyn SecretStore,
    client: &Client,
    account_id: &str,
) -> Result<Option<AuthToken>> {
    let Some(existing) = get_token(pool, store, account_id).await? else {
        return Ok(None);
    };

    let Some(expires_at) = existing.expires_at.as_deref() else {
        return Ok(Some(existing));
    };

    let expires_at = parse_expires_at(expires_at)
        .with_context(|| format!("failed to parse token expiry for `{account_id}`"))?;
    let refresh_deadline = Utc::now() + Duration::minutes(8);
    if expires_at > refresh_deadline {
        return Ok(Some(existing));
    }

    let Some(refresh_token) = existing.refresh_token.as_deref() else {
        return Ok(None);
    };

    if account_id != "openai" {
        return Ok(Some(existing));
    }

    let config = ProviderOAuthConfig::openai_default();
    refresh_openai_token(pool, store, client, account_id, &config, refresh_token).await
}

async fn refresh_openai_token(
    pool: &DbPool,
    store: &dyn SecretStore,
    client: &Client,
    account_id: &str,
    config: &ProviderOAuthConfig,
    refresh_token: &str,
) -> Result<Option<AuthToken>> {
    let refreshed = oauth::refresh_openai_tokens_with_client(client, config, refresh_token).await?;
    let id_token = refreshed
        .id_token
        .as_deref()
        .context("OpenAI refresh response did not include an id_token")?;
    let metadata = oauth::parse_openai_id_token_metadata(id_token)?;
    let next_refresh_token = refreshed.refresh_token.as_deref().unwrap_or(refresh_token);
    let expires_at = refreshed.expires_in.map(format_expires_at);

    save_token(
        pool,
        store,
        account_id,
        &refreshed.access_token,
        Some(next_refresh_token),
        expires_at.as_deref(),
        metadata.chatgpt_account_id.as_deref(),
    )
    .await?;

    get_token(pool, store, account_id).await
}

/// Resolve the stored auth for a provider, refreshing it when supported.
pub async fn resolve_auth(
    pool: &DbPool,
    store: &dyn SecretStore,
    account_id: &str,
) -> Result<AuthToken> {
    let token = if account_id == "openai" {
        refresh_if_needed(pool, store, account_id).await?
    } else {
        get_token(pool, store, account_id).await?
    };

    token.ok_or_else(|| {
        anyhow::anyhow!("No credentials for {account_id}. Run: kodo --login {account_id}")
    })
}

/// Delete an auth token for a provider.
///
/// Removes secrets from keychain and metadata from DB.
pub async fn delete_token(pool: &DbPool, store: &dyn SecretStore, provider: &str) -> Result<()> {
    debug!(provider, "deleting auth token");

    crypto::delete_secret(store, provider, "token")?;
    crypto::delete_secret(store, provider, "refresh_token")?;
    crypto::delete_secret(store, provider, "account_id")?;

    sqlx::query("DELETE FROM auth_providers WHERE provider = ?")
        .bind(provider)
        .execute(pool)
        .await?;

    Ok(())
}

/// List all providers that have stored credentials.
pub async fn list_providers(pool: &DbPool) -> Result<Vec<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT provider FROM auth_providers ORDER BY provider")
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

fn parse_expires_at(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let dt = date
            .and_hms_opt(23, 59, 59)
            .context("failed to interpret expiry date")?;
        return Ok(DateTime::from_naive_utc_and_offset(dt, Utc));
    }

    warn!(expires_at = value, "unrecognized auth token expiry format");
    bail!("unrecognized expiry format: {value}")
}

fn format_expires_at(expires_in: i64) -> String {
    (Utc::now() + Duration::seconds(expires_in)).to_rfc3339()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::ErrorKind;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::crypto::MemoryStore;
    use crate::db::open_memory;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    fn store() -> MemoryStore {
        MemoryStore::new()
    }

    #[derive(Debug, Clone)]
    struct RecordedRequest {
        path: String,
        content_type: Option<String>,
        body: String,
    }

    async fn spawn_token_server(
        responses: Vec<String>,
    ) -> Result<(String, Arc<Mutex<Vec<RecordedRequest>>>, JoinHandle<()>)> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);

        let handle = tokio::spawn(async move {
            for response_body in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                captured.lock().unwrap().push(request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        Ok((format!("http://{addr}"), requests, handle))
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> RecordedRequest {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 1024];
        let header_end;

        loop {
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(
                read > 0,
                "connection closed before request headers completed"
            );
            buffer.extend_from_slice(&chunk[..read]);
            if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                header_end = index + 4;
                break;
            }
        }

        let header_text = String::from_utf8_lossy(&buffer[..header_end]).to_string();
        let mut lines = header_text.split("\r\n").filter(|line| !line.is_empty());
        let request_line = lines.next().unwrap();
        let path = request_line.split_whitespace().nth(1).unwrap().to_string();

        let headers: HashMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);

        let mut body_bytes = buffer[header_end..].to_vec();
        while body_bytes.len() < content_length {
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "connection closed before request body completed");
            body_bytes.extend_from_slice(&chunk[..read]);
        }
        body_bytes.truncate(content_length);

        RecordedRequest {
            path,
            content_type: headers.get("content-type").cloned(),
            body: String::from_utf8(body_bytes).unwrap(),
        }
    }

    #[tokio::test]
    async fn save_and_get_token_roundtrip() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "anthropic", "sk-ant-123", None, None, None)
            .await
            .unwrap();

        let token = get_token(&pool, &s, "anthropic").await.unwrap().unwrap();
        assert_eq!(token.token, "sk-ant-123");
        assert!(token.refresh_token.is_none());
        assert!(token.account_id.is_none());
    }

    #[tokio::test]
    async fn db_stores_no_secrets() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "openai", "secret-key", None, None, None)
            .await
            .unwrap();

        // The DB should only have provider and expires_at, no token columns.
        let row: (String, Option<String>) = sqlx::query_as(
            "SELECT provider, expires_at FROM auth_providers WHERE provider = 'openai'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "openai");
        assert!(row.1.is_none()); // no expiry set
    }

    #[tokio::test]
    async fn upsert_token() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "openai", "key-1", None, None, None)
            .await
            .unwrap();
        save_token(&pool, &s, "openai", "key-2", Some("refresh-2"), None, None)
            .await
            .unwrap();

        let token = get_token(&pool, &s, "openai").await.unwrap().unwrap();
        assert_eq!(token.token, "key-2");
        assert_eq!(token.refresh_token, Some("refresh-2".into()));
    }

    #[tokio::test]
    async fn refresh_token_roundtrip() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(
            &pool,
            &s,
            "google",
            "access",
            Some("refresh-secret"),
            Some("2025-12-31"),
            None,
        )
        .await
        .unwrap();

        let token = get_token(&pool, &s, "google").await.unwrap().unwrap();
        assert_eq!(token.token, "access");
        assert_eq!(token.refresh_token, Some("refresh-secret".into()));
        assert_eq!(token.expires_at, Some("2025-12-31".into()));
    }

    #[tokio::test]
    async fn get_nonexistent_token() {
        let pool = open_memory().await.unwrap();
        let s = store();
        let result = get_token(&pool, &s, "nope").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_token_works() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "anthropic", "key", None, None, None)
            .await
            .unwrap();
        delete_token(&pool, &s, "anthropic").await.unwrap();
        assert!(get_token(&pool, &s, "anthropic").await.unwrap().is_none());

        // Keychain should also be cleaned up.
        assert!(
            crypto::get_secret(&s, "anthropic", "token")
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn list_providers_works() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "anthropic", "a", None, None, None)
            .await
            .unwrap();
        save_token(&pool, &s, "openai", "b", None, None, None)
            .await
            .unwrap();

        let providers = list_providers(&pool).await.unwrap();
        assert_eq!(providers, vec!["anthropic", "openai"]);
    }

    #[tokio::test]
    async fn refresh_if_needed_returns_existing_token_without_expiry() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "anthropic", "sk-ant-123", None, None, None)
            .await
            .unwrap();

        let token = refresh_if_needed(&pool, &s, "anthropic")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(token.token, "sk-ant-123");
        assert!(token.expires_at.is_none());
    }

    #[tokio::test]
    async fn refresh_if_needed_returns_existing_token_when_not_near_expiry() {
        let pool = open_memory().await.unwrap();
        let s = store();
        let expires_at = (Utc::now() + Duration::minutes(30)).to_rfc3339();
        save_token(
            &pool,
            &s,
            "openai",
            "existing-api-key",
            Some("refresh-1"),
            Some(&expires_at),
            None,
        )
        .await
        .unwrap();

        let token = refresh_if_needed(&pool, &s, "openai")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(token.token, "existing-api-key");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-1"));
    }

    #[tokio::test]
    async fn refresh_if_needed_returns_none_when_expired_without_refresh_token() {
        let pool = open_memory().await.unwrap();
        let s = store();
        let expires_at = (Utc::now() - Duration::minutes(5)).to_rfc3339();
        save_token(
            &pool,
            &s,
            "openai",
            "expired-key",
            None,
            Some(&expires_at),
            None,
        )
            .await
            .unwrap();

        let token = refresh_if_needed(&pool, &s, "openai").await.unwrap();
        assert!(token.is_none());
    }

    #[tokio::test]
    async fn refresh_if_needed_refreshes_openai_and_persists_updated_token() {
        let pool = open_memory().await.unwrap();
        let s = store();
        let expires_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
        save_token(
            &pool,
            &s,
            "openai",
            "stale-api-key",
            Some("refresh-1"),
            Some(&expires_at),
            None,
        )
        .await
        .unwrap();

        let id_token = "header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC0yIn19.sig";
        let (issuer, requests, server) = match spawn_token_server(vec![format!(
            r#"{{"access_token":"oauth-access","refresh_token":"refresh-2","id_token":"{id_token}","expires_in":3600}}"#
        )])
        .await
        {
            Ok(server) => server,
            Err(error) if error.downcast_ref::<std::io::Error>().map(|io| io.kind())
                == Some(ErrorKind::PermissionDenied) =>
            {
                return;
            }
            Err(error) => panic!("failed to start mock token server: {error:#}"),
        };
        let config = ProviderOAuthConfig::OpenAI {
            issuer,
            client_id: "app-test".to_string(),
        };
        let client = Client::new();

        let token = refresh_openai_token(&pool, &s, &client, "openai", &config, "refresh-1")
            .await
            .unwrap()
            .unwrap();
        server.await.unwrap();

        assert_eq!(token.token, "oauth-access");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-2"));
        assert_eq!(token.account_id.as_deref(), Some("acct-2"));
        assert!(parse_expires_at(token.expires_at.as_deref().unwrap()).unwrap() > Utc::now());

        let recorded = requests.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].path, "/oauth/token");
        assert_eq!(
            recorded[0].content_type.as_deref(),
            Some("application/json")
        );
        assert!(recorded[0].body.contains(r#""grant_type":"refresh_token""#));
        assert!(recorded[0].body.contains(r#""refresh_token":"refresh-1""#));

        let persisted = get_token(&pool, &s, "openai").await.unwrap().unwrap();
        assert_eq!(persisted.token, "oauth-access");
        assert_eq!(persisted.refresh_token.as_deref(), Some("refresh-2"));
        assert_eq!(persisted.account_id.as_deref(), Some("acct-2"));
    }

    #[tokio::test]
    async fn resolve_auth_returns_existing_api_key_for_non_openai_provider() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "gemini", "gemini-key", None, None, None)
            .await
            .unwrap();

        let token = resolve_auth(&pool, &s, "gemini").await.unwrap();
        assert_eq!(token.token, "gemini-key");
    }

    #[tokio::test]
    async fn resolve_auth_errors_when_missing_credentials() {
        let pool = open_memory().await.unwrap();
        let s = store();

        let error = resolve_auth(&pool, &s, "openai").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "No credentials for openai. Run: kodo --login openai"
        );
    }
}
