//! The gate decides which files are ever compared. Getting it wrong either rejects a file AcoustID
//! already vouched for, or leaves a noise file through.

use super::*;

fn settings(verify: bool, cross_check: bool) -> SoulseekSettings {
    SoulseekSettings {
        verify_downloads: verify,
        youtube_cross_check: cross_check,
        ..Default::default()
    }
}

fn verdict(kind: VerificationVerdict, reason: InconclusiveReason) -> VerificationResult {
    VerificationResult {
        verdict: kind,
        reason,
        ..Default::default()
    }
}

#[test]
fn wanted_when_acoust_id_had_no_say() {
    for reason in [
        InconclusiveReason::NoEntry,
        InconclusiveReason::BelowThreshold,
        InconclusiveReason::LookupFailed,
    ] {
        let v = verdict(VerificationVerdict::Inconclusive, reason);
        assert!(wanted(&v, &settings(true, true)), "{reason}");
    }
}

#[test]
fn wanted_never_when_acoust_id_decided_or_cannot_help() {
    assert!(!wanted(
        &verdict(VerificationVerdict::Confirmed, InconclusiveReason::None),
        &settings(true, true)
    ));
    assert!(!wanted(
        &verdict(VerificationVerdict::Mismatch, InconclusiveReason::None),
        &settings(true, true)
    ));
    for reason in [
        InconclusiveReason::Disabled,
        InconclusiveReason::NotFingerprinted,
        InconclusiveReason::SourceDisagreed,
        InconclusiveReason::SoundsLikeAnother,
        InconclusiveReason::LengthOff,
    ] {
        let v = verdict(VerificationVerdict::Inconclusive, reason);
        assert!(!wanted(&v, &settings(true, true)), "{reason}");
    }
}

#[test]
fn wanted_needs_both_switches() {
    let v = verdict(VerificationVerdict::Inconclusive, InconclusiveReason::NoEntry);
    assert!(!wanted(&v, &settings(false, true)));
    assert!(!wanted(&v, &settings(true, false)));
}
