//! The one-time YouTube check asks about a song that sounds nothing like a believed YouTube preview
//! and does nothing else, goes only when it should, answers each song once, never takes a failure
//! of the shim for an answer, and never waits on one song for ever.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;

use octo_core::settings::{AppSettings, LibraryActionSettings, NoticeKind, SubsonicSettings};
use tokio::sync::Notify;

use super::*;

/// Answers one by one from a script, and remembers which files it was asked about.
#[derive(Default)]
struct FakeChecker {
    script: Mutex<VecDeque<Compared>>,
    asked: Mutex<Vec<String>>,
    hang: AtomicBool,
}

impl FakeChecker {
    fn answering(answers: Vec<Compared>) -> Arc<Self> {
        Arc::new(FakeChecker {
            script: Mutex::new(answers.into()),
            asked: Mutex::new(Vec::new()),
            hang: AtomicBool::new(false),
        })
    }

    /// Answers from the script, then never again.
    fn hanging_after(answers: Vec<Compared>) -> Arc<Self> {
        let checker = Self::answering(answers);
        checker.hang.store(true, Ordering::SeqCst);
        checker
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().clone()
    }
}

#[async_trait]
impl YoutubeChecker for FakeChecker {
    async fn check(&self, path: &Path, _: &Song) -> Compared {
        self.asked
            .lock()
            .push(path.file_name().unwrap().to_string_lossy().into_owned());
        let next = self.script.lock().pop_front();
        match next {
            Some(answer) => answer,
            // An empty script past its end: a shim that never answers.
            None if self.hang.load(Ordering::SeqCst) => std::future::pending().await,
            None => Compared::NoCopy,
        }
    }
}

#[derive(Default)]
struct FakeActivity {
    busy: AtomicBool,
}

impl IAcquisitionActivity for FakeActivity {
    fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
}

fn heard(heard: Heard, bit_error: f64) -> Compared {
    Compared::Heard {
        heard,
        bit_error,
        trusted: heard != Heard::Unclear,
        video_id: "vid".into(),
    }
}

fn settings_of(per_hour: i32) -> Arc<SettingsStore> {
    Arc::new(SettingsStore::for_tests(AppSettings {
        library_actions: LibraryActionSettings {
            enabled: true,
            review_enabled: true,
            youtube_check_per_hour: per_hour,
            allowed_users: vec!["bob".into()],
            ..Default::default()
        },
        subsonic: SubsonicSettings {
            admin_username: Some("bob".into()),
            ..Default::default()
        },
        ..Default::default()
    }))
}

struct Check {
    root: tempfile::TempDir,
    stores: Mutex<Vec<Arc<YoutubeCheckStore>>>,
}

impl Check {
    fn new() -> Self {
        Check {
            root: tempfile::tempdir().expect("a temp dir"),
            stores: Mutex::new(Vec::new()),
        }
    }

    fn state_path(&self) -> PathBuf {
        self.root.path().join(".state").join("youtube-check.json")
    }

    /// A FLAC tagged with an artist and title.
    fn tagged(&self, name: &str) -> String {
        let path = self.root.path().join(name);
        std::fs::write(&path, crate::services::test_support::flac()).expect("written");
        let mut file = TagFile::open(&path).expect("opens");
        file.set_performers(&["Massive Attack".to_string()]);
        file.set_title(Some("Teardrop"));
        file.save().expect("saved");
        path.to_string_lossy().into_owned()
    }

    /// A file with no tags at all.
    fn untagged(&self, name: &str) -> String {
        let path = self.root.path().join(name);
        std::fs::write(&path, crate::services::test_support::flac()).expect("written");
        path.to_string_lossy().into_owned()
    }

    fn worker(
        &self,
        checker: Arc<FakeChecker>,
        notices: Arc<NoticeQueue>,
        settings: Arc<SettingsStore>,
        activity: Arc<FakeActivity>,
    ) -> LibraryYoutubeCheck {
        // A worker built after another is a restart, and a stopping host flushes the store.
        for previous in self.stores.lock().iter() {
            previous.flush();
        }
        let store = Arc::new(YoutubeCheckStore::new(Some(self.state_path())));
        self.stores.lock().push(store.clone());
        let root = self.root.path().to_string_lossy().into_owned();
        LibraryYoutubeCheck::new(
            store,
            notices,
            checker,
            activity,
            settings,
            Box::new(move || root.clone()),
            Clock::system(),
        )
    }

    fn simple(&self, checker: Arc<FakeChecker>) -> (LibraryYoutubeCheck, Arc<NoticeQueue>) {
        let notices = Arc::new(NoticeQueue::new());
        let worker = self.worker(
            checker,
            notices.clone(),
            settings_of(60),
            Arc::new(FakeActivity::default()),
        );
        (worker, notices)
    }
}

/// Ticks until the run is over, a bounded number of times.
async fn run_to_the_end(worker: &LibraryYoutubeCheck) {
    for _ in 0..200 {
        worker.tick().await;
        if !worker.store().read(|s| s.running) {
            return;
        }
    }
    panic!("the run did not finish");
}

#[tokio::test]
async fn nothing_happens_until_a_person_starts_it() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![]);
    let (worker, _) = check.simple(checker.clone());

    worker.tick().await;

    assert!(checker.asked().is_empty());
    assert_eq!(worker.status().state, "Idle");
}

#[tokio::test]
async fn a_song_that_sounds_nothing_like_its_preview_is_asked_about_and_one_that_does_is_not() {
    let check = Check::new();
    check.tagged("a-wrong.flac");
    check.tagged("b-right.flac");
    let checker = FakeChecker::answering(vec![heard(Heard::Mismatch, 0.41), heard(Heard::Match, 0.97)]);
    let (worker, notices) = check.simple(checker);

    worker.start();
    run_to_the_end(&worker).await;

    let questions = notices.for_user("bob", NoticeKind::Review);
    assert_eq!(questions.len(), 1);
    let question = &questions[0];
    assert!(question.local_path.ends_with("a-wrong.flac"));
    assert_eq!(question.origin, NoticeOrigin::LibrarySweep);
    assert_eq!(question.cause, InconclusiveReason::DiffersFromYouTube);
    assert!(
        question.reason.contains("41% of its bits differ"),
        "{}",
        question.reason
    );
    let (asked, alike, done) = worker.store().read(|s| (s.asked, s.alike, s.done));
    assert_eq!((asked, alike, done), (1, 1, 2));
}

#[tokio::test]
async fn an_unclear_song_and_one_with_no_preview_are_left_alone() {
    let check = Check::new();
    check.tagged("a.flac");
    check.tagged("b.flac");
    let checker = FakeChecker::answering(vec![heard(Heard::Unclear, 0.64), Compared::NoCopy]);
    let (worker, notices) = check.simple(checker);

    worker.start();
    run_to_the_end(&worker).await;

    assert!(notices.for_user("bob", NoticeKind::Review).is_empty());
    let (unsure, no_copy) = worker.store().read(|s| (s.unsure, s.no_copy));
    assert_eq!((unsure, no_copy), (1, 1));
}

#[tokio::test]
async fn a_song_with_no_tags_is_not_looked_up() {
    let check = Check::new();
    check.untagged("nameless.flac");
    let checker = FakeChecker::answering(vec![]);
    let (worker, _) = check.simple(checker.clone());

    worker.start();
    run_to_the_end(&worker).await;

    assert!(checker.asked().is_empty());
    assert_eq!(worker.store().read(|s| s.no_copy), 1);
}

#[tokio::test]
async fn each_song_is_answered_once_and_the_run_does_not_start_again() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![heard(Heard::Match, 0.95)]);
    let (worker, _) = check.simple(checker.clone());

    worker.start();
    run_to_the_end(&worker).await;
    for _ in 0..5 {
        worker.tick().await;
    }

    assert_eq!(checker.asked(), ["a.flac"]);
    let finished = worker.store().read(|s| (s.running, s.finished_utc.is_some()));
    assert_eq!(finished, (false, true));
    assert_eq!(worker.status().state, "Idle");
}

#[tokio::test]
async fn a_restart_carries_on_where_it_stopped() {
    let check = Check::new();
    check.tagged("a.flac");
    check.tagged("b.flac");
    check.tagged("c.flac");
    let first = FakeChecker::answering(vec![heard(Heard::Match, 0.95)]);
    let (worker, notices) = check.simple(first.clone());
    worker.start();
    worker.tick().await;
    assert_eq!(first.asked(), ["a.flac"]);
    drop(worker);

    let second = FakeChecker::answering(vec![]);
    let worker = check.worker(
        second.clone(),
        notices,
        settings_of(60),
        Arc::new(FakeActivity::default()),
    );
    run_to_the_end(&worker).await;

    assert_eq!(second.asked(), ["b.flac", "c.flac"]);
}

#[tokio::test]
async fn a_failure_of_the_shim_is_not_an_answer() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![Compared::Unavailable, heard(Heard::Match, 0.95)]);
    let (worker, _) = check.simple(checker.clone());

    worker.start();
    worker.tick().await;
    // Not counted, the cursor has not moved past it, and it is tried again.
    assert_eq!(
        worker.store().read(|s| (s.done, s.cursor.clone())),
        (0, String::new())
    );
    run_to_the_end(&worker).await;

    assert_eq!(checker.asked(), ["a.flac", "a.flac"]);
    assert_eq!(worker.store().read(|s| s.alike), 1);
}

#[tokio::test]
async fn five_failures_in_a_row_make_it_rest_an_hour() {
    let check = Check::new();
    for name in ["a.flac", "b.flac"] {
        check.tagged(name);
    }
    let checker = FakeChecker::answering(vec![Compared::Unavailable; 5]);
    let (worker, _) = check.simple(checker);

    worker.start();
    let mut wait = Duration::ZERO;
    for _ in 0..5 {
        wait = worker.tick().await;
    }

    assert_eq!(wait, LibraryYoutubeCheck::REST_WHEN_UNAVAILABLE);
    assert_eq!(worker.status().state, "Waiting");
    assert_eq!(worker.store().read(|s| s.done), 0);
}

#[tokio::test]
async fn zero_an_hour_is_off() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![]);
    let notices = Arc::new(NoticeQueue::new());
    let worker = check.worker(
        checker.clone(),
        notices,
        settings_of(0),
        Arc::new(FakeActivity::default()),
    );

    worker.start();
    worker.tick().await;

    assert!(checker.asked().is_empty());
    assert_eq!(worker.status().state, "Off");
}

#[tokio::test]
async fn a_download_in_flight_makes_it_wait() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![]);
    let activity = Arc::new(FakeActivity::default());
    activity.busy.store(true, Ordering::SeqCst);
    let worker = check.worker(
        checker.clone(),
        Arc::new(NoticeQueue::new()),
        settings_of(60),
        activity.clone(),
    );

    worker.start();
    worker.tick().await;
    assert!(checker.asked().is_empty());
    assert_eq!(worker.status().state, "Waiting");

    activity.busy.store(false, Ordering::SeqCst);
    worker.tick().await;
    assert_eq!(checker.asked(), ["a.flac"]);
}

#[tokio::test]
async fn nobody_on_the_allowlist_means_nobody_is_asked() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![heard(Heard::Mismatch, 0.3)]);
    let settings = Arc::new(SettingsStore::for_tests(AppSettings {
        library_actions: LibraryActionSettings {
            enabled: true,
            review_enabled: true,
            youtube_check_per_hour: 60,
            allowed_users: Vec::new(),
            ..Default::default()
        },
        ..Default::default()
    }));
    let worker = check.worker(
        checker.clone(),
        Arc::new(NoticeQueue::new()),
        settings,
        Arc::new(FakeActivity::default()),
    );

    worker.start();
    worker.tick().await;

    assert!(checker.asked().is_empty());
    assert_eq!(worker.status().state, "Paused");
}

#[tokio::test]
async fn fifty_open_questions_pause_it() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![]);
    let notices = Arc::new(NoticeQueue::new());
    for i in 0..LibraryYoutubeCheck::MAX_OPEN_QUESTIONS {
        let question = VerificationResult {
            reason: InconclusiveReason::NoEntry,
            ..Default::default()
        };
        notices.add_review_from(
            "bob",
            &format!("/music/{i}.flac"),
            &Song::default(),
            &question,
            NoticeOrigin::LibrarySweep,
        );
    }
    let worker = check.worker(
        checker.clone(),
        notices,
        settings_of(60),
        Arc::new(FakeActivity::default()),
    );

    worker.start();
    worker.tick().await;

    assert!(checker.asked().is_empty());
    assert_eq!(worker.status().state, "Paused");
}

#[tokio::test]
async fn a_song_already_asked_about_is_not_asked_again() {
    let check = Check::new();
    let path = check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![]);
    let notices = Arc::new(NoticeQueue::new());
    let question = VerificationResult {
        reason: InconclusiveReason::NoEntry,
        ..Default::default()
    };
    notices.add_review_from(
        "bob",
        &path,
        &Song::default(),
        &question,
        NoticeOrigin::LibrarySweep,
    );
    let worker = check.worker(
        checker.clone(),
        notices,
        settings_of(60),
        Arc::new(FakeActivity::default()),
    );

    worker.start();
    run_to_the_end(&worker).await;

    assert!(checker.asked().is_empty());
}

#[tokio::test]
async fn pressing_start_again_forgets_the_answers() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![heard(Heard::Match, 0.95), heard(Heard::Match, 0.95)]);
    let (worker, _) = check.simple(checker.clone());
    worker.start();
    run_to_the_end(&worker).await;

    worker.start();
    run_to_the_end(&worker).await;

    assert_eq!(checker.asked(), ["a.flac", "a.flac"]);
}

#[test]
fn a_corrupt_state_file_is_set_aside_and_the_check_starts_over() {
    let check = Check::new();
    let path = check.state_path();
    std::fs::create_dir_all(path.parent().unwrap()).expect("folder");
    std::fs::write(&path, "{ not json").expect("written");

    let store = YoutubeCheckStore::new(Some(path.clone()));

    assert!(!store.read(|s| s.running));
    assert!(!path.exists());
}

#[tokio::test]
async fn a_song_that_cannot_be_compared_after_two_rests_is_counted_as_having_no_copy() {
    let check = Check::new();
    check.tagged("a-region-locked.flac");
    check.tagged("b-fine.flac");
    // Ten failures on the first song (two rests of five), then the second answers.
    let mut script = vec![Compared::Unavailable; 10];
    script.push(heard(Heard::Match, 0.95));
    let checker = FakeChecker::answering(script);
    let (worker, _) = check.simple(checker.clone());

    worker.start();
    run_to_the_end(&worker).await;

    let (no_copy, alike, done) = worker.store().read(|s| (s.no_copy, s.alike, s.done));
    assert_eq!((no_copy, alike, done), (1, 1, 2));
    assert_eq!(checker.asked().last().map(String::as_str), Some("b-fine.flac"));
    assert!(!worker.store().read(|s| s.running));
}

#[tokio::test]
async fn an_answer_between_failures_starts_the_count_again() {
    let check = Check::new();
    check.tagged("a.flac");
    check.tagged("b.flac");
    // Four failures, an answer, four more: never five in a row, so it never rests.
    let mut script = vec![Compared::Unavailable; 4];
    script.push(heard(Heard::Match, 0.95));
    script.extend(vec![Compared::Unavailable; 4]);
    script.push(heard(Heard::Match, 0.95));
    let (worker, _) = check.simple(FakeChecker::answering(script));

    worker.start();
    for _ in 0..20 {
        let wait = worker.tick().await;
        assert_ne!(wait, LibraryYoutubeCheck::REST_WHEN_UNAVAILABLE);
        if !worker.store().read(|s| s.running) {
            break;
        }
    }

    assert_eq!(worker.store().read(|s| (s.alike, s.running)), (2, false));
}

#[tokio::test]
async fn pressing_start_says_so_at_once() {
    let check = Check::new();
    check.tagged("a.flac");
    let (worker, _) = check.simple(FakeChecker::answering(vec![]));

    worker.start();
    let status = worker.status();

    assert!(status.running);
    assert_eq!(status.state, "Waiting");
    assert!(
        status.reason.as_deref().is_some_and(|r| r.contains("Starting")),
        "{status:?}"
    );
}

#[tokio::test]
async fn a_finished_run_stays_finished_after_a_restart() {
    let check = Check::new();
    check.tagged("a.flac");
    let first = FakeChecker::answering(vec![heard(Heard::Match, 0.95)]);
    let (worker, notices) = check.simple(first);
    worker.start();
    run_to_the_end(&worker).await;
    drop(worker);

    let second = FakeChecker::answering(vec![]);
    let worker = check.worker(
        second.clone(),
        notices,
        settings_of(60),
        Arc::new(FakeActivity::default()),
    );
    for _ in 0..5 {
        worker.tick().await;
    }

    assert!(second.asked().is_empty());
    assert_eq!(worker.status().state, "Idle");
    assert!(worker.store().read(|s| s.finished_utc.is_some() && !s.running));
}

#[tokio::test(start_paused = true)]
async fn a_song_that_never_answers_is_given_up_on_and_not_waited_for() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::hanging_after(vec![]);
    let (worker, _) = check.simple(checker.clone());

    worker.start();
    let started = tokio::time::Instant::now();
    worker.tick().await;

    // The tick came back after the timeout, not never, and the song is not counted.
    assert!(started.elapsed() >= LibraryYoutubeCheck::CHECK_TIMEOUT);
    assert_eq!(checker.asked(), ["a.flac"]);
    assert_eq!(worker.store().read(|s| (s.done, s.running)), (0, true));
}

/// Says when it is asked, then answers a mismatch only once let go.
struct GatedChecker {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait]
impl YoutubeChecker for GatedChecker {
    async fn check(&self, _: &Path, _: &Song) -> Compared {
        self.entered.notify_one();
        self.release.notified().await;
        heard(Heard::Mismatch, 0.45)
    }
}

#[tokio::test]
async fn an_answer_from_a_run_started_over_meanwhile_is_dropped() {
    let check = Check::new();
    check.tagged("a.flac");
    let gate = Arc::new(GatedChecker {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let notices = Arc::new(NoticeQueue::new());
    let store = Arc::new(YoutubeCheckStore::new(None));
    let root = check.root.path().to_string_lossy().into_owned();
    let worker = Arc::new(LibraryYoutubeCheck::new(
        store,
        notices.clone(),
        gate.clone(),
        Arc::new(FakeActivity::default()),
        settings_of(60),
        Box::new(move || root.clone()),
        Clock::system(),
    ));
    worker.start();

    let ticking = {
        let worker = worker.clone();
        tokio::spawn(async move { worker.tick().await })
    };
    gate.entered.notified().await;
    worker.start();
    gate.release.notify_one();
    ticking.await.expect("the tick ends");

    assert!(notices.for_user("bob", NoticeKind::Review).is_empty());
    assert_eq!(worker.store().read(|s| (s.done, s.asked)), (0, 0));
}

// ---- The comparison with a real preview -----------------------------------------------

/// A shim that sends `audio` as the preview of every song, or nothing.
struct OneVideo {
    audio: Option<PathBuf>,
}

#[async_trait]
impl PreviewSource for OneVideo {
    async fn find(&self, _request: &PreviewRequest<'_>) -> Option<PreviewFacts> {
        Some(Self::default_facts())
    }

    async fn fetch(&self, _video_id: &str, dest: &Path) -> bool {
        self.audio
            .as_ref()
            .is_some_and(|audio| std::fs::copy(audio, dest).is_ok())
    }
}

/// 40 seconds of pink noise as FLAC, or None where ffmpeg or fpcalc is missing.
fn noise_flac(dir: &Path, name: &str, color: &str) -> Option<PathBuf> {
    let on_path = |program: &str| {
        std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
    };
    if !on_path("ffmpeg") || !on_path("fpcalc") {
        return None;
    }
    let path = dir.join(name);
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!(
            "anoisesrc=color={color}:duration=40:sample_rate=44100:amplitude=0.3"
        ))
        .arg(&path)
        .status()
        .is_ok_and(|status| status.success());
    made.then_some(path)
}

fn request() -> PreviewRequest<'static> {
    PreviewRequest {
        artist: "Massive Attack",
        title: "Teardrop",
        seconds: Some(40),
        video_id: None,
    }
}

#[tokio::test]
async fn a_file_heard_against_its_own_sound_matches_and_a_shim_that_sends_nothing_is_unavailable() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let Some(file) = noise_flac(dir.path(), "file.flac", "pink") else {
        eprintln!("skipped: ffmpeg or fpcalc is not on the PATH");
        return;
    };
    let fingerprinter = AudioFingerprinter::new();
    let preview = OneVideo::default_facts();

    let same = OneVideo {
        audio: Some(file.clone()),
    };
    let answer = hear(&same, &fingerprinter, &file, &request(), preview.clone(), 30).await;
    assert!(
        matches!(
            answer,
            Compared::Heard {
                heard: Heard::Match,
                ..
            }
        ),
        "{answer:?}"
    );

    let down = OneVideo { audio: None };
    let answer = hear(&down, &fingerprinter, &file, &request(), preview, 30).await;
    assert_eq!(answer, Compared::Unavailable);
}

#[tokio::test]
async fn a_file_fpcalc_cannot_read_has_no_preview_to_compare_with() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let broken = dir.path().join("broken.flac");
    std::fs::write(&broken, b"not audio").expect("written");
    let shim = OneVideo { audio: None };

    let answer = hear(
        &shim,
        &AudioFingerprinter::new(),
        &broken,
        &request(),
        OneVideo::default_facts(),
        30,
    )
    .await;

    assert_eq!(answer, Compared::NoCopy);
}

impl OneVideo {
    fn default_facts() -> PreviewFacts {
        PreviewFacts {
            video_id: "vid".into(),
            title: Some("Teardrop".into()),
            artist: Some("Massive Attack".into()),
            seconds: Some(40),
        }
    }
}

/// A shim nothing answers on is not an answer: the song is tried again later.
#[tokio::test]
async fn an_unreachable_shim_is_unavailable_not_no_copy() {
    let closed = format!("http://{}", crate::services::test_support::refused_address());
    let checker = AppYoutubeChecker {
        fingerprinter: Arc::new(AudioFingerprinter::new()),
        youtube: Arc::new(YouTubeResolver::with_base_url(Some(&closed))),
        settings: settings_of(60),
    };
    let check = Check::new();
    let path = check.tagged("a.flac");
    let song = Song {
        artist: "Massive Attack".into(),
        title: "Teardrop".into(),
        ..Default::default()
    };

    assert_eq!(
        checker.check(Path::new(&path), &song).await,
        Compared::Unavailable
    );
}

// ---- The list of songs to look at -------------------------------------------------------

#[tokio::test]
async fn the_wrong_and_unclear_songs_are_kept_for_the_list_and_the_alike_are_not() {
    let check = Check::new();
    for name in ["a-wrong.flac", "b-unclear.flac", "c-alike.flac"] {
        check.tagged(name);
    }
    let checker = FakeChecker::answering(vec![
        heard(Heard::Mismatch, 0.47),
        heard(Heard::Unclear, 0.3),
        heard(Heard::Match, 0.05),
    ]);
    let (worker, _) = check.simple(checker);

    worker.start();
    run_to_the_end(&worker).await;

    let page = worker.songs(SongFilter::All, 0, 50).await;
    let rows: Vec<(&str, bool, bool)> = page
        .rows
        .iter()
        .map(|row| (row.id.as_str(), row.result.clearly_wrong, row.result.trusted))
        .collect();
    assert_eq!(
        rows,
        [("a-wrong.flac", true, true), ("b-unclear.flac", false, false)]
    );
    assert_eq!((page.wrong, page.unclear), (1, 1));
    let status = worker.status();
    assert_eq!((status.listed_wrong, status.listed_unclear), (1, 1));
    assert_eq!(page.rows[0].result.video_id, "vid");
}

#[tokio::test]
async fn the_list_is_worst_first_and_filters_and_pages() {
    let check = Check::new();
    for name in ["a.flac", "b.flac", "c.flac"] {
        check.tagged(name);
    }
    let checker = FakeChecker::answering(vec![
        heard(Heard::Mismatch, 0.40),
        heard(Heard::Mismatch, 0.55),
        heard(Heard::Unclear, 0.30),
    ]);
    let (worker, _) = check.simple(checker);
    worker.start();
    run_to_the_end(&worker).await;

    let all = worker.songs(SongFilter::All, 0, 50).await;
    let ids: Vec<&str> = all.rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, ["b.flac", "a.flac", "c.flac"]);
    let wrong = worker.songs(SongFilter::Wrong, 0, 50).await;
    assert_eq!(wrong.matching, 2);
    let second = worker.songs(SongFilter::Wrong, 1, 1).await;
    assert_eq!(second.rows.len(), 1);
    assert_eq!(second.rows[0].id, "a.flac");
    assert_eq!(second.matching, 2);
    assert_eq!(worker.songs(SongFilter::Unclear, 0, 50).await.rows.len(), 1);
}

#[tokio::test]
async fn a_file_that_changed_since_it_was_heard_is_not_on_the_list_any_more() {
    let check = Check::new();
    let path = check.tagged("a.flac");
    let (worker, _) = check.simple(FakeChecker::answering(vec![heard(Heard::Mismatch, 0.5)]));
    worker.start();
    run_to_the_end(&worker).await;
    assert_eq!(worker.songs(SongFilter::All, 0, 50).await.rows.len(), 1);

    std::fs::write(&path, b"another file, another size").expect("rewritten");

    assert!(worker.songs(SongFilter::All, 0, 50).await.rows.is_empty());
    assert!(worker.store().read(|s| s.results.is_empty()));
}

#[test]
fn a_full_list_drops_the_least_suspicious_song() {
    let mut state = YoutubeCheckState::default();
    let result = |bit_error: f64| YoutubeResult {
        artist: "A".into(),
        title: "T".into(),
        bit_error,
        clearly_wrong: true,
        trusted: true,
        video_id: "vid".into(),
        stamp: String::new(),
        checked_utc: Utc::now(),
    };
    for i in 0..=YoutubeCheckState::MAX_RESULTS {
        state.remember(&format!("{i:05}.flac"), result(0.3 + i as f64 / 100_000.0));
    }

    assert_eq!(state.results.len(), YoutubeCheckState::MAX_RESULTS);
    assert!(!state.results.contains_key("00000.flac"));
    assert!(state.results.contains_key("02000.flac"));
}

#[tokio::test]
async fn the_list_survives_a_restart_and_starting_over_empties_it() {
    let check = Check::new();
    check.tagged("a.flac");
    let (worker, notices) = check.simple(FakeChecker::answering(vec![heard(Heard::Mismatch, 0.5)]));
    worker.start();
    run_to_the_end(&worker).await;
    drop(worker);

    let worker = check.worker(
        FakeChecker::answering(vec![]),
        notices,
        settings_of(60),
        Arc::new(FakeActivity::default()),
    );
    assert_eq!(worker.songs(SongFilter::All, 0, 50).await.rows.len(), 1);

    worker.start();
    assert!(worker.store().read(|s| s.results.is_empty()));
}

#[tokio::test]
async fn keeping_and_dismissing_take_songs_off_the_list_and_answer_their_question() {
    let check = Check::new();
    check.tagged("a.flac");
    check.tagged("b.flac");
    let (worker, notices) = check.simple(FakeChecker::answering(vec![
        heard(Heard::Mismatch, 0.5),
        heard(Heard::Mismatch, 0.45),
    ]));
    worker.start();
    run_to_the_end(&worker).await;
    assert_eq!(notices.for_user("bob", NoticeKind::Review).len(), 2);

    let kept = worker.settle(
        &["a.flac".to_string(), "nope.flac".to_string()],
        NoticeState::Kept,
    );
    let dismissed = worker.settle(&["b.flac".to_string()], NoticeState::Dismissed);

    // Only songs that were on the list count, and no file was touched.
    assert_eq!((kept, dismissed), (1, 1));
    assert!(worker.store().read(|s| s.results.is_empty()));
    // Answered, so none of them waits any more (the answers stay in the queue).
    let entries = notices.for_user("bob", NoticeKind::Review);
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| !entry.is_open()), "{entries:?}");
    assert!(Path::new(&check.root.path().join("a.flac")).exists());
}

/// Wrong song as a script. With `gate` set, a replacement waits there until it is let go.
struct ListReplacer {
    shown: bool,
    marked: Option<bool>,
    gate: Option<Arc<Notify>>,
    requests: Mutex<Vec<(LibraryAction, String, String)>>,
}

impl ListReplacer {
    fn new(shown: bool, marked: Option<bool>, gate: Option<Arc<Notify>>) -> Arc<Self> {
        Arc::new(ListReplacer {
            shown,
            marked,
            gate,
            requests: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl SweepReplacer for ListReplacer {
    async fn song_id(&self, _: &str, _: &str, path: &str) -> Option<String> {
        self.shown
            .then(|| format!("nd-{}", path.rsplit('/').next().unwrap_or(path)))
    }

    async fn marked(&self, _: &str) -> Option<bool> {
        self.marked
    }

    async fn replace(
        &self,
        request: crate::services::library::library_action_executor::LibraryActionRequest,
    ) -> anyhow::Result<crate::services::library::library_action_executor::LibraryActionOutcome> {
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        self.requests
            .lock()
            .push((request.action, request.navidrome_id, request.username));
        Ok(
            crate::services::library::library_action_executor::LibraryActionOutcome::new(
                LibraryActionState::Applied,
                Some("done".into()),
            ),
        )
    }
}

/// A run over `names`, each heard as a mismatch, with `replacer` set.
async fn listed(check: &Check, names: &[&str], replacer: Arc<ListReplacer>) -> Arc<LibraryYoutubeCheck> {
    listed_as(check, names, replacer, heard(Heard::Mismatch, 0.5)).await
}

/// [`listed`] with every song heard as `answer`. A mismatch is also a question in Review, and
/// the check rests at 50 of those; an unclear song is on the list without one.
async fn listed_as(
    check: &Check,
    names: &[&str],
    replacer: Arc<ListReplacer>,
    answer: Compared,
) -> Arc<LibraryYoutubeCheck> {
    for name in names {
        check.tagged(name);
    }
    let checker = FakeChecker::answering(vec![answer; names.len()]);
    let worker = Arc::new(
        check
            .worker(
                checker,
                Arc::new(NoticeQueue::new()),
                settings_of(60),
                Arc::new(FakeActivity::default()),
            )
            .with_replacer(replacer),
    );
    worker.start();
    run_to_the_end(&worker).await;
    worker
}

/// Waits, for a bounded time, until the list is as long as `len`.
async fn until_listed(worker: &LibraryYoutubeCheck, len: usize) {
    for _ in 0..200 {
        if worker.store().read(|s| s.results.len()) == len {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the list did not get to {len} songs");
}

#[tokio::test]
async fn picked_songs_are_replaced_one_after_the_other_and_leave_the_list() {
    let check = Check::new();
    let replacer = ListReplacer::new(true, Some(false), None);
    let worker = listed(&check, &["a.flac", "b.flac", "c.flac"], replacer.clone()).await;

    let report = worker
        .replace_selected("bob", &["a.flac".to_string(), "b.flac".to_string()])
        .await
        .expect("started");

    assert_eq!((report.queued, report.skipped.len()), (2, 0));
    until_listed(&worker, 1).await;
    let requests = replacer.requests.lock().clone();
    assert_eq!(
        requests,
        [
            (
                LibraryAction::WrongSong,
                "nd-a.flac".to_string(),
                "bob".to_string()
            ),
            (
                LibraryAction::WrongSong,
                "nd-b.flac".to_string(),
                "bob".to_string()
            ),
        ]
    );
    assert_eq!(worker.songs(SongFilter::All, 0, 50).await.rows[0].id, "c.flac");
}

#[tokio::test]
async fn a_starred_song_one_navidrome_does_not_show_and_one_not_on_the_list_are_left_out() {
    for (shown, marked, why) in [
        (true, Some(true), "Someone starred or rated it."),
        (true, None, "Its stars and ratings could not be read."),
        (false, Some(false), "Navidrome does not show it yet."),
    ] {
        let check = Check::new();
        let replacer = ListReplacer::new(shown, marked, None);
        let worker = listed(&check, &["a.flac"], replacer.clone()).await;

        let report = worker
            .replace_selected("bob", &["a.flac".to_string(), "ghost.flac".to_string()])
            .await
            .expect("started");

        assert_eq!(report.queued, 0, "{why}");
        assert_eq!(
            report.skipped,
            [
                ("a.flac".to_string(), why.to_string()),
                ("ghost.flac".to_string(), "It is not on the list.".to_string()),
            ]
        );
        assert!(replacer.requests.lock().is_empty());
        // Nothing left running: the next batch is not refused.
        assert!(worker.replace_selected("bob", &[]).await.is_ok());
    }
}

#[tokio::test]
async fn a_second_batch_waits_until_the_first_is_over() {
    let check = Check::new();
    let gate = Arc::new(Notify::new());
    let replacer = ListReplacer::new(true, Some(false), Some(gate.clone()));
    let worker = listed(&check, &["a.flac", "b.flac"], replacer.clone()).await;

    let first = worker
        .replace_selected("bob", &["a.flac".to_string()])
        .await
        .expect("started");
    let second = worker.replace_selected("bob", &["b.flac".to_string()]).await;

    assert_eq!(first.queued, 1);
    assert!(second.is_err(), "{second:?}");
    gate.notify_one();
    until_listed(&worker, 1).await;
    // Over: a new batch goes through.
    let third = worker.replace_selected("bob", &["b.flac".to_string()]).await;
    assert!(third.is_ok(), "{third:?}");
    gate.notify_one();
}

#[tokio::test]
async fn at_most_fifty_songs_are_taken_in_one_request() {
    let check = Check::new();
    let replacer = ListReplacer::new(true, Some(false), None);
    let names: Vec<String> = (0..LibraryYoutubeCheck::MAX_REPLACE_AT_ONCE + 5)
        .map(|i| format!("{i:03}.flac"))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let worker = listed_as(&check, &refs, replacer, heard(Heard::Unclear, 0.3)).await;

    let report = worker.replace_selected("bob", &names).await.expect("started");

    assert_eq!(report.queued, LibraryYoutubeCheck::MAX_REPLACE_AT_ONCE);
    until_listed(&worker, 5).await;
}

#[tokio::test]
async fn without_a_replacer_replacing_is_refused() {
    let check = Check::new();
    check.tagged("a.flac");
    let (worker, _) = check.simple(FakeChecker::answering(vec![heard(Heard::Mismatch, 0.5)]));
    let worker = Arc::new(worker);
    worker.start();
    run_to_the_end(&worker).await;

    assert!(
        worker
            .replace_selected("bob", &["a.flac".to_string()])
            .await
            .is_err()
    );
}
