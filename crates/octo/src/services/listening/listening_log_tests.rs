use std::sync::Arc;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use octo_core::common::Clock;
use octo_core::models::radio::LastFmRadioPlay;
use octo_core::settings::{AppSettings, ListeningSettings, SettingsStore};
use parking_lot::Mutex;

use super::*;

/// A log in a fresh folder on a clock the test moves.
struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    settings: Arc<SettingsStore>,
    now: Arc<Mutex<DateTime<Utc>>>,
    log: ListeningLog,
}

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 18, 0, 0).unwrap()
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("listening");
        let settings = Arc::new(SettingsStore::for_tests(AppSettings::default()));
        let now = Arc::new(Mutex::new(start()));
        let log = Self::open(&path, &settings, &now);
        Fixture {
            _dir: dir,
            path,
            settings,
            now,
            log,
        }
    }

    fn open(path: &Path, settings: &Arc<SettingsStore>, now: &Arc<Mutex<DateTime<Utc>>>) -> ListeningLog {
        let clock = Arc::clone(now);
        ListeningLog::with_clock(path, Arc::clone(settings), Clock::new(move || *clock.lock()))
    }

    /// The same folder read again, as after a restart.
    fn reopened(&self) -> ListeningLog {
        Self::open(&self.path, &self.settings, &self.now)
    }

    fn advance(&self, by: TimeDelta) {
        *self.now.lock() += by;
    }

    fn set(&self, listening: ListeningSettings) {
        self.settings.set(AppSettings {
            listening,
            ..Default::default()
        });
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.path)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn play(id: &str, at: DateTime<Utc>, seconds: i64) -> Play {
    Play {
        song_id: id.into(),
        cover_id: id.into(),
        artist: "Drake".into(),
        title: format!("Song {id}"),
        album: "Album".into(),
        seconds,
        played_at: at,
        ..Default::default()
    }
}

#[test]
fn a_play_is_kept_across_a_restart_under_the_name_first_seen() {
    let f = Fixture::new();
    assert!(f.log.record("Winters", play("a", start(), 200)));
    assert!(
        f.log
            .record("winters", play("b", start() + TimeDelta::minutes(4), 180))
    );
    let again = f.reopened();
    assert_eq!(
        again.people(),
        vec![("winters".to_string(), "Winters".to_string())]
    );
    let plays = again.plays_of("WINTERS");
    assert_eq!(
        plays.iter().map(|p| p.song_id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(f.files(), ["listening.json", "winters.jsonl"]);
}

#[test]
fn a_play_without_a_song_or_a_person_is_not_written() {
    let f = Fixture::new();
    assert!(!f.log.record("", play("a", start(), 10)));
    let mut nameless = play("a", start(), 10);
    nameless.title = " ".into();
    assert!(!f.log.record("winters", nameless));
    assert!(f.log.people().is_empty());
}

#[test]
fn the_length_comes_from_the_now_playing_capped_at_the_song() {
    let f = Fixture::new();
    f.log.now_playing("winters", "a");
    f.advance(TimeDelta::seconds(95));
    assert_eq!(f.log.listened_seconds("winters", "a", Some(200)), 95);
    // Longer than the song (paused, then finished): the song's length.
    f.advance(TimeDelta::seconds(200));
    assert_eq!(f.log.listened_seconds("winters", "a", Some(200)), 200);
    // Past the song's length plus five minutes the now-playing is too old to time it.
    f.advance(TimeDelta::minutes(10));
    assert_eq!(f.log.listened_seconds("winters", "a", Some(200)), 200);
    // Another song, or another person, is timed by its length.
    f.log.now_playing("winters", "b");
    f.advance(TimeDelta::seconds(30));
    assert_eq!(f.log.listened_seconds("winters", "a", Some(240)), 240);
    assert_eq!(f.log.listened_seconds("alex", "b", Some(240)), 240);
    // No length and no now-playing: nothing to time it by.
    assert_eq!(f.log.listened_seconds("alex", "b", None), 0);
    assert_eq!(f.log.listened_seconds("winters", "b", None), 30);
}

#[test]
fn recording_off_writes_nothing() {
    let f = Fixture::new();
    f.set(ListeningSettings {
        enabled: false,
        ..Default::default()
    });
    f.log.now_playing("winters", "a");
    assert!(!f.log.record("winters", play("a", start(), 100)));
    f.log.heart("winters", &["a".into()]);
    assert!(f.log.people().is_empty());
    assert!(f.files().is_empty(), "{:?}", f.files());
}

#[test]
fn a_heart_while_the_song_plays_marks_its_play() {
    let f = Fixture::new();
    // Hearted mid-song, before the app reported the play.
    f.advance(TimeDelta::seconds(60));
    f.log.heart("winters", &["a".into()]);
    f.advance(TimeDelta::seconds(120));
    assert!(f.log.record("winters", play("a", start(), 180)));
    // Hearted a minute after the play ended.
    f.advance(TimeDelta::minutes(3));
    assert!(
        f.log
            .record("winters", play("b", start() + TimeDelta::minutes(5), 60))
    );
    f.advance(TimeDelta::minutes(1));
    f.log.heart("winters", &["b".into()]);
    // Hearted long after: the play is not marked.
    assert!(
        f.log
            .record("winters", play("c", start() + TimeDelta::minutes(6), 30))
    );
    f.advance(TimeDelta::minutes(30));
    f.log.heart("winters", &["c".into()]);
    let hearted: Vec<(String, bool)> = f
        .reopened()
        .plays_of("winters")
        .into_iter()
        .map(|p| (p.song_id, p.hearted))
        .collect();
    assert_eq!(
        hearted,
        [("a".into(), true), ("b".into(), true), ("c".into(), false)]
    );
}

#[test]
fn a_heart_from_someone_else_does_not_count() {
    let f = Fixture::new();
    f.log.heart("alex", &["a".into()]);
    f.advance(TimeDelta::seconds(100));
    f.log.record("winters", play("a", start(), 100));
    assert!(!f.log.plays_of("winters")[0].hearted);
}

#[test]
fn plays_older_than_the_days_kept_are_pruned() {
    let f = Fixture::new();
    f.set(ListeningSettings {
        keep_days: 7,
        ..Default::default()
    });
    f.log
        .record("winters", play("old", start() - TimeDelta::days(6), 100));
    f.log.record("winters", play("new", start(), 100));
    f.log
        .record("alex", play("alex-old", start() - TimeDelta::days(6), 100));
    f.advance(TimeDelta::days(2));
    assert_eq!(f.log.prune(), 2);
    assert_eq!(f.log.plays_of("winters").len(), 1);
    assert!(f.log.plays_of("alex").is_empty());
    // The files say the same.
    assert_eq!(f.reopened().plays_of("winters")[0].song_id, "new");
    assert_eq!(f.files(), ["listening.json", "winters.jsonl"]);
}

#[test]
fn clearing_removes_one_persons_plays_or_everyones() {
    let f = Fixture::new();
    f.log.record("winters", play("a", start(), 100));
    f.log.record("alex", play("b", start(), 100));
    f.log.record("alex", play("c", start(), 100));
    assert_eq!(f.log.clear(Some("Alex")), 2);
    assert_eq!(f.reopened().people().len(), 1);
    assert_eq!(f.log.clear(None), 1);
    assert!(f.reopened().people().is_empty());
}

fn radio_play(id: &str, source: &str, at: DateTime<Utc>) -> LastFmRadioPlay {
    LastFmRadioPlay {
        song_id: id.into(),
        artist: "Kavinsky".into(),
        title: format!("Song {id}"),
        album: Some("OutRun".into()),
        genre: Some("Electro".into()),
        duration: Some(240),
        is_local: true,
        source: source.into(),
        played_at_utc: at,
        ..Default::default()
    }
}

#[test]
fn the_radio_history_is_copied_in_once_without_the_starter_seeds() {
    let f = Fixture::new();
    let history = vec![(
        "Winters".to_string(),
        vec![
            radio_play("scrobbled", "scrobble", start() - TimeDelta::days(2)),
            radio_play("streamed", "internet-radio", start() - TimeDelta::days(1)),
            radio_play("seed", "bootstrap-starter", start() - TimeDelta::days(1)),
            radio_play("ancient", "scrobble", start() - TimeDelta::days(120)),
        ],
    )];
    assert_eq!(f.log.backfill(history.clone()), 2);
    let plays = f.log.plays_of("winters");
    assert_eq!(
        plays.iter().map(|p| p.song_id.as_str()).collect::<Vec<_>>(),
        ["scrobbled", "streamed"]
    );
    let first = &plays[0];
    assert_eq!(first.app, UNKNOWN_APP);
    assert_eq!(first.platform, UNKNOWN_PLATFORM);
    assert_eq!(first.source, "backfill");
    assert_eq!(first.seconds, 240);
    assert_eq!(first.genre.as_deref(), Some("Electro"));
    // The radio dates a play when it ended; the log, when it started.
    assert_eq!(
        first.played_at,
        start() - TimeDelta::days(2) - TimeDelta::seconds(240)
    );
    // Never again, even after a restart.
    assert_eq!(f.reopened().backfill(history), 0);
    assert_eq!(f.reopened().plays_of("winters").len(), 2);
}

#[test]
fn genre_answers_are_kept_including_none() {
    let f = Fixture::new();
    assert_eq!(f.log.artist_genre("Drake"), None);
    f.log.remember_artist_genre("Drake", Some("Rap"));
    f.log.remember_artist_genre("Nobody", None);
    f.log.remember_album_genre("al-1", Some("Electro"));
    let again = f.reopened();
    assert_eq!(again.artist_genre(" drake "), Some(Some("Rap".into())));
    assert_eq!(again.artist_genre("Nobody"), Some(None));
    assert_eq!(again.album_genre("al-1"), Some(Some("Electro".into())));
}

#[test]
fn copied_plays_take_their_artists_genre() {
    let f = Fixture::new();
    let mut bare = radio_play("a", "scrobble", start() - TimeDelta::days(1));
    bare.genre = None;
    f.log.backfill(vec![("winters".into(), vec![bare])]);
    assert_eq!(f.log.artists_missing_genres(), ["Kavinsky"]);
    f.log.remember_artist_genre("Kavinsky", Some("Synthwave"));
    assert!(f.log.artists_missing_genres().is_empty());
    assert_eq!(f.log.fill_genres_from_artists(), 1);
    assert_eq!(
        f.reopened().plays_of("winters")[0].genre.as_deref(),
        Some("Synthwave")
    );
}

#[test]
fn a_line_torn_by_a_crash_is_left_out() {
    let f = Fixture::new();
    f.log.record("winters", play("a", start(), 100));
    let file = f.path.join("winters.jsonl");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("{\"songId\":\"b\",\"art");
    std::fs::write(&file, text).unwrap();
    assert_eq!(f.reopened().plays_of("winters").len(), 1);
}

#[test]
fn any_name_is_a_safe_file_name() {
    for key in ["winters", "a.b", ".hidden", "../up", "名前", "x y/z"] {
        let stem = file_stem(key);
        assert!(!stem.contains('/') && !stem.starts_with('.'), "{key}: {stem}");
        assert_eq!(key_of_stem(&stem).as_deref(), Some(key));
    }
}

#[test]
fn the_platform_is_read_from_the_user_agents_parentheses() {
    assert_eq!(platform_of("Octo/1.9.0 (Android)"), "android");
    assert_eq!(platform_of("Octo/1.9.0 (iPhone)"), "iphone");
    assert_eq!(platform_of("Octo/1.9.0 (Windows)"), "windows");
    assert_eq!(
        platform_of("Symfonium/12.3.0 (Linux; Android 14; Pixel 8)"),
        "android"
    );
    assert_eq!(
        platform_of("Feishin/0.12 (Macintosh; Intel Mac OS X 14)"),
        "macos"
    );
    assert_eq!(platform_of("Feishin/0.12 (X11; Linux x86_64)"), "linux");
    assert_eq!(platform_of("Octo"), UNKNOWN_PLATFORM);
    assert_eq!(platform_of(""), UNKNOWN_PLATFORM);
}
