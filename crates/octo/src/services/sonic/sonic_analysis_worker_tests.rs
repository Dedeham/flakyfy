//! `SonicAnalysisWorkerTests`: Sounds alike's pass over the library: when it holds, what it
//! sends to octo-sonic, what it skips without touching the disk, how failures and crashes are
//! remembered, and new arrivals first.

use std::sync::atomic::{AtomicBool, AtomicUsize};

use chrono::TimeZone;
use octo_core::settings::{AppSettings, RadioSourceSettings};

use super::*;
use crate::services::sonic::sonic_client::{SonicDropped, SonicFeatures, SonicHealth};

#[derive(Default)]
struct Activity(AtomicBool);

impl IAcquisitionActivity for Activity {
    fn is_busy(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// octo-sonic: its health, and per file name an answer (features, an error, or a dropped
/// connection).
struct Sidecar {
    version: Mutex<Option<i32>>,
    /// Its music folder is empty or unreadable: health answers 503.
    blind: AtomicBool,
    /// Sounds alike is off in .env: health answers 503 with off.
    off: AtomicBool,
    analysed: Mutex<Vec<String>>,
    errors: Mutex<HashMap<String, String>>,
    drops: Mutex<HashSet<String>>,
    /// Files it cannot find (404), as when it mounts another folder than Octo.
    missing: Mutex<HashSet<String>>,
    /// Files it answers with something that is not its JSON.
    garbled: Mutex<HashSet<String>>,
}

impl Default for Sidecar {
    fn default() -> Self {
        Sidecar {
            version: Mutex::new(Some(2)),
            blind: AtomicBool::new(false),
            off: AtomicBool::new(false),
            analysed: Mutex::default(),
            errors: Mutex::default(),
            drops: Mutex::default(),
            missing: Mutex::default(),
            garbled: Mutex::default(),
        }
    }
}

impl Sidecar {
    fn analysed(&self) -> Vec<String> {
        self.analysed.lock().clone()
    }
}

#[async_trait]
impl ISonicClient for Sidecar {
    async fn health(&self) -> Option<SonicHealth> {
        let version = (*self.version.lock())?;
        if self.off.load(Ordering::SeqCst) {
            return Some(SonicHealth {
                version,
                problem: None,
                off: true,
            });
        }
        Some(SonicHealth {
            version,
            problem: self
                .blind
                .load(Ordering::SeqCst)
                .then(|| "cannot see the music folder".to_string()),
            off: false,
        })
    }

    async fn analyze(&self, path: &str) -> Result<SonicAnswer, SonicDropped> {
        let name = std::path::Path::new(path)
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned();
        self.analysed.lock().push(name.clone());
        if self.drops.lock().contains(&name) {
            return Err(SonicDropped("connection reset".into()));
        }
        let answer = |error: &str, status: u16| SonicAnswer {
            features: None,
            error: Some(error.to_string()),
            status,
        };
        if self.missing.lock().contains(&name) {
            return Ok(answer("no such file", 404));
        }
        if self.garbled.lock().contains(&name) {
            return Ok(answer(
                "octo-sonic answered something Octo could not read (HTTP 200)",
                200,
            ));
        }
        if let Some(error) = self.errors.lock().get(&name) {
            return Ok(answer(error, 422));
        }
        Ok(SonicAnswer {
            features: Some(SonicFeatures {
                values: (0..23).map(|i| i as f32 / 100.0).collect(),
                version: self.version.lock().unwrap_or(2),
            }),
            error: None,
            status: 200,
        })
    }
}

struct Library {
    directory: std::path::PathBuf,
    rows: Mutex<Vec<LibrarySongRow>>,
    arrivals: Mutex<HashMap<String, LibrarySongRow>>,
    file_lookups: AtomicUsize,
    listable: AtomicBool,
    /// A page that fails, by its start.
    failing_page: Mutex<Option<i32>>,
    /// Songs Navidrome lists as missing: counted in the page, left out of its rows.
    missing_ids: Mutex<HashSet<String>>,
}

impl Library {
    fn row(
        &self,
        id: &str,
        path: std::path::PathBuf,
        size: i64,
        title: &str,
        seconds: Option<i32>,
    ) -> LibrarySongRow {
        LibrarySongRow {
            id: id.into(),
            path: path.to_string_lossy().into_owned(),
            size,
            suffix: "flac".into(),
            bit_rate: 900,
            title: title.into(),
            artist: "Artist".into(),
            duration: seconds,
            album: Some("Album".into()),
            ..Default::default()
        }
    }

    /// A row with no file behind it, for listing tests.
    fn add_listed(&self, id: &str) {
        let path = self.directory.join("none").join(format!("{id}.flac"));
        let row = self.row(id, path, 1000, id, Some(200));
        self.rows.lock().push(row);
    }

    /// A song with a file; listed unless `listed` is false.
    fn add(&self, id: &str, name: &str, seconds: Option<i32>, listed: bool) -> LibrarySongRow {
        let path = self.directory.join(name);
        std::fs::write(&path, vec![0u8; 1000 + id.len()]).expect("writes");
        let size = std::fs::metadata(&path).expect("there").len() as i64;
        let row = self.row(id, path, size, name, seconds);
        if listed {
            self.rows.lock().push(row.clone());
        }
        row
    }
}

#[async_trait]
impl ISonicLibrary for Library {
    /// As Navidrome's native API answers: the count is the page's rows before missing songs were
    /// left out, not the library's.
    async fn list(&self, start: i32, count: i32) -> Option<(Vec<LibrarySongRow>, usize)> {
        if !self.listable.load(Ordering::SeqCst) || *self.failing_page.lock() == Some(start) {
            return None;
        }
        let rows = self.rows.lock();
        let page: Vec<LibrarySongRow> = rows
            .iter()
            .skip(start as usize)
            .take(count as usize)
            .cloned()
            .collect();
        let missing = self.missing_ids.lock();
        let len = page.len();
        Some((
            page.into_iter()
                .filter(|row| !missing.contains(&row.id))
                .collect(),
            len,
        ))
    }

    async fn arrived(&self, id: &str) -> Option<LibrarySongRow> {
        self.arrivals.lock().get(id).cloned()
    }

    fn local_file(&self, row: &LibrarySongRow) -> Option<SonicFile> {
        self.file_lookups.fetch_add(1, Ordering::SeqCst);
        SonicFile::stat(&row.path)
    }
}

struct T {
    _dir: tempfile::TempDir,
    worker: Arc<SonicAnalysisWorker>,
    store: Arc<SonicStore>,
    sidecar: Arc<Sidecar>,
    library: Arc<Library>,
    activity: Arc<Activity>,
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl T {
    fn advance(&self, by: TimeDelta) {
        *self.now.lock() += by;
    }

    async fn tick(&self) {
        self.worker.tick().await;
    }

    fn state(&self) -> (String, Option<String>) {
        let status = self.worker.status();
        (status.state, status.reason)
    }

    async fn ticks_until_done(&self) -> usize {
        for tick in 1..=20 {
            self.tick().await;
            if self.worker.status().state == "Done" {
                return tick;
            }
        }
        panic!("the pass never finished");
    }
}

fn make(settings: Option<RadioSourceSettings>) -> T {
    let dir = tempfile::tempdir().expect("a temp dir");
    let store = Arc::new(SonicStore::new(dir.path().join("state.json")));
    let sidecar = Arc::new(Sidecar::default());
    let library = Arc::new(Library {
        directory: dir.path().to_path_buf(),
        rows: Mutex::default(),
        arrivals: Mutex::default(),
        file_lookups: AtomicUsize::new(0),
        listable: AtomicBool::new(true),
        failing_page: Mutex::default(),
        missing_ids: Mutex::default(),
    });
    let activity = Arc::new(Activity::default());
    let now = Arc::new(Mutex::new(Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()));
    let clock_now = now.clone();
    let settings = Arc::new(SettingsStore::for_tests(AppSettings {
        radio_sources: settings.unwrap_or(RadioSourceSettings {
            sonic_url: "http://sonic.test".into(),
            sonic_pause_seconds: 0,
            ..Default::default()
        }),
        ..Default::default()
    }));
    let worker = SonicAnalysisWorker::new(
        store.clone(),
        sidecar.clone(),
        library.clone(),
        activity.clone(),
        settings,
        None,
        Clock::new(move || *clock_now.lock()),
    );
    T {
        _dir: dir,
        worker,
        store,
        sidecar,
        library,
        activity,
        now,
    }
}

fn analyzed_song() -> SonicSong {
    SonicSong {
        stamp: "1:1".into(),
        version: 2,
        f: vec![0.0; 23],
        ..Default::default()
    }
}

// SonicAnalysisWorkerTests.ItHolds_WhenOff_Paused_Down_OrBusy
#[tokio::test]
async fn it_holds_when_off_paused_down_or_busy() {
    let off = make(Some(RadioSourceSettings {
        sounds_alike: false,
        ..Default::default()
    }));
    off.tick().await;
    assert_eq!(off.worker.status().state, "Off");

    let t = make(None);
    t.library.add("a", "a.flac", Some(200), true);
    t.worker.set_paused(true);
    t.tick().await;
    assert_eq!(t.worker.status().state, "Paused");

    t.worker.set_paused(false);
    *t.sidecar.version.lock() = None;
    t.tick().await;
    assert_eq!(
        t.state(),
        (
            "Waiting".to_string(),
            Some("octo-sonic is not answering.".to_string())
        )
    );

    *t.sidecar.version.lock() = Some(2);
    t.activity.0.store(true, Ordering::SeqCst);
    t.tick().await;
    assert_eq!(t.state().1.as_deref(), Some("Waiting for a download to finish."));
    assert!(t.sidecar.analysed().is_empty());
}

// SonicAnalysisWorkerTests.WithoutANavidromeSignIn_ItSaysSo
#[tokio::test]
async fn without_a_navidrome_sign_in_it_says_so() {
    let t = make(None);
    t.library.listable.store(false, Ordering::SeqCst);
    t.tick().await;
    assert!(
        t.state()
            .1
            .unwrap()
            .contains("Octo needs its own Navidrome sign-in")
    );
}

// SonicAnalysisWorkerTests.ItAnalysesEachSongOnce_ThenFinishesThePass
#[tokio::test]
async fn it_analyzes_each_song_once_then_finishes_the_pass() {
    let t = make(None);
    t.library.add("a", "a.flac", Some(200), true);
    t.library.add("b", "b.flac", Some(200), true);

    t.tick().await;
    t.tick().await;
    assert_eq!(t.sidecar.analysed(), ["a.flac", "b.flac"]);
    assert_eq!(t.store.read(|s| s.songs["a"].f.len()), 23);
    assert_eq!(t.store.read(|s| s.songs["a"].version), 2);

    let lookups = t.library.file_lookups.load(Ordering::SeqCst);
    t.tick().await;
    assert_eq!(t.worker.status().state, "Done");
    assert_eq!(t.store.read(|s| s.pass), 1);
    // Analyzed songs at the same size are passed over without asking for their files.
    assert_eq!(t.library.file_lookups.load(Ordering::SeqCst), lookups);
    assert_eq!(t.sidecar.analysed().len(), 2);
}

// SonicAnalysisWorkerTests.ANewFeatureVersion_IsAnalysedAgain
#[tokio::test]
async fn a_new_feature_version_is_analyzed_again() {
    let t = make(None);
    t.library.add("a", "a.flac", Some(200), true);
    t.tick().await;
    t.tick().await;
    t.advance(PASS_INTERVAL + TimeDelta::minutes(1));

    *t.sidecar.version.lock() = Some(3);
    t.tick().await;
    assert_eq!(t.sidecar.analysed(), ["a.flac", "a.flac"]);
    assert_eq!(t.store.read(|s| s.songs["a"].version), 3);
}

// SonicAnalysisWorkerTests.AnUnreadableFile_IsRemembered_AndTriedAgainAWeekLater
#[tokio::test]
async fn an_unreadable_file_is_remembered_and_tried_again_a_week_later() {
    let t = make(None);
    t.library.add("bad", "bad.flac", Some(200), true);
    t.sidecar
        .errors
        .lock()
        .insert("bad.flac".into(), "empty or too short song".into());

    t.tick().await;
    assert_eq!(
        t.store.read(|s| s.failed["bad"].error.clone()),
        "empty or too short song"
    );
    let lookups = t.library.file_lookups.load(Ordering::SeqCst);
    t.tick().await;
    assert_eq!(t.sidecar.analysed().len(), 1);
    assert_eq!(t.library.file_lookups.load(Ordering::SeqCst), lookups);

    t.advance(RETRY_FAILED_AFTER + TimeDelta::hours(1));
    t.tick().await;
    t.tick().await;
    assert_eq!(t.sidecar.analysed().len(), 2);
}

// SonicAnalysisWorkerTests.AFileThatDropsTheConnectionTwice_IsMarkedUnreadable_AndThePassMovesOn
#[tokio::test]
async fn a_file_that_drops_the_connection_twice_is_marked_unreadable_and_the_pass_moves_on() {
    let t = make(None);
    t.library.add("crash", "crash.flac", Some(200), true);
    t.library.add("fine", "fine.flac", Some(200), true);
    t.sidecar.drops.lock().insert("crash.flac".into());

    t.tick().await;
    assert_eq!(
        t.state().1.as_deref(),
        Some("octo-sonic stopped answering; trying again.")
    );
    assert!(t.store.read(|s| s.failed.is_empty()));

    t.tick().await;
    assert_eq!(
        t.store.read(|s| s.failed["crash"].error.clone()),
        "octo-sonic stopped while reading it"
    );

    t.tick().await;
    assert_eq!(t.sidecar.analysed(), ["crash.flac", "crash.flac", "fine.flac"]);
}

// SonicAnalysisWorkerTests.ASongThatJustArrived_GoesFirst_EvenIfThePassHasNotListedIt
#[tokio::test]
async fn a_song_that_just_arrived_goes_first_even_if_the_pass_has_not_listed_it() {
    let t = make(None);
    t.library.add("a", "a.flac", Some(200), true);
    t.library.add("b", "b.flac", Some(200), true);
    t.tick().await;

    let arrival = t.library.add("new", "new.flac", Some(200), false);
    t.library.arrivals.lock().insert("new".into(), arrival);
    t.worker.arrived_first("new");
    t.tick().await;

    assert_eq!(t.sidecar.analysed(), ["a.flac", "new.flac"]);
}

// SonicAnalysisWorkerTests.ALongMix_IsNeverRead
#[tokio::test]
async fn a_long_mix_is_never_read() {
    let t = make(None);
    t.library.add("mix", "mix.flac", Some(2 * 3600), true);
    t.tick().await;
    assert!(t.sidecar.analysed().is_empty());
    assert_eq!(t.worker.status().state, "Done");
}

// SonicAnalysisWorkerTests.AtThePassEnd_GoneSongsAreForgotten_AndStartOverClearsEverything
#[tokio::test]
async fn at_the_pass_end_gone_songs_are_forgotten_and_start_over_clears_everything() {
    let t = make(None);
    t.store.write(|s| {
        s.songs.insert("gone".into(), analyzed_song());
    });
    t.library.add("a", "a.flac", Some(200), true);
    t.tick().await;
    t.tick().await;
    assert!(!t.store.read(|s| s.songs.contains_key("gone")));
    assert!(t.store.read(|s| s.songs.contains_key("a")));

    t.worker.reset();
    assert!(t.store.read(|s| s.songs.is_empty()));
    assert_eq!(t.store.read(|s| s.pass), 0);
}

// SonicAnalysisWorkerTests.APageThatFails_ForgetsNothing
#[tokio::test]
async fn a_page_that_fails_forgets_nothing() {
    let t = make(None);
    for i in 0..PAGE_SIZE + 5 {
        t.library.add_listed(&format!("s{i}"));
    }
    t.store.write(|s| {
        s.songs.insert(format!("s{}", PAGE_SIZE + 2), analyzed_song());
    });
    *t.library.failing_page.lock() = Some(PAGE_SIZE);

    t.tick().await;
    assert_eq!(t.worker.status().state, "Waiting");
    assert_eq!(t.store.read(|s| s.songs.len()), 1);
    assert_eq!(t.store.read(|s| s.pass), 0);
}

// SonicAnalysisWorkerTests.APageWithAMissingSong_IsNotTheEndOfTheList
#[tokio::test]
async fn a_page_with_a_missing_song_is_not_the_end_of_the_list() {
    let t = make(None);
    for i in 0..PAGE_SIZE + 5 {
        t.library.add_listed(&format!("s{i}"));
    }
    t.library.missing_ids.lock().insert("s3".into());
    let later = format!("s{}", PAGE_SIZE + 2);
    t.store.write(|s| {
        s.songs.insert(later.clone(), analyzed_song());
    });

    t.ticks_until_done().await;
    assert!(
        t.store.read(|s| s.songs.contains_key(&later)),
        "a song on the second page is still known"
    );
    // Every listed row has no file: left out, and counted.
    assert_eq!(t.worker.status().skipped, PAGE_SIZE + 4);
}

// SonicAnalysisWorkerTests.ASongThatArrivedDuringAPass_IsKeptAtItsEnd
#[tokio::test]
async fn a_song_that_arrived_during_a_pass_is_kept_at_its_end() {
    let t = make(None);
    t.library.add("a", "a.flac", Some(200), true);
    t.tick().await;
    let arrival = t.library.add("new", "new.flac", Some(200), false);
    t.library.arrivals.lock().insert("new".into(), arrival);
    t.worker.arrived_first("new");

    t.ticks_until_done().await;
    assert!(t.store.read(|s| s.songs.contains_key("new")));
}

// SonicAnalysisWorkerTests.ASongThatArrivesBetweenPasses_IsReadWithoutANewPass
#[tokio::test]
async fn a_song_that_arrives_between_passes_is_read_without_a_new_pass() {
    let t = make(None);
    t.library.add("a", "a.flac", Some(200), true);
    t.ticks_until_done().await;
    let arrival = t.library.add("new", "new.flac", Some(200), false);
    t.library.arrivals.lock().insert("new".into(), arrival);
    t.worker.arrived_first("new");

    t.tick().await;
    t.tick().await;
    assert_eq!(t.sidecar.analysed(), ["a.flac", "new.flac"]);
    assert_eq!(t.store.read(|s| s.pass), 1);
    assert_eq!(t.worker.status().state, "Done");
}

// SonicAnalysisWorkerTests.TurnedOffInEnv_ItIsOff_NotWaitingOrFailing
#[tokio::test]
async fn turned_off_in_env_it_is_off_not_waiting_or_failing() {
    let t = make(None);
    t.library.add("a", "a.flac", Some(200), true);
    t.sidecar.off.store(true, Ordering::SeqCst);
    t.tick().await;
    assert_eq!(
        t.state(),
        ("Off".to_string(), Some(TURNED_OFF_IN_ENV.to_string()))
    );
    assert!(t.sidecar.analysed().is_empty());
    assert!(t.store.read(|s| s.failed.is_empty()));
}

// SonicAnalysisWorkerTests.WhenOctoSonicCannotSeeTheMusic_ItWaits_AndBlamesNoSong
#[tokio::test]
async fn when_octo_sonic_cannot_see_the_music_it_waits_and_blames_no_song() {
    let t = make(None);
    t.library.add("a", "a.flac", Some(200), true);
    t.sidecar.blind.store(true, Ordering::SeqCst);
    t.tick().await;
    assert_eq!(t.state().1.as_deref(), Some(CANNOT_SEE_MUSIC));
    assert!(t.sidecar.analysed().is_empty());

    // It sees a folder, but not Octo's: every file is missing to it.
    t.sidecar.blind.store(false, Ordering::SeqCst);
    t.sidecar.missing.lock().insert("a.flac".into());
    t.tick().await;
    assert_eq!(t.state().1.as_deref(), Some(CANNOT_SEE_MUSIC));
    assert!(t.store.read(|s| s.failed.is_empty()));

    t.sidecar.missing.lock().clear();
    t.tick().await;
    assert!(t.store.read(|s| s.songs.contains_key("a")));
}

// SonicAnalysisWorkerTests.AnAnswerOctoCannotRead_IsThatSongsFailure
#[tokio::test]
async fn an_answer_octo_cannot_read_is_that_songs_failure() {
    let t = make(None);
    t.library.add("odd", "odd.flac", Some(200), true);
    t.library.add("fine", "fine.flac", Some(200), true);
    t.sidecar.garbled.lock().insert("odd.flac".into());
    t.ticks_until_done().await;
    assert!(t.store.read(|s| s.failed["odd"].error.contains("could not read")));
    assert!(t.store.read(|s| s.songs.contains_key("fine")));
}

// SonicAnalysisWorkerTests.ASongWithNoLength_IsLeftOut_AndCounted
#[tokio::test]
async fn a_song_with_no_length_is_left_out_and_counted() {
    let t = make(None);
    t.library.add("zero", "zero.flac", Some(0), true);
    t.library.add("none", "none.flac", None, true);
    t.library.add("fine", "fine.flac", Some(200), true);
    t.ticks_until_done().await;
    assert_eq!(t.sidecar.analysed(), ["fine.flac"]);
}

// SonicAnalysisWorkerTests.PauseAndStartOver_AreSavedAtOnce
#[tokio::test]
async fn pause_and_start_over_are_saved_at_once() {
    let t = make(None);
    t.worker.set_paused(true);
    let path = t.store.path().to_path_buf();
    assert!(
        std::fs::read_to_string(&path)
            .expect("written")
            .contains("\"Paused\":true")
    );
    t.worker.reset();
    assert!(
        std::fs::read_to_string(&path)
            .expect("written")
            .contains("\"Pass\":0")
    );
}

/// Rust-only: before its first look the analysis says it is about to start (3de8dde), and the
/// status names what the pass is reading.
#[tokio::test]
async fn before_its_first_look_it_says_it_is_about_to_start() {
    let t = make(None);
    assert_eq!(
        t.state(),
        (
            "Waiting".to_string(),
            Some("Starts a couple of minutes after Octo does.".to_string())
        )
    );
    t.library.add("a", "a.flac", Some(200), true);
    t.library.add("b", "b.flac", Some(200), true);
    t.tick().await;
    let status = t.worker.status();
    assert_eq!(
        (status.state.as_str(), status.analysed, status.total),
        ("Running", 1, 2)
    );
    assert_eq!(
        serde_json::to_value(&status).unwrap()["analysed"],
        serde_json::json!(1)
    );
}
