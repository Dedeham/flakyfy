//! Members' account passwords (the `/family/` page and Navidrome's web player): Argon2id
//! hashes, never stored or logged in clear.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

/// The shortest account password Octo accepts.
pub const MIN_LENGTH: usize = 8;

/// An Argon2id hash (default parameters, a random salt) in PHC form: `$argon2id$v=19$...`.
pub fn hash(password: &str) -> String {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        // Only a broken random source fails here; an empty hash then verifies nothing.
        .unwrap_or_default()
}

/// Whether `password` matches `hash`. Anything that is not a hash matches nothing.
pub fn verify(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Whether a new account password is long enough, with the words the member reads when not.
pub fn acceptable(password: &str) -> Result<(), &'static str> {
    if password.chars().count() < MIN_LENGTH {
        return Err("Use at least 8 characters.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_verifies() {
        let hashed = hash("correct horse");
        assert!(hashed.starts_with("$argon2id$v=19$"), "{hashed}");
        assert!(!hashed.contains("correct horse"));
        assert!(verify("correct horse", &hashed));
        // A fresh salt each time.
        assert_ne!(hash("correct horse"), hashed);
    }

    #[test]
    fn wrong_password_fails() {
        let hashed = hash("correct horse");
        assert!(!verify("Correct horse", &hashed));
        assert!(!verify("", &hashed));
        assert!(!verify("correct horse", ""));
        assert!(!verify("correct horse", "not a hash"));
    }

    #[test]
    fn short_password_is_refused() {
        assert!(acceptable("1234567").is_err());
        assert!(acceptable("12345678").is_ok());
        assert!(acceptable("").is_err());
    }
}
