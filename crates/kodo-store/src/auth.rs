use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::crypto::{self, SecretStore};
use crate::db::DbPool;

/// A stored auth token for a provider.
/// The actual secrets (token, refresh_token) live in the OS keychain.
/// The DB only tracks which providers have credentials and their expiry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub provider: String,
    pub token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
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
) -> Result<()> {
    debug!(provider, "saving auth token");

    // Store secrets in the keychain.
    crypto::set_secret(store, provider, "token", token)?;
    if let Some(rt) = refresh_token {
        crypto::set_secret(store, provider, "refresh_token", rt)?;
    } else {
        crypto::delete_secret(store, provider, "refresh_token")?;
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

    Ok(Some(AuthToken {
        provider: provider_name,
        token,
        refresh_token,
        expires_at,
    }))
}

/// Delete an auth token for a provider.
///
/// Removes secrets from keychain and metadata from DB.
pub async fn delete_token(pool: &DbPool, store: &dyn SecretStore, provider: &str) -> Result<()> {
    debug!(provider, "deleting auth token");

    crypto::delete_secret(store, provider, "token")?;
    crypto::delete_secret(store, provider, "refresh_token")?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::MemoryStore;
    use crate::db::open_memory;

    fn store() -> MemoryStore {
        MemoryStore::new()
    }

    #[tokio::test]
    async fn save_and_get_token_roundtrip() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "anthropic", "sk-ant-123", None, None)
            .await
            .unwrap();

        let token = get_token(&pool, &s, "anthropic").await.unwrap().unwrap();
        assert_eq!(token.token, "sk-ant-123");
        assert!(token.refresh_token.is_none());
    }

    #[tokio::test]
    async fn db_stores_no_secrets() {
        let pool = open_memory().await.unwrap();
        let s = store();
        save_token(&pool, &s, "openai", "secret-key", None, None)
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
        save_token(&pool, &s, "openai", "key-1", None, None)
            .await
            .unwrap();
        save_token(&pool, &s, "openai", "key-2", Some("refresh-2"), None)
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
        save_token(&pool, &s, "anthropic", "key", None, None)
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
        save_token(&pool, &s, "anthropic", "a", None, None)
            .await
            .unwrap();
        save_token(&pool, &s, "openai", "b", None, None)
            .await
            .unwrap();

        let providers = list_providers(&pool).await.unwrap();
        assert_eq!(providers, vec!["anthropic", "openai"]);
    }
}
