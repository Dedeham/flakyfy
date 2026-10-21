//! The pre-cache's memory must survive a restart and a damaged file must never stop it.

use super::*;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("octo-precache-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn cached(id: &str) -> Cached {
    Cached {
        owner: "jamie".into(),
        list: "foryou:newreleases".into(),
        provider: "soulseek".into(),
        external_id: id.into(),
        artist: "Artist".into(),
        title: "Title".into(),
        path: "/music/a.flac".into(),
        cached_utc: DateTime::UNIX_EPOCH + TimeDelta::days(20_000),
        note: None,
    }
}

#[test]
fn update_is_written_and_read_back_by_a_new_store() {
    let path = dir("roundtrip").join("precache.json");
    let store = PrecacheStore::new(Some(path.clone()));
    store.update(|s| {
        s.picks.push(Pick {
            owner: "jamie".into(),
            key: "foryou:newreleases".into(),
            count: Some(8),
        });
        s.cached.push(cached("x1"));
    });

    let again = PrecacheStore::new(Some(path));
    again.read(|s| {
        assert_eq!(s.picks.len(), 1);
        assert_eq!(s.picks[0].count, Some(8));
        assert_eq!(s.cached, vec![cached("x1")]);
    });
}

#[test]
fn a_corrupt_file_is_set_aside_and_the_store_starts_empty() {
    let folder = dir("corrupt");
    let path = folder.join("precache.json");
    std::fs::write(&path, "{ not json").expect("written");

    let store = PrecacheStore::new(Some(path.clone()));

    assert!(store.read(|s| s.picks.is_empty() && s.cached.is_empty()));
    assert!(!path.exists());
    assert!(folder.join("precache.json.corrupt").exists());
}

#[test]
fn a_store_without_a_path_keeps_its_state_in_memory() {
    let store = PrecacheStore::new(None);
    store.update(|s| s.cached.push(cached("x1")));
    assert_eq!(store.read(|s| s.cached.len()), 1);
}

#[test]
fn a_song_let_go_is_left_alone_for_ninety_days_only() {
    let at = DateTime::UNIX_EPOCH + TimeDelta::days(20_000);
    let state = PrecacheState {
        let_go: vec![LetGo {
            key: "soulseek:x1".into(),
            at_utc: at,
        }],
        ..Default::default()
    };
    assert!(state.recently_let_go("soulseek:x1", at + TimeDelta::days(89)));
    assert!(!state.recently_let_go("soulseek:x1", at + TimeDelta::days(91)));
    assert!(!state.recently_let_go("soulseek:other", at));
}
