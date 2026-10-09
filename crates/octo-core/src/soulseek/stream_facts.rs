//! What Octo knows of the stream it relays for an outside song: the format YouTube serves it in
//! (the itag), its container, its bit rate and its exact size in bytes.

/// The facts of one video's audio stream. Every field is optional: each is filled when an
/// answer named it, and none is ever guessed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamFacts {
    /// The exact byte count of the whole stream.
    pub size: Option<u64>,
    /// YouTube's format id: "140" is AAC in mp4, "251" Opus in webm.
    pub itag: Option<String>,
    /// The container, as yt-dlp names it ("m4a", "webm").
    pub ext: Option<String>,
    /// The average bit rate in kbps.
    pub bit_rate: Option<i32>,
}

/// What a client is told about the stream: the suffix and content type it picks a decoder
/// from, and the bit rate when it is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredStream {
    pub suffix: String,
    pub content_type: String,
    pub bit_rate: Option<i32>,
}

impl StreamFacts {
    /// Whether nothing is known.
    pub fn is_empty(&self) -> bool {
        self.size.is_none() && self.itag.is_none() && self.ext.is_none() && self.bit_rate.is_none()
    }

    /// Fills what `other` knows; what it does not know is kept.
    pub fn merge(&mut self, other: &StreamFacts) {
        if other.size.is_some() {
            self.size = other.size;
        }
        if other.itag.is_some() {
            self.itag = other.itag.clone();
        }
        if other.ext.is_some() {
            self.ext = other.ext.clone();
        }
        if other.bit_rate.is_some() {
            self.bit_rate = other.bit_rate;
        }
    }

    /// The suffix, content type and bit rate that match the served format, or None when neither
    /// the format nor the container is known. Format 140 is always AAC at 128 kbps in mp4;
    /// anything else takes its container's name and its measured bit rate, if any.
    pub fn declared(&self) -> Option<DeclaredStream> {
        let itag = self.itag.as_deref().map(str::trim).filter(|i| !i.is_empty());
        let bit_rate = self.bit_rate.filter(|rate| *rate > 0);
        match itag {
            Some("140") => Some(DeclaredStream {
                suffix: "m4a".into(),
                content_type: "audio/mp4".into(),
                bit_rate: Some(128),
            }),
            Some("251") => Some(DeclaredStream {
                suffix: "webm".into(),
                content_type: "audio/webm".into(),
                bit_rate,
            }),
            _ => {
                let ext = self
                    .ext
                    .as_deref()
                    .map(|e| e.trim().to_ascii_lowercase())
                    .filter(|e| !e.is_empty())?;
                let content_type = match ext.as_str() {
                    "m4a" | "mp4" | "aac" => "audio/mp4",
                    "webm" => "audio/webm",
                    "ogg" | "opus" => "audio/ogg",
                    "mp3" => "audio/mpeg",
                    _ => "application/octet-stream",
                };
                Some(DeclaredStream {
                    suffix: ext,
                    content_type: content_type.into(),
                    bit_rate,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(itag: Option<&str>, ext: Option<&str>, bit_rate: Option<i32>) -> StreamFacts {
        StreamFacts {
            size: Some(1),
            itag: itag.map(String::from),
            ext: ext.map(String::from),
            bit_rate,
        }
    }

    #[test]
    fn format_140_is_declared_as_m4a_at_128() {
        let declared = facts(Some("140"), Some("m4a"), Some(129))
            .declared()
            .expect("known");
        assert_eq!(
            (
                declared.suffix.as_str(),
                declared.content_type.as_str(),
                declared.bit_rate
            ),
            ("m4a", "audio/mp4", Some(128))
        );
    }

    #[test]
    fn format_251_is_declared_as_webm_with_its_measured_rate_or_none() {
        let declared = facts(Some("251"), None, Some(160)).declared().expect("known");
        assert_eq!(
            (
                declared.suffix.as_str(),
                declared.content_type.as_str(),
                declared.bit_rate
            ),
            ("webm", "audio/webm", Some(160))
        );
        assert_eq!(
            facts(Some("251"), None, None).declared().expect("known").bit_rate,
            None
        );
    }

    #[test]
    fn another_format_takes_its_containers_name_and_nothing_known_declares_nothing() {
        let declared = facts(Some("139"), Some("M4A"), None).declared().expect("known");
        assert_eq!(
            (
                declared.suffix.as_str(),
                declared.content_type.as_str(),
                declared.bit_rate
            ),
            ("m4a", "audio/mp4", None)
        );
        assert_eq!(facts(Some("999"), None, Some(64)).declared(), None);
        assert_eq!(StreamFacts::default().declared(), None);
    }

    #[test]
    fn merging_fills_what_is_known_and_keeps_the_rest() {
        let mut known = facts(Some("140"), Some("m4a"), Some(129));
        known.merge(&StreamFacts {
            size: Some(6_755_585),
            ..Default::default()
        });
        assert_eq!(known.size, Some(6_755_585));
        assert_eq!(known.itag.as_deref(), Some("140"));
        assert!(StreamFacts::default().is_empty());
        assert!(!known.is_empty());
    }
}
