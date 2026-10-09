//! The secrets members hold: app passwords for any Subsonic app, 6-digit pairing codes for
//! the Octo apps, device secrets and invite tokens, plus the comparisons and the Subsonic
//! token math that check them.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use md5::{Digest, Md5};

/// The letters an app password is made of: no 0, 1, I, L or O, which read alike.
pub const APP_PASSWORD_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// A new app password, `XXXX-XXXX-XXXX-XXXX` (16 letters from [`APP_PASSWORD_ALPHABET`],
/// about 79 bits).
pub fn app_password() -> String {
    let letters: Vec<char> = (0..16)
        .map(|_| char::from(APP_PASSWORD_ALPHABET[rand::random_range(0..APP_PASSWORD_ALPHABET.len())]))
        .collect();
    letters
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

/// An app password as typed, ready to compare: upper case, dashes and spaces gone.
pub fn normalize_app_password(text: &str) -> String {
    text.chars()
        .filter(|c| *c != '-' && !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// A new pairing code: 6 digits that never start with 0.
pub fn pair_code() -> String {
    rand::random_range(100_000u32..1_000_000).to_string()
}

/// A new invite or session token: 32 random bytes, URL-safe base64 without padding.
pub fn token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

/// A new device secret for a paired Octo app: 24 random bytes, 32 URL-safe characters.
pub fn device_secret() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>())
}

/// Compares two secrets in time that does not depend on where they differ. Lengths are not
/// secret.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Subsonic's token: lower-case hex of `md5(secret + salt)`.
pub fn subsonic_token(secret: &str, salt: &str) -> String {
    hex::encode(Md5::digest(format!("{secret}{salt}").as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_password_shape() {
        for _ in 0..50 {
            let password = app_password();
            assert_eq!(password.len(), 19, "{password}");
            let groups: Vec<&str> = password.split('-').collect();
            assert_eq!(groups.len(), 4);
            for group in groups {
                assert_eq!(group.len(), 4);
                assert!(
                    group.bytes().all(|b| APP_PASSWORD_ALPHABET.contains(&b)),
                    "{password}"
                );
            }
        }
        assert_ne!(app_password(), app_password());
    }

    #[test]
    fn normalize_accepts_lower_case_and_no_dashes() {
        let password = "ABCD-EFGH-JKMN-PQRS";
        for typed in [
            "abcd-efgh-jkmn-pqrs",
            "ABCDEFGHJKMNPQRS",
            "abcd efgh jkmn pqrs",
            " AbCd-EfGh-JkMn-PqRs ",
        ] {
            assert_eq!(
                normalize_app_password(typed),
                normalize_app_password(password),
                "{typed}"
            );
        }
    }

    #[test]
    fn pair_code_is_six_digits() {
        for _ in 0..200 {
            let code = pair_code();
            assert_eq!(code.len(), 6, "{code}");
            assert!(code.bytes().all(|b| b.is_ascii_digit()));
            assert_ne!(code.as_bytes()[0], b'0');
        }
    }

    #[test]
    fn tokens_and_secrets_are_long_and_url_safe() {
        let token = token();
        assert_eq!(token.len(), 43);
        let secret = device_secret();
        assert_eq!(secret.len(), 32);
        for text in [&token, &secret] {
            assert!(
                text.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            );
        }
        assert_ne!(device_secret(), device_secret());
    }

    #[test]
    fn constant_time_eq_compares_whole_values() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secrets"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn subsonic_token_matches_the_spec_example() {
        assert_eq!(
            subsonic_token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
    }
}
