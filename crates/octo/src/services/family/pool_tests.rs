//! The family pool on a real folder: one file, links per member, the trash, better copies,
//! moving a song into the shared library, and copies where links do not work.

use std::path::Path;

use octo_core::models::domain::Song;
use serde_json::json;

use super::*;
use crate::services::family::family_playlist_store::FamilyPlaylistEntry;
use crate::services::family::test_support::{World, eventually, navidrome_with_songs, song_row, write_file};

const SONG: &str = "Artist/Album/01 Song.mp3";

fn song() -> Song {
    Song {
        title: "Song".into(),
        artist: "Artist".into(),
        album: "Album".into(),
        isrc: Some("USUM71703861".into()),
        ..Default::default()
    }
}

fn is_link(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

#[tokio::test]
async fn two_members_one_file() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 1_000);
    let alex = world.pool.link(&pooled, "alex").await.expect("linked");
    let sam = world.pool.link(&pooled, "Sam").await.expect("linked");
    assert_eq!(alex, format!("{}/alex/{SONG}", world.members()));
    assert_eq!(sam, format!("{}/sam/{SONG}", world.members()));
    assert!(is_link(&alex) && is_link(&sam));
    assert_eq!(std::fs::read(&alex).expect("reads through").len(), 1_000);
    assert_eq!(std::fs::read(&sam).expect("reads through").len(), 1_000);
    // One real audio file under the members folder: the pool's.
    let real: Vec<_> = walkdir(&world.members())
        .into_iter()
        .filter(|p| !is_link(p) && p.ends_with(".mp3"))
        .collect();
    assert_eq!(real, std::slice::from_ref(&pooled));
    assert_eq!(
        world.pool.store().get(&pooled).expect("recorded").refs,
        ["alex", "sam"]
    );
    assert_eq!(world.library.targeted(), [3, 4]);
    assert_eq!(world.index.placed_library_of(&alex), Some(3));

    // Linking again changes nothing.
    world.pool.link(&pooled, "alex").await.expect("still linked");
    assert_eq!(world.pool.store().get(&pooled).expect("recorded").refs.len(), 2);
}

#[tokio::test]
async fn link_is_relative() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 10);
    let link = world.pool.link(&pooled, "alex").await.expect("linked");
    let target = std::fs::read_link(&link).expect("a link");
    assert!(target.is_relative(), "{target:?}");
    assert_eq!(target.to_string_lossy(), format!("../../../.pool/{SONG}"));
    assert_eq!(relative_target("a.flac"), "../.pool/a.flac");
    // A relative link still works when the whole members folder moves.
    let moved = world.dir.path().join("moved");
    std::fs::rename(world.members(), &moved).expect("moved");
    assert_eq!(
        std::fs::read(moved.join("alex").join(SONG)).expect("reads").len(),
        10
    );
}

#[tokio::test]
async fn lyrics_and_cover_go_with_the_song() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 10);
    write_file(&pooled.replace(".mp3", ".lrc"), 3);
    write_file(
        &format!("{}/Artist/Album/cover.jpg", world.pool.pool_root_path()),
        3,
    );
    let link = world.pool.link(&pooled, "alex").await.expect("linked");
    assert!(is_link(&link.replace(".mp3", ".lrc")));
    assert!(is_link(&format!(
        "{}/alex/Artist/Album/cover.jpg",
        world.members()
    )));
    world.pool.unlink(&pooled, "alex").await.expect("unlinked");
    assert!(!Path::new(&format!("{}/alex/Artist", world.members())).exists());
}

#[tokio::test]
async fn remove_last_ref_trashes_file() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 10);
    world.library.map("test", "1", &pooled);
    world.pool.link(&pooled, "alex").await.expect("linked");
    world.pool.link(&pooled, "sam").await.expect("linked");

    world.pool.unlink(&pooled, "sam").await.expect("unlinked");
    assert!(Path::new(&pooled).is_file(), "Alex still has it");
    assert!(!Path::new(&format!("{}/sam/{SONG}", world.members())).exists());
    assert_eq!(world.pool.store().get(&pooled).expect("kept").refs, ["alex"]);

    world.pool.unlink(&pooled, "alex").await.expect("unlinked");
    assert!(!Path::new(&pooled).exists());
    assert!(world.pool.store().get(&pooled).is_none());
    let trashed: Vec<_> = walkdir(&format!("{}/.octo-trash", world.members()))
        .into_iter()
        .filter(|p| p.ends_with("01 Song.mp3"))
        .collect();
    assert_eq!(trashed.len(), 1, "in the trash, kept for the usual days");
    assert_eq!(
        world.library.forgotten.lock().as_slice(),
        std::slice::from_ref(&pooled)
    );
    assert!(!Path::new(&format!("{}/Artist", world.pool.pool_root_path())).exists());
}

#[tokio::test]
async fn copy_mode_when_links_fail() {
    let world = World::new(None);
    assert_eq!(world.pool.probe_links().links, LinkMode::Symlink);
    assert_eq!(world.pool.link_status().message, None);

    // A members folder that cannot be made (a file is in the way) cannot link either.
    let blocked = world.dir.path().join("blocked");
    std::fs::write(&blocked, b"x").expect("a file");
    let mut settings = world.settings.current().as_ref().clone();
    settings.family.members_path = blocked.join("members").to_string_lossy().into_owned();
    world.settings.set(settings);
    let status = world.pool.probe_links();
    assert_eq!(status.links, LinkMode::Copy);
    assert_eq!(status.message.as_deref(), Some(COPY_MODE_MESSAGE));
    assert!(
        status
            .fix
            .as_deref()
            .is_some_and(|fix| fix.contains("--vfs-links"))
    );
}

#[tokio::test]
async fn copies_replace_links_when_navidrome_never_shows_them() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 10);
    let link = world.pool.link(&pooled, "alex").await.expect("linked");
    assert!(!world.pool.links_seen());
    assert_eq!(world.pool.fall_back_to_copies(), 1);
    assert!(!is_link(&link) && Path::new(&link).is_file());
    assert_eq!(world.pool.mode(), LinkMode::Copy);
    let other = world.pool_file("Artist/Album/02 Other.mp3", 5);
    let copy = world.pool.link(&other, "sam").await.expect("copied");
    assert!(!is_link(&copy) && Path::new(&copy).is_file());
    assert!(world.pool.links_seen(), "copy mode needs no links");
}

#[tokio::test]
async fn upgrade_repoints_every_link() {
    let world = World::new(None);
    let mp3 = world.pool_file(SONG, 10);
    world.pool.record(PoolFile {
        path: mp3.clone(),
        size: 10,
        format: "mp3".into(),
        outside_ids: vec!["test:1".into()],
        isrc: Some("USUM71703861".into()),
        title: "Song".into(),
        artist: "Artist".into(),
        album: "Album".into(),
        refs: Vec::new(),
    });
    let alex_old = world.pool.link(&mp3, "alex").await.expect("linked");
    let sam_old = world.pool.link(&mp3, "sam").await.expect("linked");
    let flac_rel = SONG.replace(".mp3", ".flac");
    let alex_new = format!("{}/alex/{flac_rel}", world.members());
    let sam_new = format!("{}/sam/{flac_rel}", world.members());

    let members = world.members();
    let navidrome = navidrome_with_songs(json!([
        song_row("old-alex", 3, &format!("{members}/alex"), SONG, 10),
        song_row("old-sam", 4, &format!("{members}/sam"), SONG, 10),
        song_row("new-alex", 3, &format!("{members}/alex"), &flac_rel, 30),
        song_row("new-sam", 4, &format!("{members}/sam"), &flac_rel, 30),
    ]))
    .await;
    world.point_at(&navidrome.uri());
    world.index.refresh().await.expect("read");
    world
        .stars
        .starred
        .lock()
        .insert(("alex".into(), "old-alex".into()));
    let road_trip = world.playlists.create("Road trip", "sam", chrono::Utc::now());
    world.playlists.add_entries(
        &road_trip.id,
        vec![FamilyPlaylistEntry {
            library_id: Some("old-sam".into()),
            scope: "Personal:sam".into(),
            outside_id: None,
            added_by: "sam".into(),
            added: chrono::Utc::now(),
            pending: false,
            duration: None,
            explicit_status: None,
        }],
    );

    let flac = world.pool_file(&flac_rel, 30);
    let first = world.pool.placed(&flac, &song(), "test", "1", &[]).await;
    assert_eq!(first, None, "nobody new asked");

    assert!(is_link(&alex_new) && is_link(&sam_new));
    assert!(!Path::new(&alex_old).exists() && !Path::new(&sam_old).exists());
    assert_eq!(std::fs::read(&alex_new).expect("reads").len(), 30);
    assert!(!Path::new(&mp3).exists(), "the old copy went to the trash");
    let better = world.pool.store().get(&flac).expect("recorded");
    assert_eq!(better.refs, ["alex", "sam"]);
    assert!(better.outside_ids.contains(&"test:1".to_string()));
    assert!(world.pool.store().get(&mp3).is_none());

    assert!(
        eventually(|| world
            .stars
            .asked
            .lock()
            .contains(&("alex".into(), "new-alex".into())))
        .await,
        "Alex's star follows the song"
    );
    assert!(!world.stars.asked.lock().iter().any(|(user, _)| user == "sam"));
    assert!(
        eventually(|| {
            world.playlists.get(&road_trip.id).expect("there").entries[0]
                .library_id
                .as_deref()
                == Some("new-sam")
        })
        .await
    );
}

#[tokio::test]
async fn promote_removes_links() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 10);
    write_file(&pooled.replace(".mp3", ".lrc"), 3);
    world.pool.link(&pooled, "alex").await.expect("linked");
    world.pool.link(&pooled, "sam").await.expect("linked");

    let shared = world.pool.promote(&pooled).await.expect("moved");
    assert_eq!(shared, format!("{}/{SONG}", world.music()));
    assert_eq!(std::fs::read(&shared).expect("there").len(), 10);
    assert!(
        Path::new(&shared.replace(".mp3", ".lrc")).is_file(),
        "lyrics went with it"
    );
    assert!(!Path::new(&pooled).exists());
    for user in ["alex", "sam"] {
        assert!(!Path::new(&format!("{}/{user}/Artist", world.members())).exists());
    }
    assert!(world.pool.store().list().is_empty());
    assert_eq!(
        world.library.restored.lock().as_slice(),
        std::slice::from_ref(&shared)
    );
    assert_eq!(world.library.full_scans(), 1);
}

#[tokio::test]
async fn settle_links_members_and_moves_a_shared_ask() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 10);
    let kept = world.pool.settle(&pooled, &["alex".to_string()], false).await;
    assert_eq!(kept, pooled);
    assert!(is_link(&format!("{}/alex/{SONG}", world.members())));
    let moved = world.pool.settle(&pooled, &["alex".to_string()], true).await;
    assert_eq!(moved, format!("{}/{SONG}", world.music()));
    // A path outside the pool is left alone.
    assert_eq!(
        world.pool.settle(&moved, &["sam".to_string()], false).await,
        moved
    );
}

#[tokio::test]
async fn songs_are_found_by_outside_id_isrc_or_name() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 10);
    world.pool.placed(&pooled, &song(), "test", "1", &[]).await;
    assert_eq!(
        world
            .pool
            .find_song("test", "1", &Song::default())
            .map(|f| f.path),
        Some(pooled.clone())
    );
    let by_isrc = Song {
        isrc: Some("usum71703861".into()),
        ..Default::default()
    };
    assert!(world.pool.find_song("test", "2", &by_isrc).is_some());
    let by_name = Song {
        artist: "artist".into(),
        title: "SONG".into(),
        ..Default::default()
    };
    assert!(world.pool.find_song("test", "3", &by_name).is_some());
    let live = Song {
        artist: "Artist".into(),
        title: "Song (Live)".into(),
        ..Default::default()
    };
    assert!(world.pool.find_song("test", "4", &live).is_none());
}

#[tokio::test]
async fn family_off_places_nothing() {
    let world = World::with_family(None, false);
    assert_eq!(world.pool.pool_root(), None);
    assert!(
        !world
            .pool
            .is_pool_path(&format!("{}/.pool/{SONG}", world.members()))
    );
    assert!(world.pool.find_song("test", "1", &song()).is_none());
}

/// Every file and link under `root`.
fn walkdir(root: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = entry.file_type().expect("a kind");
            if kind.is_dir() {
                stack.push(path);
            } else {
                found.push(path.to_string_lossy().into_owned());
            }
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn a_pool_file_deleted_by_hand_takes_its_links_with_it() {
    let world = World::new(None);
    let pooled = world.pool_file(SONG, 10);
    world.library.map("test", "1", &pooled);
    let link = world.pool.link(&pooled, "alex").await.expect("linked");
    std::fs::remove_file(&pooled).expect("deleted by hand");
    assert_eq!(world.pool.sweep_dangling().await, 1);
    assert!(
        std::fs::symlink_metadata(&link).is_err(),
        "the dangling link went"
    );
    assert!(world.pool.store().list().is_empty());
    assert_eq!(world.library.forgotten.lock().as_slice(), [pooled]);
    assert_eq!(world.pool.sweep_dangling().await, 0);
}
