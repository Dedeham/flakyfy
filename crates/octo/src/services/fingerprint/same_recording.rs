//! Whether a downloaded file is the same recording as the library file it would replace.
//!
//! Better quality swaps a file for a lossless one. The file in the library is the one the person
//! trusts, so the candidate has to sound like it before it takes its place. A name, a length within
//! the download check's eight seconds and a lossless extension can all agree on another artist's song
//! with a similar title; the audio cannot. Both files are fingerprinted over up to three stretches (the
//! start, the middle and the end) and every stretch has to be alike: a remaster, a live take or a copy
//! at another speed often shares its start with the song and parts ways later.
//!
//! Only a clear match may replace. Anything else, including a comparison that could not be made, leaves
//! the original where it is: a failure here costs one upgrade, never the right song.

use std::path::Path;

use octo_core::settings::SoulseekSettings;
use octo_media::audio::AudioFingerprinter;
use octo_media::tags::TagFile;

use super::preview_check::informative_windows;

/// How much of a file each stretch covers.
pub const CLIP_SECONDS: i32 = 30;

/// How far one fingerprint may be slid against the other, in items (about twelve seconds), to absorb
/// a different intro, a leading silence or a trimmed start.
const MAX_SHIFT: usize = 100;

/// Fewer overlapping items than this (about five seconds) is too little to judge.
const MIN_OVERLAP: usize = 40;

/// How alike, in percent, a stretch has to be. Measured on real recordings: the same recording in any
/// encoding (MP3 128k, mono 64k, resampled, with a 5 s intro) scored 94 to 100; a copy sped up 3 %
/// scored 81 to 84, 6 % 72 to 83, 10 % 63 to 73; other pianists' performances of the same piece 58 to
/// 64; 609 pairs of different tracks 38 to 59.5 (median 52). A sped-up or slowed copy is another
/// master, not the file the person has.
pub const SAME_PERCENT: f64 = 85.0;

/// Below this, in percent, the audio is clearly something else; between it and [`SAME_PERCENT`] it may
/// be another version.
pub const DIFFERS_BELOW_PERCENT: f64 = 60.0;

/// Two lengths further apart than this are another cut of the song (an extended mix, a radio edit),
/// not an intro. A short intro on one of them stays inside it, and the slide absorbs it.
const LENGTH_GAP_SECONDS: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SameRecording {
    /// The same recording, 0 to 1 alike.
    Same(f64),
    /// Clearly something else.
    Differs(f64),
    /// Alike enough to doubt: another version, a remaster, a live take, a copy at another speed.
    Unsure(f64),
    /// The lengths are too far apart for an intro: another cut of the song. The difference in seconds,
    /// positive when the candidate is the longer.
    LengthDiffers(i32),
    /// A fingerprint could not be made of one of the files, or there was nothing in the original to
    /// compare with.
    CannotTell,
}

impl SameRecording {
    pub fn from_similarity(similarity: f64) -> Self {
        let percent = similarity * 100.0;
        if percent >= SAME_PERCENT {
            Self::Same(similarity)
        } else if percent < DIFFERS_BELOW_PERCENT {
            Self::Differs(similarity)
        } else {
            Self::Unsure(similarity)
        }
    }

    /// Only a clear match may take the original's place.
    pub fn allows_replacing(self) -> bool {
        matches!(self, Self::Same(_))
    }

    /// Why the original stays, for the journal and the log.
    pub fn refusal(self) -> String {
        let percent = |similarity: f64| (similarity * 100.0).round();
        match self {
            Self::Same(_) => "is the same recording".to_string(),
            Self::Differs(s) => format!(
                "does not sound like the song it would replace ({}% alike)",
                percent(s)
            ),
            Self::Unsure(s) => format!(
                "may be another version of the song it would replace ({}% alike), so the original stays",
                percent(s)
            ),
            Self::LengthDiffers(seconds) => format!(
                "is {}s {} than the song it would replace, so it may be another cut and the original stays",
                seconds.abs(),
                if seconds > 0 { "longer" } else { "shorter" }
            ),
            Self::CannotTell => {
                "could not be compared with the song it would replace, so the original stays".to_string()
            }
        }
    }
}

/// Whether the stretch has enough in it to be compared: silence, a pure tone or a steady hum
/// fingerprints to one number over and over, and two such stretches look alike whatever they are.
fn informative(items: &[u32]) -> bool {
    !items.is_empty() && informative_windows(items, items.len()).first() == Some(&true)
}

/// The best similarity, 0 to 1, over every slide of `subject` against `reference` within `max_shift`
/// items. None when either is too short to compare, or when the reference has nothing in it to say what
/// the subject should sound like. A subject with nothing in it against a reference that has sound is as
/// different as it gets: 0.
pub fn similarity(subject: &[u32], reference: &[u32], max_shift: usize) -> Option<f64> {
    if subject.len().min(reference.len()) >= MIN_OVERLAP {
        match (informative(subject), informative(reference)) {
            (_, false) => return None,
            (false, true) => return Some(0.0),
            (true, true) => {}
        }
    }
    let mut best: Option<f64> = None;
    for shift in 0..=max_shift {
        for (left, right) in [(subject, reference), (reference, subject)] {
            let Some(rest) = right.get(shift..) else {
                continue;
            };
            let overlap = left.len().min(rest.len());
            if overlap < MIN_OVERLAP {
                continue;
            }
            let differing: u32 = left[..overlap]
                .iter()
                .zip(&rest[..overlap])
                .map(|(x, y)| (x ^ y).count_ones())
                .sum();
            let score = 1.0 - f64::from(differing) / (overlap as f64 * 32.0);
            if best.is_none_or(|b| score > b) {
                best = Some(score);
            }
        }
    }
    best
}

/// Where the stretches of `clip` seconds start in a file of `shorter` seconds: the start, the middle and
/// the end, as far as the file is long enough for them not to overlap. The same times are used in both
/// files, so an intro on one of them is a constant offset the slide absorbs.
pub(crate) fn window_starts(shorter: i32, clip: i32) -> Vec<i32> {
    let last = shorter - clip;
    if clip <= 0 || last < clip {
        vec![0]
    } else if last < clip * 2 {
        vec![0, last]
    } else {
        vec![0, last / 2, last]
    }
}

fn length_of(path: &Path) -> Option<i32> {
    // 0 is a header that does not say, not a file of no length.
    TagFile::open(path)
        .ok()
        .map(|file| file.duration_seconds())
        .filter(|seconds| *seconds > 0)
}

/// `candidate` against `original`, in up to three stretches. Every stretch has to match, so the worst
/// one decides.
pub async fn same_recording(
    fingerprinter: &AudioFingerprinter,
    candidate: &Path,
    original: &Path,
    settings: &SoulseekSettings,
) -> SameRecording {
    let clip = settings.effective_fingerprint_seconds().min(CLIP_SECONDS);
    let timeout = settings.effective_fingerprint_timeout_seconds();
    let starts = match (length_of(candidate), length_of(original)) {
        (Some(new), Some(old)) => {
            if (new - old).abs() > LENGTH_GAP_SECONDS {
                return SameRecording::LengthDiffers(new - old);
            }
            window_starts(new.min(old), clip)
        }
        // The length is not known, so only the start is compared.
        _ => vec![0],
    };
    let mut worst: Option<f64> = None;
    for start in starts {
        let (new, old) = tokio::join!(
            fingerprinter.raw_fingerprint_window(candidate, start, clip, timeout),
            fingerprinter.raw_fingerprint_window(original, start, clip, timeout),
        );
        let (Some(new), Some(old)) = (new, old) else {
            return SameRecording::CannotTell;
        };
        // A stretch with nothing in it on the original's side (silence, a pure tone) cannot vouch for
        // anything, so the others decide; with none left nothing is vouched for.
        let Some(similarity) = similarity(&new.items, &old.items, MAX_SHIFT) else {
            continue;
        };
        worst = Some(worst.map_or(similarity, |w| w.min(similarity)));
    }
    worst.map_or(SameRecording::CannotTell, SameRecording::from_similarity)
}

#[cfg(test)]
#[path = "same_recording_tests.rs"]
mod tests;
