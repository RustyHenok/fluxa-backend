//! Shared helpers for opaque, single-use security tokens (invitations,
//! email verification, password resets). Tokens are random 256-bit values;
//! only their SHA-256 hex digest is persisted.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::{generate_token, hash_token};

    #[test]
    fn tokens_are_unique_and_hash_deterministically() {
        let first = generate_token();
        let second = generate_token();
        assert_ne!(first, second);
        assert_eq!(hash_token(&first), hash_token(&first));
        assert_ne!(hash_token(&first), hash_token(&second));
    }
}
