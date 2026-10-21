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

/// Points below the threshold where a difference stops being doubtful and becomes certain.
const DOUBT_MARGIN: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossCheck {
    /// The same recording.
    Match,
    /// Clearly not the same audio; the file is not the song.
    Differs,
    /// Close enough to doubt, not far enough to reject: another version, a remaster, a live take.
    Unsure,
}

/// The best similarity, 0 to 1, over every slide of one fingerprint against the other within
/// `max_shift` frames. None when either is too short to compare.
pub fn similarity(a: &[u32], b: &[u32], max_shift: usize) -> Option<f64> {
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

/// What a similarity means against a threshold given in percent.
pub fn judge(similarity: f64, threshold_percent: i32) -> CrossCheck {
    let percent = similarity * 100.0;
    if percent >= f64::from(threshold_percent) {
        CrossCheck::Match
    } else if percent < f64::from(threshold_percent - DOUBT_MARGIN) {
        CrossCheck::Differs
    } else {
        CrossCheck::Unsure
    }
}

#[cfg(test)]
#[path = "fingerprint_compare_tests.rs"]
mod tests;
