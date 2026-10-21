//! Raw fingerprints are compared to tell a wrong file from the right song, so the cases that
//! matter are the two ends: the same audio must match even when slid, and unrelated audio must
//! not.

use super::*;

/// A deterministic stand-in for a fingerprint: a xorshift stream, so two seeds are unrelated.
fn stream(seed: u32, frames: usize) -> Vec<u32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        })
        .collect()
}

#[test]
fn similarity_identical_is_one() {
    let fp = stream(7, 400);
    assert_eq!(similarity(&fp, &fp, MAX_SHIFT), Some(1.0));
}

#[test]
fn similarity_slid_copy_is_still_found() {
    let fp = stream(7, 400);
    let slid = fp[30..].to_vec();
    assert_eq!(similarity(&fp, &slid, MAX_SHIFT), Some(1.0));
    assert_eq!(similarity(&slid, &fp, MAX_SHIFT), Some(1.0));
}

#[test]
fn similarity_unrelated_is_near_half() {
    let score = similarity(&stream(7, 400), &stream(99, 400), MAX_SHIFT).expect("long enough");
    assert!((0.45..0.56).contains(&score), "{score}");
}

#[test]
fn similarity_too_short_is_none() {
    assert_eq!(similarity(&stream(7, 10), &stream(7, 400), MAX_SHIFT), None);
    assert_eq!(similarity(&[], &[], MAX_SHIFT), None);
}

#[test]
fn similarity_a_few_flipped_bits_still_matches() {
    let fp = stream(7, 400);
    let noisy: Vec<u32> = fp.iter().map(|x| x ^ 0x0101_0101).collect();
    let score = similarity(&fp, &noisy, MAX_SHIFT).expect("long enough");
    assert!(score > 0.85, "{score}");
}

#[test]
fn judge_splits_match_unsure_and_differs() {
    assert_eq!(judge(0.90, 70), CrossCheck::Match);
    assert_eq!(judge(0.70, 70), CrossCheck::Match);
    assert_eq!(judge(0.65, 70), CrossCheck::Unsure);
    assert_eq!(judge(0.60, 70), CrossCheck::Unsure);
    assert_eq!(judge(0.59, 70), CrossCheck::Differs);
    assert_eq!(judge(0.50, 70), CrossCheck::Differs);
}
