//! FindSongsPickTests: Find songs picks as they reach the queues: a second pick of a song waiting
//! in the upgrade queue, picks no download used, and how many looks one person may run at once.

use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use octo_core::models::download::AcquisitionCandidate;
use octo_core::settings::{
    AppSettings, LibraryAction, LibraryActionDefinition, LibraryActionSettings, SettingsStore,
    SoulseekSettings,
};
use tokio_util::sync::CancellationToken;

use super::download_log_tests::upgrade_worker;
use super::download_picks::{DownloadPicks, PickedCopy, SOULSEEK_SOURCE};
use super::song_finder::{
    FindSnapshot, FindTarget, FoundCopy, SearchAnswer, SongFinder, SongFinderServices, find_states,
    library_action_states,
};
use super::track_acquisition_queue::TrackAcquisitionQueue;
use crate::services::library::LibraryActionState;
use crate::services::library::UpgradeSources;
use crate::services::library::library_action_executor::LibraryActionOutcome;
use crate::services::library::upgrade_queue::{UpgradeAsk, UpgradeQueue};

fn owned() -> FindTarget {
    FindTarget {
        library_id: Some("nd-1".into()),
        owned_format: Some("mp3".into()),
        ..FindTarget::new("Air", "Sexy Boy", Some("Moon Safari"), None, "nd-1")
    }
}

struct Server {
    finder: Arc<SongFinder>,
    queue: Arc<UpgradeQueue>,
    picks: Arc<DownloadPicks>,
}

fn flac(peer: &str, rank: i32) -> FoundCopy {
    let file = r"Music\Air\Moon Safari (1998)\02 - Sexy Boy.flac";
    FoundCopy::new(
        AcquisitionCandidate {
            source: SOULSEEK_SOURCE.into(),
            peer: Some(peer.into()),
            file: Some("02 - Sexy Boy.flac".into()),
            folder: Some("Moon Safari (1998)".into()),
            format: Some("flac".into()),
            size: Some(30_000_000),
            length: Some(298),
            rank: Some(rank),
            ..Default::default()
        },
        PickedCopy {
            source: SOULSEEK_SOURCE.into(),
            peer: Some(peer.into()),
            file: Some(file.into()),
            size: 30_000_000,
            format: Some("flac".into()),
            length: Some(298),
            ..Default::default()
        },
    )
}

fn three_flacs() -> anyhow::Result<SearchAnswer> {
    Ok((
        vec![flac("peer1", 1), flac("peer2", 2), flac("peer3", 3)],
        vec!["Air Sexy Boy".to_string()],
        Some("3 files from 3 peers; 3 fit the song".to_string()),
    ))
}

fn build(target: FindTarget, acquisitions: Option<Arc<TrackAcquisitionQueue>>) -> Server {
    let settings = Arc::new(SettingsStore::for_tests(AppSettings {
        library_actions: LibraryActionSettings {
            enabled: true,
            dry_run: false,
            allowed_users: vec!["alice".into(), "bob".into()],
            actions: vec![LibraryActionDefinition {
                action: LibraryAction::BetterQuality,
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        },
        soulseek: SoulseekSettings {
            base_url: Some("http://slskd.test".into()),
            username: Some("u".into()),
            password: Some("p".into()),
            ..Default::default()
        },
        ..Default::default()
    }));
    let queue = Arc::new(UpgradeQueue::new());
    let picks = Arc::new(DownloadPicks::default());
    let finder = Arc::new(SongFinder::new(SongFinderServices {
        settings: Some(settings.clone()),
        upgrade_queue: Some(queue.clone()),
        upgrade_sources: Some(Arc::new(UpgradeSources::new(settings, None))),
        executor: true,
        acquisitions,
        picks: Some(picks.clone()),
        ..Default::default()
    }));
    finder.seams(|seams| {
        seams.resolve = Arc::new(move |_, _| {
            let target = target.clone();
            async move { Some(target) }.boxed()
        });
        seams.sources_for = Arc::new(|_| vec![(SOULSEEK_SOURCE.to_string(), None)]);
        seams.search_soulseek = Arc::new(|_, _| async { three_flacs() }.boxed());
    });
    Server { finder, queue, picks }
}

async fn look(finder: &Arc<SongFinder>, user: &str) -> FindSnapshot {
    let id = finder
        .start("nd-1", user, &CancellationToken::new())
        .await
        .expect("a look")
        .id;
    finder.wait(&id, Duration::from_secs(10)).await.expect("the look")
}

fn queued_peer(queue: &UpgradeQueue) -> Option<String> {
    let jobs = queue.snapshot();
    assert_eq!(jobs.len(), 1);
    jobs[0].pick.as_ref().and_then(|p| p.peer.clone())
}

// ---- A second pick ----------------------------------------------------------------------

#[tokio::test]
async fn a_second_pick_before_the_upgrade_starts_takes_the_place_of_the_first() {
    let server = build(owned(), None);
    let look = look(&server.finder, "alice").await;

    assert_eq!(
        server
            .finder
            .pick_by_id(&look.id, &look.copies[0].id, "alice")
            .await
            .state,
        library_action_states::QUEUED
    );
    assert_eq!(queued_peer(&server.queue).as_deref(), Some("peer1"));

    let second = server
        .finder
        .pick_by_id(&look.id, &look.copies[1].id, "alice")
        .await;

    assert_eq!(second.state, library_action_states::QUEUED);
    assert!(
        second.detail.starts_with("Changed to the copy you picked"),
        "{}",
        second.detail
    );
    assert_eq!(queued_peer(&server.queue).as_deref(), Some("peer2"));
}

#[tokio::test]
async fn a_pick_for_an_upgrade_already_running_or_someone_elses_says_another_copy_is_on_its_way() {
    let server = build(owned(), None);
    let alice = look(&server.finder, "alice").await;
    server
        .finder
        .pick_by_id(&alice.id, &alice.copies[0].id, "alice")
        .await;

    let bobs = look(&server.finder, "bob").await;
    let bob = server
        .finder
        .pick_by_id(&bobs.id, &bobs.copies[2].id, "bob")
        .await;
    assert_eq!(bob.state, library_action_states::SKIPPED);
    assert_eq!(
        bob.detail,
        "Another copy of this song is already on its way. Pick again once it ends."
    );

    assert!(server.queue.take_next().is_some());
    let late = server
        .finder
        .pick_by_id(&alice.id, &alice.copies[1].id, "alice")
        .await;
    assert_eq!(late.state, library_action_states::SKIPPED);
    assert_eq!(queued_peer(&server.queue).as_deref(), Some("peer1"));

    // The same copy again is the pick already waiting, not another one.
    assert_eq!(
        server
            .finder
            .pick_by_id(&alice.id, &alice.copies[0].id, "alice")
            .await
            .state,
        library_action_states::QUEUED
    );
}

// ---- Picks no download used ---------------------------------------------------------------

#[tokio::test]
async fn a_pick_is_let_go_when_its_download_ends_without_using_it() {
    let target = FindTarget {
        external_id: Some("ext1".into()),
        ..FindTarget::new("Air", "Sexy Boy", Some("Moon Safari"), Some(298), "ext1")
    };
    let acquisitions = Arc::new(TrackAcquisitionQueue::new());
    let server = build(target, Some(acquisitions.clone()));
    let look = look(&server.finder, "alice").await;

    assert_eq!(
        server
            .finder
            .pick_by_id(&look.id, &look.copies[0].id, "alice")
            .await
            .state,
        library_action_states::QUEUED
    );
    assert!(server.picks.has("ext1"));

    // The download ends at once, "already in your library", and never takes the pick.
    let request = acquisitions
        .dequeue(&CancellationToken::new())
        .await
        .expect("the request");
    acquisitions.release(&request);
    request
        .completion
        .try_set_result("/music/Air/Moon Safari/02 - Sexy Boy.flac");
    for _ in 0..200 {
        if !server.picks.has("ext1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(!server.picks.has("ext1"));
}

#[test]
fn forgetting_a_pick_leaves_a_newer_one_for_the_same_song() {
    let picks = DownloadPicks::default();
    let copy = |peer: &str| {
        Arc::new(PickedCopy {
            source: SOULSEEK_SOURCE.into(),
            peer: Some(peer.into()),
            file: Some("a.flac".into()),
            ..Default::default()
        })
    };
    let (first, second) = (copy("p1"), copy("p2"));
    picks.pin("ext1", first.clone());
    picks.pin("ext1", second);

    assert!(!picks.forget("ext1", &first));
    assert_eq!(
        picks
            .take("ext1", "Soulseek")
            .and_then(|p| p.peer.clone())
            .as_deref(),
        Some("p2")
    );
}

#[tokio::test]
async fn an_upgrade_lets_go_of_a_pick_it_never_used() {
    let queue = Arc::new(UpgradeQueue::new());
    let picks = Arc::new(DownloadPicks::default());
    queue.add(
        vec![UpgradeAsk {
            title: Some("Sexy Boy".into()),
            artist: Some("Air".into()),
            album: Some("Moon Safari".into()),
            suffix: Some("mp3".into()),
            pick: Some((*flac("peer1", 1).pick).clone()),
            ..UpgradeAsk::new("nd-1")
        }],
        "alice",
        "find",
    );
    let worker = upgrade_worker(&queue, None, picks.clone(), |request| {
        (request.on_replacement_queued.as_ref().expect("a follower"))("soulseek", "ext1");
        // Joined a download already running, so this pick was never taken.
        LibraryActionOutcome::new(LibraryActionState::Failed, Some("joined".into()))
    });

    worker.tick(&CancellationToken::new()).await;
    worker.drain().await;

    assert!(!picks.has("ext1"));
}

// ---- How many looks run at once --------------------------------------------------------------

#[tokio::test]
async fn one_person_runs_two_looks_at_once_and_another_persons_look_is_never_dropped_for_room() {
    let slow_song = FindTarget {
        external_id: Some("ext1".into()),
        ..FindTarget::new("Air", "Sexy Boy", Some("Moon Safari"), Some(298), "ext1")
    };
    let quick_song = FindTarget {
        title: "Kelly Watch the Stars".into(),
        external_id: Some("ext2".into()),
        ..slow_song.clone()
    };
    let server = build(slow_song.clone(), None);
    let (release, released) = tokio::sync::watch::channel(false);
    let slow_title = slow_song.title.clone();
    server.finder.seams(|seams| {
        // Looks for "slow" wait; every other one ends at once.
        seams.resolve = Arc::new(move |id, _| {
            let found = if id == "slow" {
                slow_song.clone()
            } else {
                quick_song.clone()
            };
            async move { Some(found) }.boxed()
        });
        seams.search_soulseek = Arc::new(move |song, _| {
            let slow = song.title == slow_title;
            let mut released = released.clone();
            async move {
                if slow {
                    let _ = released.wait_for(|done| *done).await;
                }
                Ok((Vec::new(), Vec::new(), Some("Nothing found".to_string())))
            }
            .boxed()
        });
    });
    let ct = CancellationToken::new();

    let bobs = server.finder.start("slow", "bob", &ct).await.expect("bob's look");
    for _ in 0..2 {
        assert_eq!(
            server
                .finder
                .start("slow", "alice", &ct)
                .await
                .expect("a look")
                .state,
            find_states::SEARCHING
        );
    }
    let third = server.finder.start("slow", "alice", &ct).await.expect("a look");

    assert_eq!(third.state, find_states::FAILED);
    assert_eq!(
        third.error.as_deref(),
        Some("You already have 2 searches running. Search again once one of them ends.")
    );

    // Many finished looks later, bob's look, still running, is still there to follow.
    for i in 0..SongFinder::CAPACITY + 5 {
        let id = server
            .finder
            .start("quick", &format!("user{i}"), &ct)
            .await
            .expect("a look")
            .id;
        server.finder.wait(&id, Duration::from_secs(10)).await;
    }

    assert_eq!(
        server.finder.get(&bobs.id, "bob").expect("bob's look").state,
        find_states::SEARCHING
    );
    release.send(true).expect("released");
}

#[tokio::test]
async fn a_library_songs_look_is_only_for_the_people_who_may_replace_it() {
    let server = build(owned(), None);
    let ct = CancellationToken::new();

    let carol = server.finder.start("nd-1", "carol", &ct).await.expect("a look");

    assert_eq!(carol.state, find_states::FAILED);
    assert_eq!(
        carol.error.as_deref(),
        Some("carol is not on the library actions allowed list.")
    );
    assert_eq!(
        server
            .finder
            .start("nd-1", "alice", &ct)
            .await
            .expect("a look")
            .state,
        find_states::SEARCHING
    );
}
