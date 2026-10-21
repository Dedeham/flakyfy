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

// A window of silence or a steady tone fingerprints to a few repeated numbers and looks like any
// other. Such a window says nothing, and a file that is silent where the reference is not is not it.

#[test]
fn silence_and_a_steady_tone_are_not_informative_but_music_is() {
    assert!(!is_informative(&[]));
    assert!(!is_informative(&[7; 200]));
    assert!(is_informative(&stream(7, 200)));
}

#[test]
fn a_short_pause_is_informative_and_most_of_a_window_silent_is_not() {
    let mut frames = stream(7, 200);
    frames[..50].fill(0);
    assert!(is_informative(&frames), "a quarter silent");
    frames[..70].fill(0);
    assert!(!is_informative(&frames), "over a third silent");
}

#[test]
fn a_silent_file_against_music_is_as_different_as_it_gets() {
    assert_eq!(similarity(&[0; 200], &stream(7, 200), MAX_SHIFT), Some(0.0));
}

#[test]
fn a_reference_with_nothing_in_it_says_nothing() {
    assert_eq!(similarity(&stream(7, 200), &[0; 200], MAX_SHIFT), None);
    assert_eq!(similarity(&[0; 200], &[0; 200], MAX_SHIFT), None);
    assert_eq!(similarity(&[5; 200], &[9; 200], MAX_SHIFT), None);
}
