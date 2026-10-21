//! Whether two raw Chromaprint fingerprints are the same recording, by bit error rate.
//!
//! AcoustID can only judge what it has heard. When it has not heard a file, this compares the
//! file against the official YouTube Music copy of the same song instead. Raw Chromaprint is one
//! `u32` per frame, so identical audio differs in few bits and unrelated audio (a different song,
//! noise) differs in about half of them.

/// Fewer overlapping frames than this (about five seconds) is too little to judge.
const MIN_OVERLAP: usize = 40;

/// How far one fingerprint may be slid against the other, in frames (about twelve seconds), to
/// absorb a different intro, a leading silence or a trimmed start.
pub const MAX_SHIFT: usize = 100;

/// At or above this many percent two fingerprints are the same recording; unrelated audio scores
/// about 50, so a gap of ten points under it is where doubt ends and rejecting is safe.
const MATCH_PERCENT: f64 = 70.0;
pub const DIFFERS_BELOW_PERCENT: f64 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossCheck {
    /// The same recording.
    Match,
    /// Clearly not the same audio; the file is not the song.
    Differs,
    /// Close enough to doubt, not far enough to reject: another version, a remaster, a live take.
    Unsure,
}

/// At least this share of a window's frames have to be different numbers for it to say anything.
/// Silence, a pure tone or a steady hum fingerprints to a few repeated numbers, and two such windows
/// look alike whatever they are: on generated audio silence against a 440 Hz tone scored 94% and a
/// 440 Hz against an 880 Hz tone 100%. Measured on 83 windows of real music, the lowest share was
/// 84% (median 99%); 10 s of silence in a 30 s window leaves 72%, 20 s leaves 36%. At 70% the
/// frames that match only for being silent lift an unrelated pair to about 66% at the most, under
/// every decision line.
const MIN_DISTINCT_SHARE: f64 = 0.7;

/// Whether the window has enough in it to be compared (see [`MIN_DISTINCT_SHARE`]).
pub fn is_informative(frames: &[u32]) -> bool {
    if frames.is_empty() {
        return false;
    }
    let mut distinct = frames.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    distinct.len() as f64 >= frames.len() as f64 * MIN_DISTINCT_SHARE
}

/// The best similarity, 0 to 1, over every slide of `subject` against `reference` within
/// `max_shift` frames. None when either is too short to compare, or when the reference has nothing
/// in it to say what the subject should sound like. A subject with nothing in it against a
/// reference that has sound is as different as it gets: 0.
pub fn similarity(subject: &[u32], reference: &[u32], max_shift: usize) -> Option<f64> {
    if subject.len().min(reference.len()) >= MIN_OVERLAP {
        match (is_informative(subject), is_informative(reference)) {
            (_, false) => return None,
            (false, true) => return Some(0.0),
            (true, true) => {}
        }
    }
    let (a, b) = (subject, reference);
    let mut best: Option<f64> = None;
    for shift in 0..=max_shift {
        for (left, right) in [(a, b), (b, a)] {
            let Some(rest) = right.get(shift..) else { continue };
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

/// What a similarity, 0 to 1, means.
pub fn judge(similarity: f64) -> CrossCheck {
    let percent = similarity * 100.0;
    if percent >= MATCH_PERCENT {
        CrossCheck::Match
    } else if percent < DIFFERS_BELOW_PERCENT {
        CrossCheck::Differs
    } else {
        CrossCheck::Unsure
    }
}

#[cfg(test)]
#[path = "fingerprint_compare_tests.rs"]
mod tests;
