//! The decisions abilities lead to, as pure functions: what a heart does, how a library
//! stream is capped, whether downloads and shares are allowed, and what happens to a request.

use super::abilities::{Abilities, AddToLibrary, RequestQuality};
use super::device::NetworkPlace;

/// What a heart on a song does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeartAction {
    /// A library song: Navidrome stars it, as today.
    Relay,
    /// An outside song: Octo downloads it (into the member's own library for a member).
    Download,
    /// An outside song: Octo saves a link; no download.
    Save,
}

/// What a heart does for a caller. `abilities` is None for the owner and unmanaged accounts,
/// who keep today's behavior.
pub fn heart_action(abilities: Option<&Abilities>, is_outside: bool) -> HeartAction {
    if !is_outside {
        return HeartAction::Relay;
    }
    match abilities.map(|a| a.add_to_library) {
        None | Some(AddToLibrary::Direct) => HeartAction::Download,
        Some(AddToLibrary::Request | AddToLibrary::SaveOnly) => HeartAction::Save,
    }
}

/// How a library `stream` (or `download`) request is rewritten for a capped member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamRewrite {
    /// The format to ask Navidrome for (`opus` when capped).
    pub format: Option<&'static str>,
    /// The `maxBitRate` to ask for.
    pub max_bit_rate: Option<u32>,
    /// Remove `format=raw`, which skips every cap.
    pub drop_raw: bool,
}

/// Why a stream is refused, with the words the member reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Away from home with listening away turned off.
    Away,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Refusal::Away => "Listening away from home is off for this account.",
        }
    }
}

/// The cap a member streams at from `place`: the away cap when away and set, else the stream
/// cap. None means the original.
pub fn cap_for(a: &Abilities, place: NetworkPlace) -> Option<u32> {
    if place == NetworkPlace::Away && a.away_cap > 0 {
        return Some(a.away_cap);
    }
    a.stream_cap.kbps()
}

/// The rewrite for one library stream. `client_max` is the `maxBitRate` the client sent
/// (0 or absent means none); a lower one than the cap wins.
pub fn stream_rewrite(
    a: &Abilities,
    place: NetworkPlace,
    client_max: Option<u32>,
) -> Result<StreamRewrite, Refusal> {
    if place == NetworkPlace::Away && !a.away {
        return Err(Refusal::Away);
    }
    let Some(cap) = cap_for(a, place) else {
        return Ok(StreamRewrite::default());
    };
    let rate = match client_max.filter(|m| *m > 0) {
        Some(client) => client.min(cap),
        None => cap,
    };
    Ok(StreamRewrite {
        format: Some("opus"),
        max_bit_rate: Some(rate),
        drop_raw: true,
    })
}

/// `download` and zips.
pub fn may_download(a: &Abilities) -> bool {
    a.download_files
}

/// Share links.
pub fn may_share(a: &Abilities) -> bool {
    a.share
}

/// What becomes of a new request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestDecision {
    /// Not allowed, with the words the member reads.
    Refuse(String),
    /// Waits for an approver.
    Pending,
    /// Approved at once.
    AutoApprove,
}

/// `Some("Your library is full (10.2 of 10 GB)")` when the member's personal library holds
/// their storage limit or more; None when there is room or no limit.
pub fn storage_full(a: &Abilities, used_bytes: u64) -> Option<String> {
    let limit = a.storage_limit_bytes()?;
    (used_bytes >= limit).then(|| {
        format!(
            "Your library is full ({:.1} of {} GB)",
            used_bytes as f64 / 1_000_000_000.0,
            a.storage_limit_gb
        )
    })
}

/// The decision for a new request: above the member's highest quality or past the weekly
/// limit is refused; otherwise auto-approve when the member has it and their library has
/// room, else it waits. `used` is [`super::request::quota_used`].
pub fn request_decision(
    a: &Abilities,
    quality: RequestQuality,
    used: u32,
    storage_used_bytes: u64,
) -> RequestDecision {
    if quality.rank() > a.request_quality.rank() {
        return RequestDecision::Refuse(format!(
            "This account can ask for {} copies at most.",
            a.request_quality.label()
        ));
    }
    if a.weekly_request_limit > 0 && used >= a.weekly_request_limit {
        return RequestDecision::Refuse(format!(
            "You have used all {} of your requests this week.",
            a.weekly_request_limit
        ));
    }
    if a.auto_approve && storage_full(a, storage_used_bytes).is_none() {
        RequestDecision::AutoApprove
    } else {
        RequestDecision::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::family::abilities::{Preset, StreamCap};

    fn listener() -> Abilities {
        Abilities::preset(Preset::Listener)
    }

    #[test]
    fn listener_outside_heart_saves() {
        assert_eq!(heart_action(Some(&listener()), true), HeartAction::Save);
        let save_only = Abilities {
            add_to_library: AddToLibrary::SaveOnly,
            ..listener()
        };
        assert_eq!(heart_action(Some(&save_only), true), HeartAction::Save);
        assert_eq!(heart_action(Some(&listener()), false), HeartAction::Relay);
    }

    #[test]
    fn member_outside_heart_downloads() {
        let member = Abilities::preset(Preset::Member);
        assert_eq!(heart_action(Some(&member), true), HeartAction::Download);
        assert_eq!(heart_action(Some(&member), false), HeartAction::Relay);
    }

    #[test]
    fn owner_is_unchanged() {
        assert_eq!(heart_action(None, true), HeartAction::Download);
        assert_eq!(heart_action(None, false), HeartAction::Relay);
        let owner = Abilities::preset(Preset::Owner);
        for place in [NetworkPlace::Home, NetworkPlace::Away] {
            assert_eq!(
                stream_rewrite(&owner, place, Some(320)),
                Ok(StreamRewrite::default())
            );
        }
    }

    #[test]
    fn away_off_refuses() {
        let kid = Abilities::preset(Preset::Kid);
        assert_eq!(stream_rewrite(&kid, NetworkPlace::Away, None), Err(Refusal::Away));
        assert!(stream_rewrite(&kid, NetworkPlace::Home, None).is_ok());
    }

    #[test]
    fn away_cap_applies_only_away() {
        let home = stream_rewrite(&listener(), NetworkPlace::Home, None).expect("allowed");
        assert_eq!(home.max_bit_rate, Some(192));
        let away = stream_rewrite(&listener(), NetworkPlace::Away, None).expect("allowed");
        assert_eq!(away.max_bit_rate, Some(128));
        assert_eq!(away.format, Some("opus"));
        // An away cap of 0 means the stream cap away too.
        let same = Abilities {
            away_cap: 0,
            ..listener()
        };
        assert_eq!(
            stream_rewrite(&same, NetworkPlace::Away, None)
                .expect("allowed")
                .max_bit_rate,
            Some(192)
        );
        // An away cap alone caps only away.
        let away_only = Abilities {
            stream_cap: StreamCap::Original,
            away_cap: 96,
            ..listener()
        };
        assert_eq!(
            stream_rewrite(&away_only, NetworkPlace::Home, None),
            Ok(StreamRewrite::default())
        );
        assert_eq!(
            stream_rewrite(&away_only, NetworkPlace::Away, None)
                .expect("allowed")
                .max_bit_rate,
            Some(96)
        );
    }

    #[test]
    fn client_lower_bitrate_wins() {
        let lower = stream_rewrite(&listener(), NetworkPlace::Home, Some(96)).expect("allowed");
        assert_eq!(lower.max_bit_rate, Some(96));
        let higher = stream_rewrite(&listener(), NetworkPlace::Home, Some(320)).expect("allowed");
        assert_eq!(higher.max_bit_rate, Some(192));
        // maxBitRate=0 is "no limit" in Subsonic: the cap stands.
        let zero = stream_rewrite(&listener(), NetworkPlace::Home, Some(0)).expect("allowed");
        assert_eq!(zero.max_bit_rate, Some(192));
    }

    #[test]
    fn original_cap_rewrites_nothing() {
        let member = Abilities::preset(Preset::Member);
        let rewrite = stream_rewrite(&member, NetworkPlace::Away, Some(128)).expect("allowed");
        assert_eq!(rewrite, StreamRewrite::default());
        assert!(!rewrite.drop_raw);
        let capped = stream_rewrite(&listener(), NetworkPlace::Home, None).expect("allowed");
        assert!(capped.drop_raw);
    }

    #[test]
    fn downloads_and_shares_follow_abilities() {
        assert!(!may_download(&listener()));
        assert!(!may_share(&listener()));
        let member = Abilities::preset(Preset::Member);
        assert!(may_download(&member));
        assert!(may_share(&member));
    }

    #[test]
    fn flac_above_kid_max_is_refused() {
        let kid = Abilities::preset(Preset::Kid);
        let RequestDecision::Refuse(message) = request_decision(&kid, RequestQuality::Flac, 0, 0) else {
            panic!("refused");
        };
        assert!(message.contains("MP3"), "{message}");
        assert_eq!(
            request_decision(&kid, RequestQuality::Mp3, 0, 0),
            RequestDecision::Pending
        );
    }

    #[test]
    fn limit_reached_is_refused_with_count() {
        let RequestDecision::Refuse(message) = request_decision(&listener(), RequestQuality::Best, 10, 0)
        else {
            panic!("refused");
        };
        assert!(message.contains("10"), "{message}");
        assert_eq!(
            request_decision(&listener(), RequestQuality::Best, 9, 0),
            RequestDecision::Pending
        );
        // No limit: never refused for the count.
        let member = Abilities::preset(Preset::Member);
        assert_eq!(
            request_decision(&member, RequestQuality::Best, 10_000, 0),
            RequestDecision::AutoApprove
        );
    }

    #[test]
    fn auto_approve_under_limit() {
        let listener_auto = Abilities {
            auto_approve: true,
            ..listener()
        };
        assert_eq!(
            request_decision(&listener_auto, RequestQuality::Flac, 3, 0),
            RequestDecision::AutoApprove
        );
    }

    #[test]
    fn a_full_library_waits_for_an_approver() {
        let member = Abilities::preset(Preset::Member); // 50 GB
        assert_eq!(storage_full(&member, 49_999_999_999), None);
        assert_eq!(
            storage_full(&member, 51_200_000_000).as_deref(),
            Some("Your library is full (51.2 of 50 GB)")
        );
        assert_eq!(
            request_decision(&member, RequestQuality::Best, 0, 60_000_000_000),
            RequestDecision::Pending
        );
        let owner = Abilities::preset(Preset::Owner);
        assert_eq!(storage_full(&owner, u64::MAX), None);
    }
}
