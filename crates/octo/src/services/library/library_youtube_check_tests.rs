//! The one-time YouTube check asks about a song that sounds nothing like its YouTube copy and does
//! nothing else, goes only when it should, answers each song once, and never takes a failure of
//! YouTube for an answer.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;

use octo_core::fingerprint::fingerprint_compare::CrossCheck;
use octo_core::settings::{AppSettings, LibraryActionSettings, NoticeKind, SubsonicSettings};

use super::*;
use crate::services::fingerprint::you_tube_cross_check::CrossCheckOutcome;

/// Answers one by one from a script, and remembers which files it was asked about.
#[derive(Default)]
struct FakeChecker {
    script: Mutex<VecDeque<Compared>>,
    asked: Mutex<Vec<String>>,
}

impl FakeChecker {
    fn answering(answers: Vec<Compared>) -> Arc<Self> {
        Arc::new(FakeChecker {
            script: Mutex::new(answers.into()),
            asked: Mutex::new(Vec::new()),
        })
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
        self.script.lock().pop_front().unwrap_or(Compared::NoReference)
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

fn outcome(verdict: CrossCheck, similarity: f64) -> Compared {
    Compared::Done(CrossCheckOutcome {
        video_id: "vid".into(),
        similarity,
        verdict,
    })
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
    for _ in 0..50 {
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
async fn a_song_that_sounds_nothing_like_its_copy_is_asked_about_and_one_that_does_is_not() {
    let check = Check::new();
    check.tagged("a-wrong.flac");
    check.tagged("b-right.flac");
    let checker = FakeChecker::answering(vec![
        outcome(CrossCheck::Differs, 0.41),
        outcome(CrossCheck::Match, 0.97),
    ]);
    let (worker, notices) = check.simple(checker);

    worker.start();
    run_to_the_end(&worker).await;

    let questions = notices.for_user("bob", NoticeKind::Review);
    assert_eq!(questions.len(), 1);
    let question = &questions[0];
    assert!(question.local_path.ends_with("a-wrong.flac"));
    assert_eq!(question.origin, NoticeOrigin::LibrarySweep);
    assert_eq!(question.cause, InconclusiveReason::DiffersFromYouTube);
    assert!(question.reason.contains("41% alike"), "{}", question.reason);
    let (asked, alike, done) = worker.store().read(|s| (s.asked, s.alike, s.done));
    assert_eq!((asked, alike, done), (1, 1, 2));
}

#[tokio::test]
async fn an_unsure_song_and_one_with_no_copy_are_left_alone() {
    let check = Check::new();
    check.tagged("a.flac");
    check.tagged("b.flac");
    let checker = FakeChecker::answering(vec![outcome(CrossCheck::Unsure, 0.64), Compared::NoReference]);
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
    let checker = FakeChecker::answering(vec![outcome(CrossCheck::Match, 0.95)]);
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
    let first = FakeChecker::answering(vec![outcome(CrossCheck::Match, 0.95)]);
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
async fn a_failure_of_youtube_is_not_an_answer() {
    let check = Check::new();
    check.tagged("a.flac");
    let checker = FakeChecker::answering(vec![Compared::Unavailable, outcome(CrossCheck::Match, 0.95)]);
    let (worker, _) = check.simple(checker.clone());

    worker.start();
    worker.tick().await;
    // Not stamped, not counted, and tried again.
    assert_eq!(worker.store().read(|s| (s.done, s.checked.len())), (0, 0));
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
    assert_eq!(worker.store().read(|s| s.checked.len()), 0);
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
    let checker = FakeChecker::answering(vec![outcome(CrossCheck::Differs, 0.3)]);
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
    let checker = FakeChecker::answering(vec![
        outcome(CrossCheck::Match, 0.95),
        outcome(CrossCheck::Match, 0.95),
    ]);
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
