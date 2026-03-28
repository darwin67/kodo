use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};

/// Derive a 256-bit encryption key from machine-specific fingerprint data.
///
/// The key is derived from:
/// - hostname
/// - current user name
/// - a fixed salt specific to kodo
///
/// This is not intended to be unbreakable — it prevents casual reading of
/// tokens from the DB file. For stronger protection, OS keychain integration
/// should be added in the future.
pub fn derive_machine_key() -> Key<Aes256Gcm> {
    let hostname = hostname();
    let username = username();

    let mut hasher = Sha256::new();
    hasher.update(b"kodo-token-encryption-v1:");
    hasher.update(hostname.as_bytes());
    hasher.update(b":");
    hasher.update(username.as_bytes());

    let hash = hasher.finalize();
    *Key::<Aes256Gcm>::from_slice(&hash)
}

/// Encrypt a plaintext string. Returns a base64-encoded string containing
/// the nonce (12 bytes) prepended to the ciphertext.
pub fn encrypt(plaintext: &str, key: &Key<Aes256Gcm>) -> Result<String> {
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    // Prepend nonce to ciphertext: [nonce (12 bytes)][ciphertext...]
    let mut combined = Vec::with_capacity(nonce.len() + ciphertext.len());
    combined.extend_from_slice(&nonce);
    combined.extend_from_slice(&ciphertext);

    Ok(BASE64.encode(&combined))
}

/// Decrypt a base64-encoded string that was produced by `encrypt()`.
/// Returns the original plaintext.
pub fn decrypt(encoded: &str, key: &Key<Aes256Gcm>) -> Result<String> {
    let combined = BASE64.decode(encoded).context("failed to decode base64")?;

    if combined.len() < 13 {
        // 12 bytes nonce + at least 1 byte ciphertext
        bail!("encrypted data too short");
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new(key);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))?;

    String::from_utf8(plaintext).context("decrypted data is not valid UTF-8")
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| {
            std::process::Command::new("hostname")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_else(|_| "unknown-host".to_string())
}

fn username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown-user".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_key_is_deterministic() {
        let k1 = derive_machine_key();
        let k2 = derive_machine_key();
        assert_eq!(k1, k2);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = derive_machine_key();
        let plaintext = "sk-ant-api-key-12345-secret";
        let encrypted = encrypt(plaintext, &key).unwrap();

        // Encrypted output should be base64 and different from plaintext.
        assert_ne!(encrypted, plaintext);
        assert!(encrypted.len() > plaintext.len());

        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_produces_different_ciphertexts() {
        // Due to random nonce, encrypting the same plaintext twice
        // should produce different ciphertexts.
        let key = derive_machine_key();
        let e1 = encrypt("same-text", &key).unwrap();
        let e2 = encrypt("same-text", &key).unwrap();
        assert_ne!(e1, e2);

        // Both should decrypt to the same value.
        assert_eq!(decrypt(&e1, &key).unwrap(), "same-text");
        assert_eq!(decrypt(&e2, &key).unwrap(), "same-text");
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let key = derive_machine_key();
        let encrypted = encrypt("secret", &key).unwrap();

        // Create a different key.
        let mut hasher = Sha256::new();
        hasher.update(b"wrong-key-material");
        let wrong_hash = hasher.finalize();
        let wrong_key = Key::<Aes256Gcm>::from_slice(&wrong_hash);

        let result = decrypt(&encrypted, wrong_key);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_invalid_base64_fails() {
        let key = derive_machine_key();
        let result = decrypt("not-valid-base64!!!", &key);
        assert!(result.is_err());
    }

    #[test]
    fn decrypt_too_short_fails() {
        let key = derive_machine_key();
        let short = BASE64.encode(b"short");
        let result = decrypt(&short, &key);
        assert!(result.is_err());
    }

    #[test]
    fn encrypt_empty_string() {
        let key = derive_machine_key();
        let encrypted = encrypt("", &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn encrypt_unicode() {
        let key = derive_machine_key();
        let plaintext = "API key: 🔑 très secret";
        let encrypted = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
