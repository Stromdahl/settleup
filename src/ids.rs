//! Random identifiers and token hashing. A device's raw token lives only in its
//! cookie; the server stores only the SHA-256 hash, so a database leak can't be
//! replayed to impersonate a member.

use rand::Rng;
use rand::distr::Alphanumeric;
use sha2::{Digest, Sha256};

/// A random alphanumeric string of `len` characters.
pub fn random_token(len: usize) -> String {
    rand::rng()
        .sample_iter(Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// Short, URL-friendly group identifier used in the shareable link.
pub fn group_id() -> String {
    random_token(10)
}

/// A secret device token (owner or guest).
pub fn device_token() -> String {
    random_token(32)
}

/// Hex-encoded SHA-256 of a token, for storage and lookup.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// The per-group cookie name that carries this device's token.
pub fn cookie_name(group_id: &str) -> String {
    format!("su_{group_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_distinct_and_sized() {
        let a = device_token();
        let b = device_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_stable_and_hex() {
        let t = "hello";
        assert_eq!(hash_token(t), hash_token(t));
        assert_eq!(hash_token(t).len(), 64); // 32 bytes hex
        assert_ne!(hash_token("a"), hash_token("b"));
    }
}
