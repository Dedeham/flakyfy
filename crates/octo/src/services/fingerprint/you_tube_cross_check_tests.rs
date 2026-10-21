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

// The comparison itself, on real audio (forty seconds: YouTube Music rows under thirty are not songs): a shim that serves the "official" copy, and files made by
// ffmpeg. Skipped where ffmpeg or fpcalc is missing, like the other audio tests.

use std::process::Command;

use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MELODY: &str = "aevalsrc='0.4*sin(2*PI*t*(220+55*floor(mod(t*3,8)))) + 0.2*sin(2*PI*t*(330+40*floor(mod(t*2,7))))':d=40:s=44100";
const OTHER_MELODY: &str = "aevalsrc='0.4*sin(2*PI*t*(300+70*floor(mod(t*5,9)))) + 0.2*sin(2*PI*t*(500+30*floor(mod(t*3,5))))':d=40:s=44100";
const NOISE: &str = "anoisesrc=d=40:c=pink:a=0.3";

const ROWS: &str = r#"{"tracks":[
    {"videoId":"atv1","title":"Song","artists":["Artist"],"durationSeconds":40,"videoType":"MUSIC_VIDEO_TYPE_ATV"}
]}"#;

fn tools_present() -> bool {
    ["ffmpeg", "fpcalc"].iter().all(|tool| {
        Command::new(tool)
            .arg("-version")
            .output()
            .is_ok_and(|out| out.status.success())
    })
}

/// A file made by ffmpeg from a lavfi source, named per test so parallel tests never share one.
fn make(name: &str, source: &str, extra: &[&str]) -> std::path::PathBuf {
    let target = std::env::temp_dir().join(format!("octo-xcheck-{}-{name}", std::process::id()));
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", source])
        .args(extra)
        .arg(&target)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg made {name}");
    target
}

async fn shim_serving(reference: &Path) -> MockServer {
    let shim = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ytm/search"))
        .and(query_param("q", "Artist Song"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ROWS))
        .mount(&shim)
        .await;
    Mock::given(method("GET"))
        .and(path("/stream"))
        .and(query_param("id", "atv1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "audio/wav")
                .set_body_bytes(std::fs::read(reference).expect("reference written")),
        )
        .mount(&shim)
        .await;
    shim
}

async fn outcome(file: &Path, reference: &Path) -> Option<CrossCheckOutcome> {
    let shim = shim_serving(reference).await;
    let youtube = YouTubeResolver::with_base_url(Some(&shim.uri()));
    compare(
        &AudioFingerprinter::new(),
        &youtube,
        file,
        "Artist",
        "Song",
        Some(40),
        &SoulseekSettings::default(),
    )
    .await
    .done()
}

#[tokio::test]
async fn compare_the_same_song_in_another_encoding_matches() {
    if !tools_present() {
        return;
    }
    let reference = make("same-ref.wav", MELODY, &[]);
    let download = make("same-dl.mp3", MELODY, &["-c:a", "libmp3lame", "-b:a", "128k"]);

    let found = outcome(&download, &reference)
        .await
        .expect("a reference and two fingerprints");

    assert_eq!(found.verdict, CrossCheck::Match, "{}", found.similarity);
    assert_eq!(found.video_id, "atv1");
}

#[tokio::test]
async fn compare_noise_for_a_song_differs() {
    if !tools_present() {
        return;
    }
    let reference = make("noise-ref.wav", MELODY, &[]);
    let download = make("noise-dl.wav", NOISE, &[]);

    let found = outcome(&download, &reference)
        .await
        .expect("a reference and two fingerprints");

    assert_eq!(found.verdict, CrossCheck::Differs, "{}", found.similarity);
}

#[tokio::test]
async fn compare_another_song_differs() {
    if !tools_present() {
        return;
    }
    let reference = make("other-ref.wav", MELODY, &[]);
    let download = make("other-dl.wav", OTHER_MELODY, &[]);

    let found = outcome(&download, &reference)
        .await
        .expect("a reference and two fingerprints");

    assert_eq!(found.verdict, CrossCheck::Differs, "{}", found.similarity);
}

/// No confident row on YouTube Music is no opinion, never a rejection.
#[tokio::test]
async fn compare_without_a_reference_has_no_opinion() {
    if !tools_present() {
        return;
    }
    let download = make("none-dl.wav", NOISE, &[]);
    let shim = MockServer::start().await;
    Mock::given(path("/ytm/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"tracks":[]}"#))
        .mount(&shim)
        .await;
    let youtube = YouTubeResolver::with_base_url(Some(&shim.uri()));

    let found = compare(
        &AudioFingerprinter::new(),
        &youtube,
        &download,
        "Artist",
        "Song",
        Some(40),
        &SoulseekSettings::default(),
    )
    .await;

    assert_eq!(found, Compared::NoReference);
}

// The retry budget decides when Soulseek stops asking peers, so a miscount either wastes downloads
// of wrong files or gives up on a song that still had a good peer.

#[test]
fn gives_up_only_at_the_budget() {
    assert!(!gives_up(0, 3, false, false));
    assert!(!gives_up(2, 3, false, false));
    assert!(gives_up(3, 3, false, false));
    assert!(gives_up(4, 3, false, false));
    assert!(gives_up(1, 1, false, false));
}

#[test]
fn gives_up_never_while_a_transcode_is_held_or_the_caller_is_gone() {
    assert!(!gives_up(3, 3, true, false));
    assert!(!gives_up(3, 3, false, true));
    assert!(!gives_up(9, 3, true, true));
}

#[test]
fn retries_setting_stays_between_one_and_ten() {
    for (set, expected) in [(0, 1), (-4, 1), (3, 3), (10, 10), (99, 10)] {
        let s = SoulseekSettings {
            youtube_cross_check_retries: set,
            ..Default::default()
        };
        assert_eq!(s.effective_youtube_cross_check_retries(), expected, "{set}");
    }
}

/// A YouTube Music that cannot answer is not "no reference": a caller that remembers its answers
/// must be able to ask again.
#[tokio::test]
async fn compare_while_youtube_music_is_down_is_unavailable_not_final() {
    if !tools_present() {
        return;
    }
    let download = make("down-dl.wav", NOISE, &[]);
    let shim = MockServer::start().await;
    Mock::given(path("/ytm/search"))
        .respond_with(ResponseTemplate::new(503).set_body_string(r#"{"error":"busy"}"#))
        .mount(&shim)
        .await;
    let youtube = YouTubeResolver::with_base_url(Some(&shim.uri()));

    let found = compare(
        &AudioFingerprinter::new(),
        &youtube,
        &download,
        "Artist",
        "Song",
        Some(40),
        &SoulseekSettings::default(),
    )
    .await;

    assert_eq!(found, Compared::Unavailable);
}

/// A reference found but a stream that will not play is a failure of the shim, not a verdict.
#[tokio::test]
async fn compare_with_a_stream_that_will_not_play_is_unavailable() {
    if !tools_present() {
        return;
    }
    let download = make("nostream-dl.wav", NOISE, &[]);
    let shim = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ytm/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ROWS))
        .mount(&shim)
        .await;
    Mock::given(path("/stream"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&shim)
        .await;
    let youtube = YouTubeResolver::with_base_url(Some(&shim.uri()));

    let found = compare(
        &AudioFingerprinter::new(),
        &youtube,
        &download,
        "Artist",
        "Song",
        Some(40),
        &SoulseekSettings::default(),
    )
    .await;

    assert_eq!(found, Compared::Unavailable);
}

/// A file that cannot be read costs YouTube no request and is a final answer.
#[tokio::test]
async fn compare_with_an_unreadable_file_is_a_final_no_reference() {
    if !tools_present() {
        return;
    }
    let shim = MockServer::start().await;
    let youtube = YouTubeResolver::with_base_url(Some(&shim.uri()));

    let found = compare(
        &AudioFingerprinter::new(),
        &youtube,
        Path::new("/no/such/file.flac"),
        "Artist",
        "Song",
        Some(40),
        &SoulseekSettings::default(),
    )
    .await;

    assert_eq!(found, Compared::NoReference);
    assert!(shim.received_requests().await.unwrap_or_default().is_empty());
}

/// A file that is there but gives no fingerprint (fpcalc missing, busy, or bytes that are not audio)
/// is not a final "no copy": the caller asks again, and gives up on a file that never works.
#[tokio::test]
async fn compare_with_a_file_that_gives_no_fingerprint_is_unavailable() {
    let garbage = std::env::temp_dir().join(format!("octo-xcheck-{}-garbage.flac", std::process::id()));
    std::fs::write(&garbage, b"not audio at all").expect("written");
    let shim = MockServer::start().await;
    let youtube = YouTubeResolver::with_base_url(Some(&shim.uri()));

    let found = compare(
        &AudioFingerprinter::new(),
        &youtube,
        &garbage,
        "Artist",
        "Song",
        Some(40),
        &SoulseekSettings::default(),
    )
    .await;

    assert_eq!(found, Compared::Unavailable);
    assert!(shim.received_requests().await.unwrap_or_default().is_empty());
}
