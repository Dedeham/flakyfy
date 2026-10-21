//! A wrong file must never replace a right one, so the verdicts that refuse matter as much as the
//! one that allows. The audio cases skip where ffmpeg or fpcalc is missing.

use std::process::Command;

use super::*;

const MELODY: &str = "aevalsrc='0.4*sin(2*PI*t*(220+55*floor(mod(t*3,8)))) + 0.2*sin(2*PI*t*(330+40*floor(mod(t*2,7))))':d=40:s=44100";
const OTHER_MELODY: &str = "aevalsrc='0.4*sin(2*PI*t*(300+70*floor(mod(t*5,9)))) + 0.2*sin(2*PI*t*(500+30*floor(mod(t*3,5))))':d=40:s=44100";
const NOISE: &str = "anoisesrc=d=40:c=pink:a=0.3";

fn tools_present() -> bool {
    ["ffmpeg", "fpcalc"].iter().all(|tool| {
        Command::new(tool)
            .arg("-version")
            .output()
            .is_ok_and(|out| out.status.success())
    })
}

fn make(name: &str, source: &str, extra: &[&str]) -> std::path::PathBuf {
    let target = std::env::temp_dir().join(format!("octo-same-{}-{name}", std::process::id()));
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", source])
        .args(extra)
        .arg(&target)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg made {name}");
    target
}

async fn judged(candidate: &Path, original: &Path) -> SameRecording {
    same_recording(
        &AudioFingerprinter::new(),
        candidate,
        original,
        &SoulseekSettings::default(),
    )
    .await
}

#[test]
fn only_a_clear_match_allows_replacing() {
    assert!(SameRecording::from_similarity(0.97).allows_replacing());
    assert!(SameRecording::from_similarity(0.85).allows_replacing());
    // The same recording at another speed (3% faster scored 81 to 84) is another master.
    assert!(!SameRecording::from_similarity(0.84).allows_replacing());
    assert!(!SameRecording::from_similarity(0.70).allows_replacing());
    assert!(!SameRecording::from_similarity(0.65).allows_replacing());
    assert!(!SameRecording::from_similarity(0.40).allows_replacing());
    assert!(!SameRecording::CannotTell.allows_replacing());
}

#[test]
fn only_a_sound_that_is_another_is_thrown_away() {
    assert!(SameRecording::from_similarity(0.40).is_another_sound());
    assert!(SameRecording::from_similarity(0.65).is_another_sound());
    assert!(!SameRecording::from_similarity(0.97).is_another_sound());
    // A comparison that failed says nothing about the file.
    assert!(!SameRecording::CannotTell.is_another_sound());
}

#[test]
fn refusals_name_the_reason_and_the_percentage() {
    assert!(SameRecording::Differs(0.41).refusal().contains("41% alike"));
    assert!(SameRecording::Unsure(0.66).refusal().contains("another version"));
    assert!(
        SameRecording::CannotTell
            .refusal()
            .contains("could not be compared")
    );
}

#[tokio::test]
async fn the_same_recording_as_flac_replaces_its_mp3() {
    if !tools_present() {
        return;
    }
    let mp3 = make("same.mp3", MELODY, &["-c:a", "libmp3lame", "-b:a", "128k"]);
    let flac = make("same.flac", MELODY, &["-c:a", "flac"]);

    let found = judged(&flac, &mp3).await;

    assert!(found.allows_replacing(), "{found:?}");
}

#[tokio::test]
async fn another_song_or_noise_never_replaces() {
    if !tools_present() {
        return;
    }
    let mp3 = make("kept.mp3", MELODY, &["-c:a", "libmp3lame", "-b:a", "128k"]);
    let other = make("other.flac", OTHER_MELODY, &["-c:a", "flac"]);
    let noise = make("noise.flac", NOISE, &["-c:a", "flac"]);

    for candidate in [other, noise] {
        let found = judged(&candidate, &mp3).await;
        assert!(matches!(found, SameRecording::Differs(_)), "{found:?}");
        assert!(!found.allows_replacing());
    }
}

#[tokio::test]
async fn a_missing_file_cannot_be_told() {
    if !tools_present() {
        return;
    }
    let mp3 = make("present.mp3", MELODY, &["-c:a", "libmp3lame", "-b:a", "128k"]);
    let gone = std::env::temp_dir().join("octo-same-does-not-exist.flac");

    assert_eq!(judged(&gone, &mp3).await, SameRecording::CannotTell);
    assert_eq!(judged(&mp3, &gone).await, SameRecording::CannotTell);
}

#[test]
fn windows_cover_start_middle_and_end_of_a_long_file_and_only_the_start_of_a_short_one() {
    assert_eq!(window_starts(200, 30), vec![0, 85, 170]);
    assert_eq!(window_starts(80, 30), vec![0, 50]);
    assert_eq!(window_starts(40, 30), vec![0]);
    assert_eq!(window_starts(100, 0), vec![0]);
}

#[test]
fn a_length_gap_is_a_reason_to_keep_the_original() {
    let longer = SameRecording::LengthDiffers(45);
    assert!(!longer.allows_replacing());
    assert!(longer.is_another_sound());
    assert!(longer.refusal().contains("45s longer"));
    assert!(
        SameRecording::LengthDiffers(-30)
            .refusal()
            .contains("30s shorter")
    );
}

/// Another tune, long enough (80 s) to follow forty seconds of `LONG_MELODY` to 120 s, so the file
/// is not refused for its length before the windows are compared.
const LONG_OTHER_MELODY: &str = "aevalsrc='0.4*sin(2*PI*t*(300+70*floor(mod(t*5,9)))) + 0.2*sin(2*PI*t*(500+30*floor(mod(t*3,5))))':d=80:s=44100";

const LONG_MELODY: &str = "aevalsrc='0.4*sin(2*PI*t*(220+55*floor(mod(t*3,8)))) + 0.2*sin(2*PI*t*(330+40*floor(mod(t*2,7))))':d=120:s=44100";

#[tokio::test]
async fn a_youtube_copy_with_a_short_intro_is_still_the_same_song() {
    if !tools_present() {
        return;
    }
    let mp3 = make(
        "intro.mp3",
        LONG_MELODY,
        &["-af", "adelay=5000|5000", "-c:a", "libmp3lame", "-b:a", "128k"],
    );
    let flac = make("intro.flac", LONG_MELODY, &["-c:a", "flac"]);

    let found = judged(&flac, &mp3).await;

    assert!(found.allows_replacing(), "{found:?}");
}

#[tokio::test]
async fn the_same_start_with_another_ending_never_replaces() {
    if !tools_present() {
        return;
    }
    let mp3 = make("tail.mp3", LONG_MELODY, &["-c:a", "libmp3lame", "-b:a", "128k"]);
    let other = std::env::temp_dir().join(format!("octo-same-{}-tail.flac", std::process::id()));
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", LONG_MELODY])
        .args(["-f", "lavfi", "-i", LONG_OTHER_MELODY])
        .args([
            "-filter_complex",
            "[0]atrim=0:40[a];[1]atrim=0:80,asetpts=PTS-STARTPTS[b];[a][b]concat=n=2:v=0:a=1",
            "-c:a",
            "flac",
        ])
        .arg(&other)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success());

    let found = judged(&other, &mp3).await;

    // Refused for the sound of its later windows, not for its length: the start alone would match.
    assert!(matches!(found, SameRecording::Differs(_)), "{found:?}");
}
