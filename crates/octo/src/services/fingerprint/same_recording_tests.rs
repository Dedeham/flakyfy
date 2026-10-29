use std::path::{Path, PathBuf};

use octo_core::settings::SoulseekSettings;
use octo_media::audio::AudioFingerprinter;

use super::*;
use crate::services::fingerprint::preview_check::ITEM_SECONDS;

/// A fingerprint of `seconds` of made-up sound: the same seed is the same sound.
fn sound(seed: u64, seconds: f64) -> Vec<u32> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..(seconds / ITEM_SECONDS) as usize)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u32
        })
        .collect()
}

/// The same sound heard through another encoder: every item with `flips` of its bits turned over.
fn reencoded(items: &[u32], flips: u32) -> Vec<u32> {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| item ^ (0..flips).fold(0u32, |mask, k| mask | 1 << ((i as u32 * 7 + k * 5) % 32)))
        .collect()
}

#[test]
fn the_measured_lines_decide() {
    assert!(matches!(
        SameRecording::from_similarity(0.95),
        SameRecording::Same(_)
    ));
    assert!(matches!(
        SameRecording::from_similarity(0.85),
        SameRecording::Same(_)
    ));
    // A copy a few percent too fast scored 81 to 84: another master, not the file.
    assert!(matches!(
        SameRecording::from_similarity(0.83),
        SameRecording::Unsure(_)
    ));
    assert!(matches!(
        SameRecording::from_similarity(0.62),
        SameRecording::Unsure(_)
    ));
    assert!(matches!(
        SameRecording::from_similarity(0.52),
        SameRecording::Differs(_)
    ));
}

#[test]
fn only_a_clear_match_may_replace_and_everything_else_says_why_the_original_stays() {
    assert!(SameRecording::Same(0.97).allows_replacing());
    for other in [
        SameRecording::Differs(0.5),
        SameRecording::Unsure(0.8),
        SameRecording::LengthDiffers(-25),
        SameRecording::CannotTell,
    ] {
        assert!(!other.allows_replacing(), "{other:?}");
        assert!(!other.refusal().is_empty());
    }
    assert!(SameRecording::Differs(0.52).refusal().contains("52% alike"));
    assert!(
        SameRecording::LengthDiffers(-25)
            .refusal()
            .contains("25s shorter")
    );
}

#[test]
fn stretches_are_the_start_the_middle_and_the_end_as_far_as_the_file_reaches() {
    assert_eq!(window_starts(240, 30), [0, 105, 210]);
    assert_eq!(window_starts(80, 30), [0, 50]);
    assert_eq!(window_starts(50, 30), [0]);
    assert_eq!(window_starts(0, 30), [0]);
}

#[test]
fn the_same_sound_through_another_encoder_is_alike_and_another_sound_is_not() {
    let original = sound(1, 30.0);

    let copy = similarity(&reencoded(&original, 3), &original, MAX_SHIFT).expect("compared");
    let other = similarity(&sound(2, 30.0), &original, MAX_SHIFT).expect("compared");

    assert!(copy > 0.85, "{copy}");
    assert!(other < 0.6, "{other}");
}

#[test]
fn an_intro_on_one_of_them_is_absorbed() {
    let original = sound(1, 30.0);
    let mut with_intro = sound(9, 5.0);
    with_intro.extend(&original);

    let found = similarity(&with_intro, &original, MAX_SHIFT).expect("compared");

    assert!(found > 0.95, "{found}");
}

#[test]
fn silence_in_the_candidate_is_as_different_as_it_gets_and_silence_in_the_original_says_nothing() {
    let song = sound(1, 30.0);
    let silence = vec![0u32; song.len()];

    assert_eq!(similarity(&silence, &song, MAX_SHIFT), Some(0.0));
    assert_eq!(similarity(&song, &silence, MAX_SHIFT), None);
    // Two held tones look alike whatever they are, so they vouch for nothing.
    assert_eq!(similarity(&silence, &silence, MAX_SHIFT), None);
}

#[test]
fn too_little_to_compare_says_nothing() {
    assert_eq!(similarity(&sound(1, 2.0), &sound(1, 2.0), MAX_SHIFT), None);
    assert_eq!(similarity(&[], &sound(1, 30.0), MAX_SHIFT), None);
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_not_vouched_for() {
    let answer = same_recording(
        &AudioFingerprinter::new(),
        Path::new("/nonexistent/candidate.flac"),
        Path::new("/nonexistent/original.flac"),
        &SoulseekSettings::default(),
    )
    .await;

    assert_eq!(answer, SameRecording::CannotTell);
}

const MELODY: &str = "aevalsrc='0.4*sin(2*PI*t*(220+55*floor(mod(t*3,8)))) + 0.2*sin(2*PI*t*(330+40*floor(mod(t*2,7))))':s=44100";
const NOISE: &str = "anoisesrc=c=pink:a=0.3";

fn tools_present() -> bool {
    ["ffmpeg", "fpcalc"].iter().all(|tool| {
        std::process::Command::new(tool)
            .arg("-version")
            .output()
            .is_ok_and(|out| out.status.success())
    })
}

/// `seconds` of `source` written to `name` in `dir` with the extra ffmpeg arguments.
fn made(dir: &Path, name: &str, source: &str, seconds: u32, extra: &[&str]) -> PathBuf {
    let target = dir.join(name);
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("{source}:d={seconds}"),
        ])
        .args(extra)
        .arg(&target)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg made {name}");
    target
}

/// Real files: the same melody in another encoding is the same recording, another sound of the same
/// length is not, and a file much longer is another cut. Skipped where ffmpeg or fpcalc is missing.
#[tokio::test]
async fn real_files_are_judged_by_what_they_sound_like() {
    if !tools_present() {
        eprintln!("skipped: ffmpeg or fpcalc is not on the PATH");
        return;
    }
    let dir = tempfile::tempdir().expect("a temp dir");
    let original = made(
        dir.path(),
        "original.mp3",
        MELODY,
        100,
        &["-c:a", "libmp3lame", "-b:a", "128k"],
    );
    let lossless = made(dir.path(), "same.flac", MELODY, 100, &["-c:a", "flac"]);
    let other = made(dir.path(), "other.flac", NOISE, 100, &["-c:a", "flac"]);
    let longer = made(dir.path(), "longer.flac", MELODY, 140, &["-c:a", "flac"]);
    let fingerprinter = AudioFingerprinter::new();
    let settings = SoulseekSettings::default();

    let same = same_recording(&fingerprinter, &lossless, &original, &settings).await;
    let differs = same_recording(&fingerprinter, &other, &original, &settings).await;
    let cut = same_recording(&fingerprinter, &longer, &lossless, &settings).await;

    assert!(same.allows_replacing(), "{same:?}");
    assert!(
        matches!(differs, SameRecording::Differs(_)),
        "another sound of the same length: {differs:?}"
    );
    assert_eq!(cut, SameRecording::LengthDiffers(40));
}
