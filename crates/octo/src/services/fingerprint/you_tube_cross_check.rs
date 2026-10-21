//! Whether a download is the song it claims to be, judged against the official YouTube Music copy.
//!
//! AcoustID cannot judge a recording it has never heard, and such a file is kept on purpose
//! (obscure music is what Soulseek is for). That also lets through a file that is not music at all.
//! YouTube Music has nearly everything, so when AcoustID has no say, both are fingerprinted here
//! and compared by bit error rate. No reference found means no opinion: a wrong reference must
//! never cost a good file.

use std::path::Path;

use octo_core::fingerprint::fingerprint_compare::{self, CrossCheck, MAX_SHIFT};
use octo_core::fingerprint::{InconclusiveReason, VerificationResult, VerificationVerdict};
use octo_core::settings::SoulseekSettings;
use octo_media::audio::AudioFingerprinter;
use tracing::{debug, info};

use crate::services::you_tube::you_tube_resolver::YouTubeResolver;

#[derive(Debug, Clone, PartialEq)]
pub struct CrossCheckOutcome {
    /// The YouTube Music video the file was compared with.
    pub video_id: String,
    /// 0 to 1, how alike the two fingerprints are.
    pub similarity: f64,
    pub verdict: CrossCheck,
}

/// Whether AcoustID's answer left the file open to this check: it had no entry, nothing it knew
/// was sure enough, or it could not be asked (no key, or unreachable).
pub fn wanted(verdict: &VerificationResult, settings: &SoulseekSettings) -> bool {
    settings.verify_downloads
        && settings.youtube_cross_check
        && verdict.verdict == VerificationVerdict::Inconclusive
        && matches!(
            verdict.reason,
            InconclusiveReason::NoEntry
                | InconclusiveReason::BelowThreshold
                | InconclusiveReason::LookupFailed
        )
}

/// The file against the official copy of `artist - title`. None when there is no confident copy
/// on YouTube Music, or either side could not be fingerprinted: no opinion.
pub async fn compare(
    fingerprinter: &AudioFingerprinter,
    youtube: &YouTubeResolver,
    path: &Path,
    artist: &str,
    title: &str,
    length: Option<i32>,
    settings: &SoulseekSettings,
) -> Option<CrossCheckOutcome> {
    let reference = youtube.music_song(artist, title, length).await?;
    let seconds = settings.effective_fingerprint_seconds();
    let timeout = settings.effective_fingerprint_timeout_seconds();
    let stream = format!("{}/stream?id={}", youtube.base_url(), reference.video_id);

    let Some(official) = fingerprinter
        .fingerprint_raw_from_url(&stream, seconds, timeout)
        .await
    else {
        debug!(
            "no fingerprint of YouTube video {} for '{artist} - {title}'",
            reference.video_id
        );
        return None;
    };
    let ours = fingerprinter.fingerprint_raw(path, seconds, timeout).await?;
    let similarity = fingerprint_compare::similarity(&ours, &official, MAX_SHIFT)?;
    let verdict = fingerprint_compare::judge(similarity, settings.effective_youtube_match_threshold());
    info!(
        "'{artist} - {title}' is {:.0}% alike to YouTube video {}: {verdict:?}",
        similarity * 100.0,
        reference.video_id
    );
    Some(CrossCheckOutcome {
        video_id: reference.video_id,
        similarity,
        verdict,
    })
}

#[cfg(test)]
#[path = "you_tube_cross_check_tests.rs"]
mod tests;
