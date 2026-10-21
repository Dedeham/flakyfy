//! The pre-cache fetches a few songs and later lets some go, so the cases that matter are the
//! ones where it must not: a song someone took up, a song it cannot be sure about, a dry run,
//! and a round while someone else downloads.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use octo_core::settings::AppSettings;
use parking_lot::Mutex;

use super::super::precache_store::Pick;
use super::*;

const LIST: &str = "for:newreleases";

fn at(days: i64) -> DateTime<Utc> {
    DateTime::UNIX_EPOCH + TimeDelta::days(20_000 + days)
}

fn song(id: &str) -> Candidate {
    Candidate {
        provider: "soulseek".into(),
        external_id: id.into(),
        artist: "Artist".into(),
        title: format!("Song {id}"),
    }
}

#[derive(Default)]
struct World {
    songs: Vec<Candidate>,
    here: HashSet<String>,
    busy: bool,
    fail_fetches: bool,
    uptake: Uptake,
    song_known: bool,
    refuse_let_go: bool,
    fetched: Mutex<Vec<String>>,
    let_go: Mutex<Vec<String>>,
}

#[async_trait]
impl PrecacheWorld for World {
    fn lists(&self) -> Vec<ListInfo> {
        Vec::new()
    }
    fn outside_songs(&self, _: &str, _: &str) -> Vec<Candidate> {
        self.songs.clone()
    }
    fn busy(&self) -> bool {
        self.busy
    }
    async fn already_here(&self, song: &Candidate) -> bool {
        self.here.contains(&song.key())
    }
    async fn fetch(&self, song: &Candidate) -> Result<String, String> {
        if self.fail_fetches {
            return Err("no peer".into());
        }
        self.fetched.lock().push(song.key());
        Ok(format!("/music/{}.flac", song.external_id))
    }
    fn file_exists(&self, _: &str) -> bool {
        true
    }
    async fn song_id(&self, _: &Cached) -> Option<String> {
        self.song_known.then(|| "nd-1".to_string())
    }
    async fn uptake(&self, _: &str) -> Uptake {
        self.uptake
    }
    async fn let_go(&self, song_id: &str) -> Result<(), String> {
        if self.refuse_let_go {
            return Err("library actions are off".into());
        }
        self.let_go.lock().push(song_id.to_string());
        Ok(())
    }
}

fn worker(
    world: World,
    precache: bool,
    dry_run: bool,
    now: DateTime<Utc>,
) -> (Arc<PlaylistPrecacheWorker>, Arc<World>) {
    let mut settings = AppSettings::default();
    settings.generated_playlists.precache = precache;
    settings.generated_playlists.precache_dry_run = dry_run;
    settings.generated_playlists.precache_count = 3;
    let store = Arc::new(PrecacheStore::new(None));
    store.update(|s| {
        s.picks.push(Pick {
            owner: "jamie".into(),
            key: LIST.into(),
            count: None,
        });
    });
    let world = Arc::new(world);
    let worker = PlaylistPrecacheWorker::new(
        store,
        Arc::new(SettingsStore::for_tests(settings)),
        world.clone(),
        Clock::fixed(now),
    );
    (Arc::new(worker), world)
}

fn cached(id: &str, cached_utc: DateTime<Utc>) -> Cached {
    Cached {
        owner: "jamie".into(),
        list: LIST.into(),
        provider: "soulseek".into(),
        external_id: id.into(),
        artist: "Artist".into(),
        title: format!("Song {id}"),
        path: format!("/music/{id}.flac"),
        cached_utc,
        note: None,
    }
}

fn five_songs() -> Vec<Candidate> {
    ["a", "b", "c", "d", "e"].map(song).to_vec()
}

fn settings_of(worker: &PlaylistPrecacheWorker) -> GeneratedPlaylistSettings {
    worker.settings.current().generated_playlists.clone()
}

#[tokio::test]
async fn fill_fetches_up_to_the_count_and_records_each_song() {
    let (worker, world) = worker(
        World {
            songs: five_songs(),
            ..Default::default()
        },
        true,
        true,
        at(0),
    );

    assert!(worker.fill(&settings_of(&worker)).await);

    assert_eq!(world.fetched.lock().len(), 3);
    assert_eq!(worker.store.read(|s| s.cached.len()), 3);
}

#[tokio::test]
async fn fill_leaves_songs_already_here_or_cached_or_let_go_alone() {
    let world = World {
        songs: five_songs(),
        here: HashSet::from(["soulseek:a".to_string()]),
        ..Default::default()
    };
    let (worker, world) = worker(world, true, true, at(0));
    worker.store.update(|s| {
        s.cached.push(cached("b", at(0)));
        s.let_go.push(LetGo {
            key: "soulseek:c".into(),
            at_utc: at(-1),
        });
    });

    worker.fill(&settings_of(&worker)).await;

    // a is here, b is cached, c was let go: only d and e are left, and b already counts as one.
    let mut fetched = world.fetched.lock().clone();
    fetched.sort();
    assert_eq!(fetched, ["soulseek:d", "soulseek:e"]);
}

#[tokio::test]
async fn fill_does_nothing_for_a_list_that_already_has_its_count() {
    let (worker, world) = worker(
        World {
            songs: five_songs(),
            ..Default::default()
        },
        true,
        true,
        at(0),
    );
    worker.store.update(|s| {
        for id in ["x", "y", "z"] {
            s.cached.push(cached(id, at(0)));
        }
    });

    assert!(worker.fill(&settings_of(&worker)).await);

    assert!(world.fetched.lock().is_empty());
}

#[tokio::test]
async fn fill_stops_while_someone_else_is_downloading() {
    let (worker, world) = worker(
        World {
            songs: five_songs(),
            busy: true,
            ..Default::default()
        },
        true,
        true,
        at(0),
    );

    assert!(!worker.fill(&settings_of(&worker)).await);

    assert!(world.fetched.lock().is_empty());
}

#[tokio::test]
async fn fill_gives_up_after_three_fetches_fail_in_a_row() {
    let (worker, _) = worker(
        World {
            songs: five_songs(),
            fail_fetches: true,
            ..Default::default()
        },
        true,
        true,
        at(0),
    );

    assert!(!worker.fill(&settings_of(&worker)).await);
    assert_eq!(worker.store.read(|s| s.cached.len()), 0);
}

#[tokio::test]
async fn expire_leaves_a_song_that_is_not_old_enough() {
    let world = World {
        song_known: true,
        ..Default::default()
    };
    let (worker, world) = worker(world, true, false, at(29));
    worker.store.update(|s| s.cached.push(cached("a", at(0))));

    worker.expire(&settings_of(&worker)).await;

    assert!(world.let_go.lock().is_empty());
    assert_eq!(worker.store.read(|s| s.cached.len()), 1);
}

#[tokio::test]
async fn expire_lets_go_of_an_untouched_song_and_remembers_it() {
    let world = World {
        song_known: true,
        uptake: Uptake {
            marked: Some(false),
            plays: Some(0),
        },
        ..Default::default()
    };
    let (worker, world) = worker(world, true, false, at(31));
    worker.store.update(|s| s.cached.push(cached("a", at(0))));

    worker.expire(&settings_of(&worker)).await;

    assert_eq!(*world.let_go.lock(), ["nd-1"]);
    worker.store.read(|s| {
        assert!(s.cached.is_empty());
        assert!(s.recently_let_go("soulseek:a", at(31)));
    });
}

#[tokio::test]
async fn expire_on_a_dry_run_only_says_what_would_go() {
    let world = World {
        song_known: true,
        uptake: Uptake {
            marked: Some(false),
            plays: Some(0),
        },
        ..Default::default()
    };
    let (worker, world) = worker(world, true, true, at(31));
    worker.store.update(|s| s.cached.push(cached("a", at(0))));

    worker.expire(&settings_of(&worker)).await;

    assert!(world.let_go.lock().is_empty());
    worker.store.read(|s| {
        assert_eq!(s.cached.len(), 1);
        assert!(s.cached[0].note.as_deref().is_some_and(|n| n.contains("dry run")));
    });
}

#[tokio::test]
async fn expire_hands_a_starred_song_to_the_library() {
    let world = World {
        song_known: true,
        uptake: Uptake {
            marked: Some(true),
            plays: Some(0),
        },
        ..Default::default()
    };
    let (worker, world) = worker(world, true, false, at(31));
    worker.store.update(|s| s.cached.push(cached("a", at(0))));

    worker.expire(&settings_of(&worker)).await;

    assert!(world.let_go.lock().is_empty());
    assert!(worker.store.read(|s| s.cached.is_empty()));
}

#[tokio::test]
async fn expire_leaves_a_song_whose_marks_cannot_be_read_for_the_next_look() {
    let world = World {
        song_known: true,
        uptake: Uptake {
            marked: None,
            plays: Some(0),
        },
        ..Default::default()
    };
    let (worker, world) = worker(world, true, false, at(31));
    worker.store.update(|s| s.cached.push(cached("a", at(0))));

    worker.expire(&settings_of(&worker)).await;

    assert!(world.let_go.lock().is_empty());
    assert_eq!(worker.store.read(|s| s.cached.len()), 1);
}

#[tokio::test]
async fn expire_keeps_the_song_and_says_why_when_it_cannot_be_let_go() {
    let world = World {
        song_known: true,
        refuse_let_go: true,
        uptake: Uptake {
            marked: Some(false),
            plays: Some(0),
        },
        ..Default::default()
    };
    let (worker, _) = worker(world, true, false, at(31));
    worker.store.update(|s| s.cached.push(cached("a", at(0))));

    worker.expire(&settings_of(&worker)).await;

    worker.store.read(|s| {
        assert_eq!(s.cached.len(), 1);
        assert!(
            s.cached[0]
                .note
                .as_deref()
                .is_some_and(|n| n.contains("library actions are off"))
        );
    });
}

#[tokio::test]
async fn expire_waits_for_navidrome_to_show_a_song() {
    let (worker, world) = worker(World::default(), true, false, at(31));
    worker.store.update(|s| s.cached.push(cached("a", at(0))));

    worker.expire(&settings_of(&worker)).await;

    assert!(world.let_go.lock().is_empty());
    assert_eq!(worker.store.read(|s| s.cached.len()), 1);
}

#[tokio::test]
async fn tick_does_nothing_while_the_pre_cache_is_off() {
    let (worker, world) = worker(
        World {
            songs: five_songs(),
            ..Default::default()
        },
        false,
        true,
        at(0),
    );

    worker.tick().await;

    assert!(world.fetched.lock().is_empty());
    assert!(worker.store.read(|s| s.last_fill_utc.is_none()));
}

#[tokio::test]
async fn tick_fills_once_and_stamps_the_round() {
    let (worker, world) = worker(
        World {
            songs: five_songs(),
            ..Default::default()
        },
        true,
        true,
        at(0),
    );

    worker.tick().await;
    worker.tick().await;

    // The second tick is within the interval: nothing more is fetched.
    assert_eq!(world.fetched.lock().len(), 3);
    worker.store.read(|s| {
        assert_eq!(s.last_fill_utc, Some(at(0)));
        assert_eq!(s.last_expiry_utc, Some(at(0)));
    });
}
