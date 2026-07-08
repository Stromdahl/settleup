//! Passphrase hashing for the group recovery secret.
//!
//! Recovery passphrases guard owner takeover (a correct guess rotates the owner's
//! device token), and a group link is widely shared — so the stored secret must be
//! expensive to crack offline and must not collide across groups. We use
//! **argon2id** (memory-hard) with a freshly generated per-passphrase random salt,
//! storing the result as a self-describing PHC string (e.g.
//! `$argon2id$v=19$m=...,t=...,p=...$<salt>$<hash>`) in the existing `groups.recovery`
//! TEXT column — the salt and parameters travel with the hash, so there is no schema
//! change. Verification re-parses that PHC string; a value that isn't a valid PHC
//! string (such as a legacy unsalted SHA-256 hex from before this change) simply
//! fails to verify rather than panicking — pre-1.0 recovery hashes are throwaway.

use argon2::Argon2;
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{Error, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};

/// Hash a passphrase with argon2id (default params) and a fresh random salt,
/// returning the PHC string to store. The salt is embedded, so identical
/// passphrases in different groups produce different hashes.
pub fn hash_passphrase(pass: &str) -> Result<String, Error> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default().hash_password(pass.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verify `pass` against a stored PHC string. Returns `false` — never panics — if
/// `stored` cannot be parsed as a PHC hash (e.g. a legacy SHA-256 hex value) or if
/// the passphrase does not match.
pub fn verify_passphrase(pass: &str, stored: &str) -> bool {
    match PasswordHash::new(stored) {
        Ok(parsed) => Argon2::default()
            .verify_password(pass.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_round_trips() {
        let stored = hash_passphrase("correct horse battery staple").unwrap();
        assert!(
            stored.starts_with("$argon2id$"),
            "PHC string names argon2id"
        );
        assert!(verify_passphrase("correct horse battery staple", &stored));
    }

    #[test]
    fn wrong_passphrase_does_not_verify() {
        let stored = hash_passphrase("hunter2").unwrap();
        assert!(!verify_passphrase("hunter3", &stored));
    }

    #[test]
    fn same_passphrase_hashes_differ_by_salt() {
        let a = hash_passphrase("hunter2").unwrap();
        let b = hash_passphrase("hunter2").unwrap();
        assert_ne!(a, b, "a fresh random salt makes each hash unique");
    }

    #[test]
    fn legacy_non_phc_hash_fails_gracefully() {
        // A 64-char hex string like the old `ids::hash_token` produced is not a PHC
        // hash: verification must return false, not panic.
        let legacy = "0".repeat(64);
        assert!(!verify_passphrase("hunter2", &legacy));
    }
}
