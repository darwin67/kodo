use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::crypto;
use crate::db::DbPool;

/// A stored auth token for a provider.
/// Tokens are returned in plaintext after decryption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub provider: String,
    pub token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
}

/// Store or update an auth token for a provider.
/// Tokens and refresh tokens are encrypted at rest using AES-256-GCM.
pub async fn save_token(
    pool: &DbPool,
    provider: &str,
    token: &str,
    refresh_token: Option<&str>,
    expires_at: Option<&str>,
) -> Result<()> {
    debug!(provider, "saving auth token (encrypted)");

    let key = crypto::derive_machine_key();
    let encrypted_token = crypto::encrypt(token, &key)?;
    let encrypted_refresh = refresh_token
        .map(|rt| crypto::encrypt(rt, &key))
        .transpose()?;

    sqlx::query(
        "INSERT INTO auth_tokens (provider, token, refresh_token, expires_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(provider) DO UPDATE SET \
           token = excluded.token, \
           refresh_token = excluded.refresh_token, \
           expires_at = excluded.expires_at, \
           updated_at = datetime('now')",
    )
    .bind(provider)
    .bind(&encrypted_token)
    .bind(&encrypted_refresh)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get the auth token for a provider.
/// Returns the decrypted plaintext token.
pub async fn get_token(pool: &DbPool, provider: &str) -> Result<Option<AuthToken>> {
    let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT provider, token, refresh_token, expires_at FROM auth_tokens WHERE provider = ?",
    )
    .bind(provider)
    .fetch_optional(pool)
    .await?;

    match row {
        Some(r) => {
            let key = crypto::derive_machine_key();
            let token = crypto::decrypt(&r.1, &key)?;
            let refresh_token = r.2.map(|rt| crypto::decrypt(&rt, &key)).transpose()?;

            Ok(Some(AuthToken {
                provider: r.0,
                token,
                refresh_token,
                expires_at: r.3,
            }))
        }
        None => Ok(None),
    }
}

/// Delete an auth token for a provider.
pub async fn delete_token(pool: &DbPool, provider: &str) -> Result<()> {
    sqlx::query("DELETE FROM auth_tokens WHERE provider = ?")
        .bind(provider)
        .execute(pool)
        .await?;
    Ok(())
}

/// List all stored auth tokens (providers only, not the actual tokens).
pub async fn list_providers(pool: &DbPool) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as("SELECT provider FROM auth_tokens ORDER BY provider")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_memory;

    #[tokio::test]
    async fn save_and_get_token_roundtrip() {
        let pool = open_memory().await.unwrap();
        save_token(&pool, "anthropic", "sk-ant-123", None, None)
            .await
            .unwrap();

        let token = get_token(&pool, "anthropic").await.unwrap().unwrap();
        assert_eq!(token.provider, "anthropic");
        assert_eq!(token.token, "sk-ant-123");
        assert!(token.refresh_token.is_none());
    }

    #[tokio::test]
    async fn token_is_encrypted_in_db() {
        let pool = open_memory().await.unwrap();
        save_token(&pool, "openai", "sk-plaintext-key", None, None)
            .await
            .unwrap();

        // Read the raw value from the DB — it should NOT be the plaintext.
        let row: (String,) =
            sqlx::query_as("SELECT token FROM auth_tokens WHERE provider = 'openai'")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_ne!(row.0, "sk-plaintext-key", "token should be encrypted in DB");
        assert!(
            row.0.len() > "sk-plaintext-key".len(),
            "encrypted token should be longer than plaintext"
        );
    }

    #[tokio::test]
    async fn upsert_token() {
        let pool = open_memory().await.unwrap();
        save_token(&pool, "openai", "key-1", None, None)
            .await
            .unwrap();
        save_token(&pool, "openai", "key-2", Some("refresh-2"), None)
            .await
            .unwrap();

        let token = get_token(&pool, "openai").await.unwrap().unwrap();
        assert_eq!(token.token, "key-2");
        assert_eq!(token.refresh_token, Some("refresh-2".into()));
    }

    #[tokio::test]
    async fn refresh_token_encrypted_roundtrip() {
        let pool = open_memory().await.unwrap();
        save_token(
            &pool,
            "google",
            "access-token",
            Some("refresh-secret"),
            Some("2025-12-31"),
        )
        .await
        .unwrap();

        let token = get_token(&pool, "google").await.unwrap().unwrap();
        assert_eq!(token.token, "access-token");
        assert_eq!(token.refresh_token, Some("refresh-secret".into()));
        assert_eq!(token.expires_at, Some("2025-12-31".into()));

        // Verify raw refresh_token is also encrypted.
        let row: (Option<String>,) =
            sqlx::query_as("SELECT refresh_token FROM auth_tokens WHERE provider = 'google'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_ne!(row.0.as_deref(), Some("refresh-secret"));
    }

    #[tokio::test]
    async fn get_nonexistent_token() {
        let pool = open_memory().await.unwrap();
        let result = get_token(&pool, "nope").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_token_works() {
        let pool = open_memory().await.unwrap();
        save_token(&pool, "anthropic", "key", None, None)
            .await
            .unwrap();
        delete_token(&pool, "anthropic").await.unwrap();
        assert!(get_token(&pool, "anthropic").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_providers_works() {
        let pool = open_memory().await.unwrap();
        save_token(&pool, "anthropic", "a", None, None)
            .await
            .unwrap();
        save_token(&pool, "openai", "b", None, None).await.unwrap();

        let providers = list_providers(&pool).await.unwrap();
        assert_eq!(providers, vec!["anthropic", "openai"]);
    }
}
