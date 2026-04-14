use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

const CODE_VERIFIER_BYTES: usize = 64;
const STATE_BYTES: usize = 32;

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

fn random_base64url(byte_len: usize) -> String {
    let mut bytes = vec![0_u8; byte_len];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
