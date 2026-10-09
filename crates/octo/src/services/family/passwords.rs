//! Members' account passwords (the `/family/` page and Navidrome's web player): Argon2id
//! hashes, never stored or logged in clear.

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

/// The shortest account password Octo accepts.
pub const MIN_LENGTH: usize = 8;
/// The longest password Octo checks or keeps, in bytes. Anything longer is wrong without being
/// hashed, so a huge password cannot tie up a hashing slot.
pub const MAX_BYTES: usize = 1024;
/// How long a sign-in waits for a hashing slot before it is told to try again shortly.
pub const SLOT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// The words for a sign-in that found every hashing slot busy for [`SLOT_WAIT`].
pub const BUSY: &str = "Too many sign-ins right now, try again shortly.";

/// Every hashing slot stayed busy for [`SLOT_WAIT`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

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
    if password.len() > MAX_BYTES {
        return false;
    }
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// How many Argon2 hashes or checks run at once. Each takes about 19 MiB and a core for a
/// while, so a flood of sign-ins waits its turn instead of running the server out of memory.
pub const AT_ONCE: usize = 2;

static ARGON_SLOTS: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(AT_ONCE));

/// A hashing slot, waiting at most `wait` for one.
async fn slot(wait: std::time::Duration) -> Result<tokio::sync::SemaphorePermit<'static>, Busy> {
    match tokio::time::timeout(wait, ARGON_SLOTS.acquire()).await {
        Ok(Ok(permit)) => Ok(permit),
        _ => Err(Busy),
    }
}

/// [`verify`] on a blocking thread, at most [`AT_ONCE`] at a time. [`Busy`] when no slot came
/// free within [`SLOT_WAIT`]. A password over [`MAX_BYTES`] is wrong without waiting.
pub async fn verify_async(password: &str, hash: &str) -> Result<bool, Busy> {
    verify_waiting(password, hash, SLOT_WAIT).await
}

async fn verify_waiting(password: &str, hash: &str, wait: std::time::Duration) -> Result<bool, Busy> {
    if password.len() > MAX_BYTES {
        return Ok(false);
    }
    let _slot = slot(wait).await?;
    let (password, hash) = (password.to_string(), hash.to_string());
    Ok(tokio::task::spawn_blocking(move || verify(&password, &hash))
        .await
        .unwrap_or(false))
}

/// [`hash`] on a blocking thread, at most [`AT_ONCE`] at a time. [`Busy`] when no slot came
/// free within [`SLOT_WAIT`]; empty when it could not run.
pub async fn hash_async(password: &str) -> Result<String, Busy> {
    let _slot = slot(SLOT_WAIT).await?;
    let password = password.to_string();
    Ok(tokio::task::spawn_blocking(move || hash(&password))
        .await
        .unwrap_or_default())
}

/// Whether a new account password is long enough, with the words the member reads when not.
pub fn acceptable(password: &str) -> Result<(), &'static str> {
    if password.chars().count() < MIN_LENGTH {
        return Err("Use at least 8 characters.");
    }
    if password.len() > MAX_BYTES {
        return Err("Use a shorter password: at most 1024 characters.");
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

    #[tokio::test]
    async fn checks_run_a_few_at_a_time_and_still_answer() {
        let hashed = hash_async("correct horse").await.expect("a slot");
        assert!(hashed.starts_with("$argon2id$"), "{hashed}");
        let checks = (0..AT_ONCE * 3).map(|n| {
            let hashed = hashed.clone();
            async move {
                let typed = if n % 2 == 0 {
                    "correct horse"
                } else {
                    "wrong horse"
                };
                verify_async(typed, &hashed).await.expect("a slot")
            }
        });
        let answers = futures::future::join_all(checks).await;
        assert_eq!(
            answers.iter().filter(|ok| **ok).count(),
            (AT_ONCE * 3).div_ceil(2)
        );
    }

    #[test]
    fn short_password_is_refused() {
        assert!(acceptable("1234567").is_err());
        assert!(acceptable("12345678").is_ok());
        assert!(acceptable("").is_err());
    }

    #[test]
    fn a_password_over_the_cap_is_refused_and_never_matches() {
        let long = "x".repeat(MAX_BYTES + 1);
        assert!(acceptable(&long).is_err());
        assert!(acceptable(&"x".repeat(MAX_BYTES)).is_ok());
        // A hash of the long password, made directly, still never matches it.
        let hashed = hash(&long);
        assert!(!verify(&long, &hashed));
    }

    #[tokio::test]
    async fn a_check_that_waits_too_long_for_a_slot_is_told_to_try_again() {
        let hashed = hash("correct horse");
        let held: Vec<_> = futures::future::join_all((0..AT_ONCE).map(|_| ARGON_SLOTS.acquire()))
            .await
            .into_iter()
            .map(|permit| permit.expect("a slot"))
            .collect();
        let waited = verify_waiting("correct horse", &hashed, std::time::Duration::from_millis(50)).await;
        assert_eq!(waited, Err(Busy));
        // A password over the cap is answered at once, slots or not.
        let long = "x".repeat(MAX_BYTES + 1);
        assert_eq!(
            verify_waiting(&long, &hashed, std::time::Duration::from_millis(50)).await,
            Ok(false)
        );
        drop(held);
        assert_eq!(
            verify_waiting("correct horse", &hashed, std::time::Duration::from_secs(30)).await,
            Ok(true)
        );
    }
}
