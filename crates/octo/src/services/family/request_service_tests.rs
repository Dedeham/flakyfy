//! Requests from making to done: met without a download where they can be, waiting for an
//! approver where they must, fetched into the family pool, and done only once the library
//! index finds the song in the member's own library.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use octo_core::family::abilities::{Preset, RequestQuality};
use octo_core::family::request::{RequestKind, RequestOutcome, RequestState};
use octo_core::models::domain::{Album, Song};
use parking_lot::Mutex;
use serde_json::json;
use wiremock::MockServer;

use super::*;
use crate::services::family::personal_library::PersonalLibrary;
use crate::services::family::pool::FamilyPlacement;
use crate::services::family::request_store::RequestStore;
use crate::services::family::test_support::{
    World, eventually, member, navidrome_with_songs, song_row, write_file,
};
use crate::services::library::OwnedCopy;

/// Outside songs are `ext-test-song-<n>`, albums `ext-test-album-<n>`; each song's title is
/// "Song <n>" by "Artist".
#[derive(Default)]
struct Catalog {
    shared: Mutex<HashSet<String>>,
    lossy_shared: Mutex<HashSet<String>>,
    /// How long a song lookup takes.
    delay: Mutex<Duration>,
}

#[async_trait]
impl RequestCatalog for Catalog {
    fn outside(&self, id: &str) -> Option<OutsideTarget> {
        if let Some(n) = id.strip_prefix("ext-test-song-") {
            return Some(OutsideTarget {
                kind: RequestKind::Song,
                provider: "test".into(),
                external_id: n.into(),
            });
        }
        id.strip_prefix("ext-test-album-").map(|_| OutsideTarget {
            kind: RequestKind::Album,
            provider: "test".into(),
            external_id: id.into(),
        })
    }

    async fn song(&self, _: &str, external_id: &str) -> Option<Song> {
        let delay = *self.delay.lock();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        Some(Song {
            title: format!("Song {external_id}"),
            artist: "Artist".into(),
            album: "Album".into(),
            external_provider: Some("test".into()),
            external_id: Some(external_id.into()),
            ..Default::default()
        })
    }

    async fn album(&self, _: &str, external_id: &str) -> Option<Album> {
        Some(Album {
            title: format!("Album {external_id}"),
            artist: "Artist".into(),
            songs: vec![self.song("test", "a1").await?, self.song("test", "a2").await?],
            ..Default::default()
        })
    }

    async fn shared_copy(&self, song: &Song) -> Option<OwnedCopy> {
        let lossless = self.shared.lock().contains(&song.title);
        let lossy = self.lossy_shared.lock().contains(&song.title);
        (lossless || lossy).then(|| OwnedCopy {
            navidrome_id: Some(format!("shared-{}", song.title)),
            absolute_path: format!("/music/{}.flac", song.title),
            suffix: if lossless { "flac" } else { "mp3" }.into(),
            bit_rate: if lossless { 900 } else { 320 },
            lossless,
        })
    }
}

/// Downloads that land at once in the family pool, like the worker would place them.
struct Fetcher {
    pool: Arc<FamilyPool>,
    in_flight: Mutex<HashSet<String>>,
    no_flac: Mutex<bool>,
    asked: Mutex<Vec<(String, String, RequestQuality)>>,
    upgrades: Mutex<Vec<(String, String)>>,
    imports: Mutex<Vec<(String, String)>>,
}

impl Fetcher {
    fn new(pool: Arc<FamilyPool>) -> Self {
        Fetcher {
            pool,
            in_flight: Mutex::new(HashSet::new()),
            no_flac: Mutex::new(false),
            asked: Mutex::new(Vec::new()),
            upgrades: Mutex::new(Vec::new()),
            imports: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl RequestFetcher for Fetcher {
    fn in_flight(&self, provider: &str, external_id: &str) -> bool {
        self.in_flight
            .lock()
            .contains(&format!("{provider}:{external_id}"))
    }

    async fn song(
        &self,
        user: &str,
        provider: &str,
        external_id: &str,
        quality: RequestQuality,
        song: Option<&Song>,
    ) -> Fetched {
        self.asked
            .lock()
            .push((user.to_string(), external_id.to_string(), quality));
        if quality == RequestQuality::Flac && *self.no_flac.lock() {
            return Fetched::Failed("No Soulseek FLAC found".into());
        }
        let extension = if quality == RequestQuality::Mp3 {
            "mp3"
        } else {
            "flac"
        };
        let path = format!(
            "{}/Artist/Album/{external_id}.{extension}",
            self.pool.pool_root_path()
        );
        write_file(&path, 1_000);
        self.pool
            .placed(
                &path,
                song.expect("a song"),
                provider,
                external_id,
                &[user.to_string()],
            )
            .await;
        Fetched::Landed(path)
    }

    async fn album(&self, user: &str, provider: &str, _: &str) -> Vec<String> {
        let mut landed = Vec::new();
        for n in ["a1", "a2"] {
            let song = Song {
                title: format!("Song {n}"),
                artist: "Artist".into(),
                ..Default::default()
            };
            if let Fetched::Landed(path) = self
                .song(user, provider, n, RequestQuality::Best, Some(&song))
                .await
            {
                landed.push(path);
            }
        }
        landed
    }

    fn upgrade(&self, owner: &str, navidrome_id: &str, _: &RequestFacts) -> bool {
        self.upgrades
            .lock()
            .push((owner.to_string(), navidrome_id.to_string()));
        true
    }

    fn upgrade_result(&self, navidrome_id: &str) -> Option<Result<(), String>> {
        self.upgrades
            .lock()
            .iter()
            .any(|(_, id)| id == navidrome_id)
            .then_some(Ok(()))
    }

    async fn import(&self, user: &str, list_id: &str) -> Result<String, String> {
        self.imports.lock().push((user.to_string(), list_id.to_string()));
        Ok("Queued.".into())
    }
}

struct Setup {
    world: World,
    catalog: Arc<Catalog>,
    fetcher: Arc<Fetcher>,
    service: Arc<RequestService>,
    events: tokio::sync::broadcast::Receiver<FamilyEvent>,
}

fn setup(world: World) -> Setup {
    world.family.upsert_member(member("kim", Preset::Kid, 5));
    let catalog = Arc::new(Catalog::default());
    let fetcher = Arc::new(Fetcher::new(world.pool.clone()));
    let events = Arc::new(FamilyEvents::new());
    let receiver = events.subscribe();
    let personal = Arc::new(PersonalLibrary::new(
        world.settings.clone(),
        world.family.clone(),
        world.index.clone(),
        world.pool.clone(),
    ));
    let service = RequestService::with_timing(
        RequestParts {
            settings: world.settings.clone(),
            family: world.family.clone(),
            requests: Arc::new(RequestStore::in_memory()),
            personal,
            catalog: catalog.clone(),
            fetcher: fetcher.clone(),
            stars: world.stars.clone(),
            events,
            playlists: world.playlists.clone(),
        },
        RequestTiming {
            poll: Duration::from_millis(10),
            upgrade_wait: Duration::from_millis(500),
        },
    );
    Setup {
        world,
        catalog,
        fetcher,
        service,
        events: receiver,
    }
}

/// A Navidrome whose index knows `songs` in Alex's (3) or Sam's (4) library, by path inside it.
async fn navidrome_knowing(world: &World, songs: &[(&str, i64, &str)]) -> MockServer {
    let members = world.members();
    let rows: Vec<_> = songs
        .iter()
        .map(|(id, library, relative)| {
            let user = if *library == 3 { "alex" } else { "sam" };
            song_row(id, *library, &format!("{members}/{user}"), relative, 1_000)
        })
        .collect();
    navidrome_with_songs(json!(rows)).await
}

fn state_of(setup: &Setup, id: &str) -> RequestState {
    setup.service.get(id).expect("kept").state
}

#[tokio::test]
async fn already_shared_is_done_at_once() {
    let setup = setup(World::new(None));
    setup.catalog.shared.lock().insert("Song 1".into());
    let made = setup
        .service
        .create("sam", "ext-test-song-1", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(made.state, RequestState::Done);
    assert_eq!(made.outcome, Some(RequestOutcome::AlreadyShared));
    assert_eq!(made.library_song_id.as_deref(), Some("shared-Song 1"));
    assert_eq!(setup.service.quota_used("sam"), 0, "no download, no slot");
    assert!(setup.fetcher.asked.lock().is_empty());
    assert!(
        setup
            .world
            .stars
            .asked
            .lock()
            .contains(&("sam".into(), "shared-Song 1".into()))
    );
}

#[tokio::test]
async fn a_song_another_member_has_is_added_at_once() {
    let mut setup = setup(World::new(None));
    let pooled = setup.world.pool_file("Artist/Album/7.flac", 10);
    let song = setup.catalog.song("test", "7").await.expect("a song");
    setup
        .world
        .pool
        .placed(&pooled, &song, "test", "7", &["sam".to_string()])
        .await;

    let made = setup
        .service
        .create("alex", "ext-test-song-7", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(made.state, RequestState::Done);
    assert_eq!(made.outcome, Some(RequestOutcome::AddedFromFamily));
    let link = format!("{}/alex/Artist/Album/7.flac", setup.world.members());
    assert!(std::fs::symlink_metadata(&link).is_ok());
    assert!(setup.fetcher.asked.lock().is_empty(), "no download");
    assert_eq!(setup.service.quota_used("alex"), 0);
    assert!(matches!(setup.events.try_recv(), Ok(FamilyEvent::RequestDone(_))));

    // A kid's request for the same song still waits: approval is about what they hear.
    let kid = setup
        .service
        .create("kim", "ext-test-song-7", RequestQuality::Mp3, None)
        .await
        .expect("made");
    assert_eq!(kid.state, RequestState::Pending);
}

#[tokio::test]
async fn auto_approve_enqueues_at_once() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(&world, &[("alex-2", 3, "Artist/Album/2.flac")]).await;
    world.point_at(&navidrome.uri());
    let setup = setup(world);
    let made = setup
        .service
        .create("alex", "ext-test-song-2", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(made.state, RequestState::Approved);
    assert!(
        eventually(|| state_of(&setup, &made.id) == RequestState::Done).await,
        "{:?}",
        setup.service.get(&made.id)
    );
    let done = setup.service.get(&made.id).expect("kept");
    assert_eq!(done.outcome, Some(RequestOutcome::Downloaded));
    assert_eq!(done.library_song_id.as_deref(), Some("alex-2"));
    assert_eq!(
        setup.fetcher.asked.lock()[0],
        ("alex".into(), "2".into(), RequestQuality::Best)
    );
    assert_eq!(setup.service.quota_used("alex"), 1);
}

#[tokio::test]
async fn done_only_after_index_sees_it() {
    let world = World::new(None);
    // Navidrome has not scanned the song yet.
    let navidrome = navidrome_knowing(&world, &[]).await;
    world.point_at(&navidrome.uri());
    let setup = setup(world);
    let made = setup
        .service
        .create("alex", "ext-test-song-3", RequestQuality::Best, None)
        .await
        .expect("made");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        state_of(&setup, &made.id),
        RequestState::Approved,
        "not done before it shows"
    );

    // The scan finds it.
    let rows = json!([song_row(
        "alex-3",
        3,
        &format!("{}/alex", setup.world.members()),
        "Artist/Album/3.flac",
        1_000
    )]);
    let found = navidrome_with_songs(rows).await;
    setup.world.point_at(&found.uri());
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Done).await);
    assert_eq!(
        setup
            .service
            .get(&made.id)
            .expect("kept")
            .library_song_id
            .as_deref(),
        Some("alex-3")
    );
    assert!(setup.world.pool.links_seen());
}

#[tokio::test]
async fn a_song_that_never_shows_fails_after_its_own_copy_was_tried() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(&world, &[]).await;
    world.point_at(&navidrome.uri());
    let setup = setup(world);
    let made = setup
        .service
        .create("alex", "ext-test-song-4", RequestQuality::Best, None)
        .await
        .expect("made");
    let failed = eventually_long(|| state_of(&setup, &made.id) == RequestState::Failed).await;
    assert!(failed, "{:?}", setup.service.get(&made.id));
    let request = setup.service.get(&made.id).expect("kept");
    assert_eq!(request.failure.as_deref(), Some(NOT_SHOWN_MESSAGE));
    // This one link became a copy; nobody else's links change for a scan that was only slow.
    let link = format!("{}/alex/Artist/Album/4.flac", setup.world.members());
    assert!(
        !std::fs::symlink_metadata(&link)
            .expect("there")
            .file_type()
            .is_symlink()
    );
    assert_eq!(setup.world.pool.mode(), LinkMode::Symlink);
}

#[tokio::test]
async fn a_copy_navidrome_shows_where_it_never_showed_the_link_is_done() {
    let world = World::new(None);
    // Navidrome lists the song at Alex's place, but only once it is a real file there.
    let members = world.members();
    let rows = json!([song_row(
        "alex-30",
        3,
        &format!("{members}/alex"),
        "Artist/Album/30.flac",
        1_000
    )]);
    let shows_files = navidrome_with_songs(rows).await;
    let shows_nothing = navidrome_knowing(&world, &[]).await;
    world.point_at(&shows_nothing.uri());
    let setup = setup(world);
    let link = format!("{members}/alex/Artist/Album/30.flac");
    let watcher = {
        let (settings, link) = (setup.world.settings.clone(), link.clone());
        let url = shows_files.uri();
        tokio::spawn(async move {
            loop {
                let is_file = std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_file());
                if is_file {
                    let mut now = settings.current().as_ref().clone();
                    now.subsonic.url = Some(url);
                    settings.set(now);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
    };
    let made = setup
        .service
        .create("alex", "ext-test-song-30", RequestQuality::Best, None)
        .await
        .expect("made");
    assert!(eventually_long(|| state_of(&setup, &made.id) == RequestState::Done).await);
    watcher.await.expect("joins");
    assert_eq!(setup.world.pool.store().link_state().unshown, 1);
    assert_eq!(
        setup.world.pool.mode(),
        LinkMode::Symlink,
        "once is not enough to give up on links"
    );
}

#[tokio::test]
async fn approved_requests_resume_after_a_restart() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(&world, &[("alex-31", 3, "Artist/Album/31.flac")]).await;
    world.point_at(&navidrome.uri());
    let setup = setup(world);
    // Approved before the restart; nothing runs it now.
    let mut left = setup.service.new_request(
        &setup.world.family.member("alex").expect("alex"),
        RequestKind::Song,
        "ext-test-song-31",
        RequestQuality::Best,
        RequestFacts::default(),
    );
    left.state = RequestState::Approved;
    let left = setup.service.store().create(left, chrono::Utc::now());
    assert_eq!(setup.service.resume_approved(), 1);
    assert!(eventually(|| state_of(&setup, &left.id) == RequestState::Done).await);
    assert_eq!(setup.service.resume_approved(), 0, "once per start");
    // Asking for the same song again is a new ask now, not stuck behind the old one.
    assert_eq!(setup.fetcher.asked.lock().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_approvers_at_once_start_a_request_once() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(&world, &[]).await;
    world.point_at(&navidrome.uri());
    let mut co = member("cy", Preset::CoAdmin, 7);
    co.edits.insert("ApproveRequests".into(), json!(true));
    world.family.upsert_member(co);
    let mut unlimited = member("sam", Preset::Listener, 4);
    unlimited.edits.insert("WeeklyRequestLimit".into(), json!(0));
    world.family.upsert_member(unlimited);
    let mut setup = setup(world);
    for n in 0..20 {
        let made = setup
            .service
            .create("sam", &format!("ext-test-song-4{n}0"), RequestQuality::Best, None)
            .await
            .expect("made");
        assert_eq!(made.state, RequestState::Pending);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let approvers: Vec<_> = ["winters", "cy"]
            .into_iter()
            .map(|by| {
                let (service, barrier, id) = (setup.service.clone(), barrier.clone(), made.id.clone());
                tokio::task::spawn_blocking(move || {
                    barrier.wait();
                    service.approve(&id, by, None)
                })
            })
            .collect();
        let mut approved = 0;
        for approver in approvers {
            match approver.await.expect("joins") {
                Ok(_) => approved += 1,
                Err(e) => assert_eq!(e.message, "This request was already decided."),
            }
        }
        assert_eq!(approved, 1, "exactly one approver decides");
    }
    let mut decided = 0;
    while let Ok(event) = setup.events.try_recv() {
        if matches!(event, FamilyEvent::RequestDecided(_)) {
            decided += 1;
        }
    }
    assert_eq!(decided, 20, "each approval is told once");
}

#[tokio::test]
async fn the_same_ask_twice_at_once_makes_one_request() {
    let mut setup = setup(World::new(None));
    *setup.catalog.delay.lock() = Duration::from_millis(50);
    let (first, second) = tokio::join!(
        setup
            .service
            .create("sam", "ext-test-song-32", RequestQuality::Best, None),
        setup
            .service
            .create("sam", "ext-test-song-32", RequestQuality::Best, None),
    );
    let (first, second) = (first.expect("made"), second.expect("made"));
    assert_eq!(first.id, second.id);
    assert_eq!(setup.service.requests_of("sam").len(), 1);
    let mut created = 0;
    while let Ok(event) = setup.events.try_recv() {
        if matches!(event, FamilyEvent::RequestCreated(_)) {
            created += 1;
        }
    }
    assert_eq!(created, 1, "the approvers hear of it once");
}

#[tokio::test]
async fn a_failed_flac_asked_again_as_mp3_takes_its_slot_and_its_approval() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(&world, &[("sam-33", 4, "Artist/Album/33.mp3")]).await;
    world.point_at(&navidrome.uri());
    let setup = setup(world);
    *setup.fetcher.no_flac.lock() = true;
    let made = setup
        .service
        .create("sam", "ext-test-song-33", RequestQuality::Flac, None)
        .await
        .expect("made");
    assert_eq!(made.state, RequestState::Pending, "a listener's request waits");
    setup
        .service
        .approve(&made.id, "winters", None)
        .expect("approved");
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Failed).await);

    let again = setup
        .service
        .retry_as_mp3(&made.id, "sam")
        .await
        .expect("asked again");
    assert_eq!(
        again.state,
        RequestState::Approved,
        "no second wait for an approver"
    );
    assert_eq!(setup.service.quota_used("sam"), 1, "one slot, as the FLAC held");
    assert!(eventually(|| state_of(&setup, &again.id) == RequestState::Done).await);
}

#[tokio::test]
async fn a_failed_flac_asked_again_waits_when_its_slot_was_taken() {
    let world = World::new(None);
    let mut small = member("sam", Preset::Listener, 4);
    small.edits.insert("WeeklyRequestLimit".into(), json!(1));
    world.family.upsert_member(small);
    let setup = setup(world);
    *setup.fetcher.no_flac.lock() = true;
    let made = setup
        .service
        .create("sam", "ext-test-song-34", RequestQuality::Flac, None)
        .await
        .expect("made");
    setup
        .service
        .approve(&made.id, "winters", None)
        .expect("approved");
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Failed).await);
    // The freed slot went to another ask meanwhile.
    setup
        .service
        .create("sam", "ext-test-song-35", RequestQuality::Best, None)
        .await
        .expect("made");
    let refused = setup
        .service
        .retry_as_mp3(&made.id, "sam")
        .await
        .expect_err("no slot left");
    assert_eq!(refused.message, "You have used all 1 of your requests this week.");
}

#[tokio::test]
async fn a_member_never_reads_a_path_inside_the_server() {
    let setup = setup(World::new(None));
    let pooled = setup.world.pool_file("Artist/Album/36.flac", 10);
    let song = setup.catalog.song("test", "36").await.expect("a song");
    setup
        .world
        .pool
        .placed(&pooled, &song, "test", "36", &["sam".to_string()])
        .await;
    // Alex's own folder cannot be made: a file is in the way.
    std::fs::write(format!("{}/alex", setup.world.members()), b"in the way").expect("a file");
    let refused = setup
        .service
        .create("alex", "ext-test-song-36", RequestQuality::Best, None)
        .await
        .expect_err("could not be linked");
    assert!(!refused.message.contains('/'), "{}", refused.message);
    assert!(!refused.message.contains(".pool"), "{}", refused.message);
}

#[tokio::test]
async fn a_song_shared_where_the_member_cannot_look_is_not_theirs_already() {
    let world = World::new(None);
    let navidrome = navidrome_with_songs(json!([song_row(
        "shared-Song 37",
        1,
        &world.music(),
        "Artist/Album/37.flac",
        1_000
    )]))
    .await;
    world.point_at(&navidrome.uri());
    world.index.refresh().await.expect("read");
    let mut sam = member("sam", Preset::Listener, 4);
    sam.shared_libraries = crate::services::family::SharedLibraries::Ids(vec![2]);
    world.family.upsert_member(sam);
    let setup = setup(world);
    setup.catalog.shared.lock().insert("Song 37".into());

    let sams = setup
        .service
        .create("sam", "ext-test-song-37", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(sams.state, RequestState::Pending, "Sam cannot see that library");
    assert_eq!(sams.outcome, None);
    let alexs = setup
        .service
        .create("alex", "ext-test-song-37", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(alexs.outcome, Some(RequestOutcome::AlreadyShared));
}

#[tokio::test]
async fn flac_request_that_finds_no_real_flac_fails_and_can_be_asked_again_as_mp3() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(&world, &[("alex-5", 3, "Artist/Album/5.mp3")]).await;
    world.point_at(&navidrome.uri());
    let setup = setup(world);
    *setup.fetcher.no_flac.lock() = true;
    let made = setup
        .service
        .create("alex", "ext-test-song-5", RequestQuality::Flac, None)
        .await
        .expect("made");
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Failed).await);
    assert_eq!(
        setup.service.get(&made.id).expect("kept").failure.as_deref(),
        Some(NO_REAL_FLAC)
    );
    assert_eq!(setup.service.quota_used("alex"), 0, "a failure frees its slot");

    let again = setup
        .service
        .retry_as_mp3(&made.id, "alex")
        .await
        .expect("asked again");
    assert_eq!(again.quality, RequestQuality::Mp3);
    assert!(eventually(|| state_of(&setup, &again.id) == RequestState::Done).await);
    assert!(setup.service.retry_as_mp3(&again.id, "alex").await.is_err());
}

#[tokio::test]
async fn a_flac_ask_for_a_lossy_shared_song_asks_for_a_better_copy() {
    let setup = setup(World::new(None));
    setup.catalog.lossy_shared.lock().insert("Song 6".into());
    let made = setup
        .service
        .create("sam", "ext-test-song-6", RequestQuality::Flac, None)
        .await
        .expect("made");
    assert_eq!(made.kind, RequestKind::Upgrade);
    assert_eq!(made.target, "shared-Song 6");
    assert_eq!(made.state, RequestState::Pending);
    setup
        .service
        .approve(&made.id, "winters", None)
        .expect("approved");
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Done).await);
    assert_eq!(
        setup.fetcher.upgrades.lock().as_slice(),
        [("winters".to_string(), "shared-Song 6".to_string())],
        "the better copy is asked for as the owner"
    );
}

#[tokio::test]
async fn listener_requests_wait_and_decline_frees_quota() {
    let mut setup = setup(World::new(None));
    let made = setup
        .service
        .create("sam", "ext-test-song-8", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(made.state, RequestState::Pending);
    assert!(matches!(
        setup.events.try_recv(),
        Ok(FamilyEvent::RequestCreated(_))
    ));
    assert_eq!(setup.service.quota_used("sam"), 1);
    // Asking again answers the same request.
    let again = setup
        .service
        .create("sam", "ext-test-song-8", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(again.id, made.id);

    assert!(
        setup.service.approve(&made.id, "sam", None).is_err(),
        "not an approver"
    );
    let declined = setup
        .service
        .decline(&made.id, "winters", Some("Not this one"))
        .expect("declined");
    assert_eq!(declined.state, RequestState::Declined);
    assert_eq!(declined.note, "Not this one");
    assert!(matches!(
        setup.events.try_recv(),
        Ok(FamilyEvent::RequestDecided(_))
    ));
    assert_eq!(setup.service.quota_used("sam"), 0);
    assert!(setup.fetcher.asked.lock().is_empty());

    let other = setup
        .service
        .create("sam", "ext-test-song-9", RequestQuality::Best, None)
        .await
        .expect("made");
    assert!(setup.service.cancel(&other.id, "alex").is_err(), "only their own");
    assert_eq!(
        setup.service.cancel(&other.id, "sam").expect("canceled").state,
        RequestState::Cancelled
    );
}

#[tokio::test]
async fn approver_can_override_limit() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(&world, &[("alex-10", 3, "Artist/Album/10.flac")]).await;
    world.point_at(&navidrome.uri());
    let mut small = member("alex", Preset::Member, 3);
    small.edits.insert("StorageLimitGb".into(), json!(1));
    world.family.upsert_member(small);
    world.index.note_placed(
        3,
        &format!("{}/alex/Big/Album/x.flac", world.members()),
        2_000_000_000,
    );
    let setup = setup(world);

    let made = setup
        .service
        .create("alex", "ext-test-song-10", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(made.state, RequestState::Pending, "a full library waits");
    assert_eq!(made.note, "Your library is full (2.0 of 1 GB)");
    let approved = setup
        .service
        .approve(&made.id, "winters", None)
        .expect("approved");
    assert_eq!(approved.decided_by.as_deref(), Some("winters"));
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Done).await);
}

#[tokio::test]
async fn album_request_is_one_request() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(
        &world,
        &[
            ("alex-a1", 3, "Artist/Album/a1.flac"),
            ("alex-a2", 3, "Artist/Album/a2.flac"),
        ],
    )
    .await;
    world.point_at(&navidrome.uri());
    let setup = setup(world);
    let made = setup
        .service
        .create("alex", "ext-test-album-1", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(made.kind, RequestKind::Album);
    assert_eq!(setup.service.requests_of("alex").len(), 1);
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Done).await);
    assert_eq!(setup.fetcher.asked.lock().len(), 2, "both songs");
}

#[tokio::test]
async fn joining_a_download_in_flight_adds_from_family() {
    let world = World::new(None);
    let navidrome = navidrome_knowing(&world, &[("alex-11", 3, "Artist/Album/11.flac")]).await;
    world.point_at(&navidrome.uri());
    let setup = setup(world);
    setup.fetcher.in_flight.lock().insert("test:11".into());
    let made = setup
        .service
        .create("alex", "ext-test-song-11", RequestQuality::Best, None)
        .await
        .expect("made");
    assert_eq!(made.state, RequestState::Approved);
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Done).await);
    assert_eq!(
        setup.service.get(&made.id).expect("kept").outcome,
        Some(RequestOutcome::AddedFromFamily)
    );
    assert_eq!(setup.service.quota_used("alex"), 0);
}

#[tokio::test]
async fn import_lists_are_one_request_handed_to_the_trickle() {
    let setup = setup(World::new(None));
    let facts = RequestFacts {
        title: "Road trip".into(),
        ..Default::default()
    };
    let made = setup
        .service
        .create_import("sam", "list-1", facts.clone())
        .expect("made");
    assert_eq!(
        (made.kind, made.state),
        (RequestKind::Import, RequestState::Pending)
    );
    assert_eq!(made.title, "Road trip");
    assert_eq!(
        setup
            .service
            .create_import("sam", "list-1", facts)
            .expect("same")
            .id,
        made.id
    );
    setup
        .service
        .approve(&made.id, "winters", None)
        .expect("approved");
    assert!(eventually(|| state_of(&setup, &made.id) == RequestState::Done).await);
    assert_eq!(
        setup.fetcher.imports.lock().as_slice(),
        [("sam".to_string(), "list-1".to_string())]
    );
}

#[tokio::test]
async fn refusals_say_why() {
    let setup = setup(World::new(None));
    let kid_flac = setup
        .service
        .create("kim", "ext-test-song-12", RequestQuality::Flac, None)
        .await
        .expect_err("refused");
    assert_eq!(kid_flac.code, 50);
    assert_eq!(kid_flac.message, "This account can ask for MP3 copies at most.");

    let mut save_only = member("pat", Preset::Listener, 6);
    save_only.edits.insert("AddToLibrary".into(), json!("SaveOnly"));
    setup.world.family.upsert_member(save_only);
    let pat = setup
        .service
        .create("pat", "ext-test-song-12", RequestQuality::Best, None)
        .await
        .expect_err("refused");
    assert_eq!(pat.message, NO_REQUESTS_MESSAGE);

    let owner = setup
        .service
        .create("winters", "ext-test-song-12", RequestQuality::Best, None)
        .await
        .expect_err("refused");
    assert_eq!(owner.code, 50);
    let unknown = setup
        .service
        .create("sam", "not-a-song", RequestQuality::Best, None)
        .await
        .expect_err("refused");
    assert_eq!(unknown.code, 70);

    for n in 0..10 {
        setup
            .service
            .create("sam", &format!("ext-test-song-1{n}0"), RequestQuality::Best, None)
            .await
            .expect("made");
    }
    let past_limit = setup
        .service
        .create("sam", "ext-test-song-999", RequestQuality::Best, None)
        .await
        .expect_err("refused");
    assert_eq!(
        past_limit.message,
        "You have used all 10 of your requests this week."
    );
}

#[tokio::test]
async fn family_off_takes_no_requests() {
    let setup = setup(World::with_family(None, false));
    let refused = setup
        .service
        .create("sam", "ext-test-song-1", RequestQuality::Best, None)
        .await
        .expect_err("refused");
    assert_eq!(refused.code, 50);
}

/// [`eventually`] with room for the pool to wait its whole time twice.
async fn eventually_long(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    check()
}
