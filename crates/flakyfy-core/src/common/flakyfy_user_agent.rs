//! How Flakyfy introduces itself to the open services that ask to be told who is calling: LRCLIB,
//! MusicBrainz and its Cover Art Archive, and AcoustID submissions. A generic user agent is what
//! those services throttle first.

use std::sync::LazyLock;

/// The release this build is ([`crate::VERSION`]), without any `+build` suffix, or "dev" when
/// there is none.
pub fn version() -> &'static str {
    static VERSION: LazyLock<&'static str> = LazyLock::new(|| match crate::VERSION.split('+').next() {
        Some(version) if !version.is_empty() => version,
        _ => "dev",
    });
    &VERSION
}

/// The User-Agent header value: `Flakyfy/{version} (+https://github.com/Dedeham/flakyfy)`.
pub fn value() -> &'static str {
    static VALUE: LazyLock<String> =
        LazyLock::new(|| format!("Flakyfy/{} (+https://github.com/Dedeham/flakyfy)", version()));
    &VALUE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_flakyfy_and_its_release() {
        assert_eq!(version(), crate::VERSION.split('+').next().expect("a version"));
        assert_eq!(
            value(),
            format!("Flakyfy/{} (+https://github.com/Dedeham/flakyfy)", version())
        );
    }
}
