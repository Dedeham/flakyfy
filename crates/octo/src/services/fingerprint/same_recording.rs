//! Whether a downloaded file is the same recording as the library file it would replace.
//!
//! Better quality swaps a lossy file for a lossless one. The lossy file is the one the person
//! trusts (often the YouTube copy of a heart), so the lossless candidate has to sound like it before
//! it takes its place. Name, length and AcoustID can all agree on a wrong file; the audio cannot.
//! Both are fingerprinted over their first half minute and compared by bit error rate.
//!
//! Only a clear match may replace. Anything else, including a comparison that could not be made,
//! leaves the original where it is: a failure here costs one upgrade, never the right song.

use std::path::Path;

use octo_core::fingerprint::fingerprint_compare::{self, MAX_SHIFT};
use octo_core::settings::SoulseekSettings;
use octo_media::audio::AudioFingerprinter;
use octo_media::tags::TagFile;

use super::you_tube_cross_check::CLIP_SECONDS;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SameRecording {
    /// The same recording, 0 to 1 alike.
    Same(f64),
    /// Clearly something else.
    Differs(f64),
    /// Alike enough to doubt: another version, a remaster, a live take, a copy at another speed.
    Unsure(f64),
    /// The lengths are too far apart for an intro: another cut of the song. The difference in
    /// seconds, positive when the candidate is the longer.
    LengthDiffers(i32),
    /// A fingerprint could not be made of one of the files.
    CannotTell,
}

/// How alike, in percent, a candidate has to be to take a trusted file's place. Stricter than the
/// YouTube comparison's 70, because a wrong swap costs more than a missed upgrade. Measured on real
/// recordings: the same recording in any encoding (MP3 128k, mono 64k, resampled, with a 5 s intro)
/// scored 94 to 100%; a copy sped up 3% scored 81 to 84%, 6% scored 72 to 83%, 10% 63 to 73%; other
/// pianists' performances of the same piece scored 58 to 64%; speech against piano 53%. A sped-up
/// or slowed copy is another master, not the file the person has.
pub const SAME_PERCENT: f64 = 85.0;

impl SameRecording {
    pub fn from_similarity(similarity: f64) -> Self {
        let percent = similarity * 100.0;
        if percent >= SAME_PERCENT {
            Self::Same(similarity)
        } else if percent < fingerprint_compare::DIFFERS_BELOW_PERCENT {
            Self::Differs(similarity)
        } else {
            Self::Unsure(similarity)
        }
    }

    /// Only a clear match may take the original's place.
    pub fn allows_replacing(self) -> bool {
        matches!(self, Self::Same(_))
    }

    /// Whether the audio itself says it is another song or version. A comparison that could not be
    /// made says nothing, so a candidate is not thrown away for it.
    pub fn is_another_sound(self) -> bool {
        matches!(self, Self::Differs(_) | Self::Unsure(_) | Self::LengthDiffers(_))
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

/// Two lengths further apart than this are another cut of the song (an extended mix, a radio edit),
/// not an intro. A short intro on a YouTube copy, a few seconds, is the same song and stays inside
/// it; the fingerprint slide (`MAX_SHIFT`, about twelve seconds) absorbs it.
const LENGTH_GAP_SECONDS: i32 = 20;

/// Where the windows of `clip` seconds start in a file of `shorter` seconds: the start, the middle and
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

/// `candidate` against `original`, in up to three windows. Every window has to match: a remaster or
/// a live take often shares its intro with the song and parts ways later.
pub async fn same_recording(
    fingerprinter: &AudioFingerprinter,
    candidate: &Path,
    original: &Path,
    settings: &SoulseekSettings,
) -> SameRecording {
    let clip = settings.effective_fingerprint_seconds().min(CLIP_SECONDS);
    let timeout = settings.effective_fingerprint_timeout_seconds();
    let lengths = (length_of(candidate), length_of(original));
    let starts = match lengths {
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
            fingerprinter.fingerprint_raw_window(candidate, start, clip, timeout),
            fingerprinter.fingerprint_raw_window(original, start, clip, timeout),
        );
        let (Some(new), Some(old)) = (new, old) else {
            return SameRecording::CannotTell;
        };
        // A window with nothing in it on the original's side (silence, a pure tone) cannot vouch for
        // anything, so the others decide; with none left nothing is vouched for.
        let Some(similarity) = fingerprint_compare::similarity(&new, &old, MAX_SHIFT) else {
            continue;
        };
        worst = Some(worst.map_or(similarity, |w| w.min(similarity)));
    }
    worst.map_or(SameRecording::CannotTell, SameRecording::from_similarity)
}

#[cfg(test)]
#[path = "same_recording_tests.rs"]
mod tests;
