//! The family service over the services that do its work, through the whole pipeline: requests
//! go to the request service and are told once to the owner and the member whatever made them,
//! members' storage and own library are read from their personal library, a removed member's
//! files go through the family pool, the link probe reaches both pages, live streams come from
//! the stream tracker, the unmarked count from the library index, and family playlists follow
//! one set of rules. Navidrome is a fake that walks its libraries' folders on disk; pushes go
//! to a mock ntfy server.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use octo_core::family::abilities::Preset;
use octo_core::family::device::{Device, DeviceKind, NetworkPlace};
use octo_core::family::request::{RequestKind, RequestState};
use octo_core::models::domain::Song;
use octo_core::settings::{AppSettings, FamilySettings, NotificationSettings, SubsonicSettings};
use serde_json::{Map, Value, json};
use tower::ServiceExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::app::{AppState, TestServices};
use crate::controllers::subsonic::route_tests::TestMetadata;
use crate::http::pipeline::{App, app_routes, build_with};
use crate::http::static_files::{StaticAssets, StaticRoots};
use crate::services::admin::SessionKind;
use crate::services::family::fake_navidrome::{FakeNavidrome, FakeNavidromeState};
use crate::services::family::stream_tracker::{StreamPurpose, StreamStart};
use crate::services::family::test_support::{eventually, write_file};
use crate::services::family::{Member, SharedLibraries};

const HOME: &str = "192.168.1.20";
const OUTSIDE: &str = "ext-deezer-song-77";
const OUTSIDE_TOO: &str = "ext-deezer-song-78";

/// The app password of each member's Subsonic app.
fn app_password(user: &str) -> String {
    format!("{}-APPP-ASSW-ORDX", user.to_uppercase())
}

struct Wired {
    app: App,
    state: AppState,
    nd: FakeNavidrome,
    ntfy: MockServer,
    dir: tempfile::TempDir,
}

struct Reply {
    status: StatusCode,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }

    fn subsonic(&self) -> Value {
        self.json()["subsonic-response"].clone()
    }
}

fn member(state: &AppState, name: &str, preset: Preset, library: i64) -> Member {
    Member {
        username: name.into(),
        display_name: name[..1].to_uppercase() + &name[1..],
        preset,
        edits: Map::new(),
        navidrome_user_id: format!("u-{name}"),
        navidrome_password: state.family_vault.seal("hidden").expect("sealed"),
        personal_library_id: Some(library),
        shared_libraries: SharedLibraries::All,
        account_password: String::new(),
        created: Utc::now(),
        suspended: false,
        ntfy_topic: format!("{name}-topic"),
    }
}

fn outside_song(id: &str, title: &str) -> Song {
    Song {
        id: format!("ext-deezer-song-{id}"),
        title: title.into(),
        artist: "Somebody".into(),
        album: "Elsewhere".into(),
        duration: Some(180),
        is_local: false,
        external_provider: Some("deezer".into()),
        external_id: Some(id.into()),
        explicit_content_lyrics: Some(0),
        ..Default::default()
    }
}

/// The family `winters` owns: the listeners alex (library 3) and sam (library 4), and dee, a
/// member whose own library (5) is over her 1 GB limit. A shared library (1) holds one song.
async fn wired(family_on: bool) -> Wired {
    let dir = tempfile::tempdir().expect("a temp dir");
    let music = dir.path().join("music");
    let members = dir.path().join("members");
    for folder in ["alex", "sam", "dee"] {
        std::fs::create_dir_all(members.join(folder)).expect("member folder");
    }
    write_file(
        &music
            .join("Shared Artist/Shared Album/01 Track.flac")
            .to_string_lossy(),
        2000,
    );
    // Dee's library is full: one big song of her own.
    write_file(&members.join("dee/Big/Album/01 Big.flac").to_string_lossy(), 16);

    let nd = FakeNavidrome::start().await;
    for (id, name, admin) in [
        ("u-admin", "admin", true),
        ("u-winters", "winters", true),
        ("u-alex", "alex", false),
        ("u-sam", "sam", false),
        ("u-dee", "dee", false),
    ] {
        nd.add_user(id, name, admin);
    }
    nd.add_library(1, "Music", &music.to_string_lossy());
    for (id, name) in [(3, "alex"), (4, "sam"), (5, "dee")] {
        nd.add_library(
            id,
            &format!("{name}'s library"),
            &members.join(name).to_string_lossy(),
        );
    }
    nd.state
        .playlists
        .lock()
        .push(("pl-request".into(), "Request a copy".into(), "sam".into()));
    nd.state.rescan();
    // Dee's song is counted at the size Navidrome reports, past her limit.
    for row in nd.state.songs.lock().iter_mut() {
        if row["libraryId"] == 5 {
            row["size"] = json!(2_000_000_000u64);
        }
    }

    let ntfy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "x"})))
        .mount(&ntfy)
        .await;

    let closed = format!("http://{}", crate::services::test_support::refused_address());
    let settings = AppSettings {
        soulseek: octo_core::settings::SoulseekSettings {
            base_url: Some(closed.clone()),
            ..Default::default()
        },
        radio_sources: octo_core::settings::RadioSourceSettings {
            sonic_url: format!("{closed}/sonic"),
            ..Default::default()
        },
        subsonic: SubsonicSettings {
            url: Some(nd.uri()),
            admin_username: Some("admin".into()),
            admin_password: Some("pw".into()),
            auto_detect_download_path: false,
            enable_search_discovery: false,
            ..Default::default()
        },
        family: FamilySettings {
            enabled: family_on,
            members_path: members.to_string_lossy().into_owned(),
            ..Default::default()
        },
        notifications: NotificationSettings {
            ntfy_url: format!("{}/owner-topic", ntfy.uri()),
            ..Default::default()
        },
        ..Default::default()
    };
    let metadata = TestMetadata::default();
    metadata
        .songs
        .lock()
        .insert("77".into(), outside_song("77", "Outside"));
    metadata
        .songs
        .lock()
        .insert("78".into(), outside_song("78", "Outside too"));
    let state = AppState::for_tests_with(
        settings,
        TestServices {
            config_dir: Some(dir.path().to_path_buf()),
            sign_in: Some("required".into()),
            metadata: Some(Arc::new(metadata)),
            deezer_base_url: Some(closed.clone()),
            raw_settings: vec![
                ("YouTube:ShimUrl".into(), closed.clone()),
                ("Soulseek:BaseUrl".into(), closed),
                (
                    "Library:DownloadPath".into(),
                    music.to_string_lossy().into_owned(),
                ),
            ],
            ..Default::default()
        },
    );
    assert!(state.family_vault.create_key_if_missing().expect("a key"));
    let store = &state.family_store;
    store.set_owner("winters");
    store.upsert_member(member(&state, "alex", Preset::Listener, 3));
    store.upsert_member(member(&state, "sam", Preset::Listener, 4));
    let mut dee = member(&state, "dee", Preset::Member, 5);
    dee.edits.insert("StorageLimitGb".into(), json!(1));
    store.upsert_member(dee);
    for user in ["alex", "sam", "dee"] {
        store.add_device(Device {
            id: format!("d_{user}"),
            username: user.into(),
            name: format!("{user}'s phone"),
            kind: DeviceKind::SubsonicApp,
            secret: state.family_vault.seal(&app_password(user)).expect("sealed"),
            created: Utc::now(),
            last_seen: None,
            app: String::new(),
            place: NetworkPlace::Home,
            seen_apps: Vec::new(),
            navidrome_jwt: None,
            quality: Default::default(),
        });
    }
    state.library_index.refresh().await.expect("the index is read");
    let app = build_with(
        state.clone(),
        app_routes(&StaticAssets::load(&StaticRoots::from_env())),
    );
    Wired {
        app,
        state,
        nd,
        ntfy,
        dir,
    }
}

impl Wired {
    fn members(&self) -> String {
        self.dir.path().join("members").to_string_lossy().into_owned()
    }

    fn music(&self) -> String {
        self.dir.path().join("music").to_string_lossy().into_owned()
    }

    fn cookie(&self, user: &str) -> String {
        let kind = if user == "winters" {
            SessionKind::FamilyNavidrome
        } else {
            SessionKind::Member
        };
        format!(
            "octo_family={}",
            self.state.browse_sessions.create_family(user, kind)
        )
    }

    async fn send(&self, method: Method, uri: &str, cookie: Option<&str>, body: Option<Value>) -> Reply {
        let mut req = Request::builder().method(method.clone()).uri(uri);
        if let Some(cookie) = cookie {
            req = req.header("cookie", cookie);
        }
        if !matches!(method, Method::GET | Method::HEAD) {
            req = req.header("x-octo-family", "1").header("x-octo-admin", "1");
        }
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        if !body.is_empty() {
            req = req.header("content-type", "application/json");
        }
        let mut req = req.body(Body::from(body)).expect("a request");
        let address: SocketAddr = format!("{HOME}:50000").parse().expect("an address");
        req.extensions_mut().insert(ConnectInfo(address));
        let res = self.app.clone().oneshot(req).await.expect("infallible");
        let status = res.status();
        let bytes = res.into_body().collect().await.expect("a body").to_bytes();
        Reply {
            status,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    async fn page(&self, method: Method, path: &str, user: &str, body: Option<Value>) -> Reply {
        let cookie = self.cookie(user);
        self.send(method, path, Some(&cookie), body).await
    }

    /// A Subsonic call from a member's app.
    async fn rest(&self, user: &str, endpoint: &str, query: &str) -> Reply {
        let uri = format!(
            "/rest/{endpoint}?u={user}&p={}&v=1.16.1&c=test&f=json&{query}",
            app_password(user)
        );
        self.send(Method::GET, &uri, None, None).await
    }

    /// The pushes the mock ntfy server took for one topic: `(title, message)`.
    async fn pushes(&self, topic: &str) -> Vec<(String, String)> {
        self.ntfy
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .filter(|push| push["topic"] == topic)
            .map(|push| {
                (
                    push["title"].as_str().unwrap_or("").to_string(),
                    push["message"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }

    /// The owner's pushes about new requests.
    async fn asked(&self) -> Vec<String> {
        self.pushes("owner-topic")
            .await
            .into_iter()
            .map(|(title, _)| title)
            .filter(|title| title.contains(" asked for "))
            .collect()
    }

    /// Starts Family's background work, which tells people about requests, and waits until it
    /// listens.
    async fn start_family_work(&self) {
        self.state.start_family();
        self.state.workers.start();
        let events = Arc::clone(&self.state.family_events);
        assert!(
            eventually(|| events.listeners() > 0).await,
            "Family's work never listened"
        );
    }

    /// A file in the family pool at `relative`, linked into each of `users`' libraries, with
    /// the index read again.
    async fn pooled(&self, relative: &str, bytes: usize, users: &[&str]) -> String {
        let pool = &self.state.family_pool_service;
        let path = format!("{}/{relative}", pool.pool_root_path());
        write_file(&path, bytes);
        for user in users {
            pool.link(&path, user).await.expect("linked");
        }
        self.nd.state.rescan();
        self.state.library_index.refresh().await.expect("read");
        path
    }

    async fn until_quiet(&self) {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

// ------------------------------------------------------------------------------------------
// Requests and their notices
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn web_requests_go_through_the_request_service() {
    let w = wired(true).await;
    let made = w
        .page(
            Method::POST,
            "/api/family/me/requests",
            "alex",
            Some(json!({ "id": OUTSIDE, "quality": "Flac" })),
        )
        .await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.body);
    let made = made.json();
    assert_eq!(made["state"], "Pending");
    // The catalog named it: the request service looked the song up.
    assert_eq!(made["title"], "Outside");
    assert_eq!(made["artist"], "Somebody");
    let id = made["id"].as_str().expect("an id").to_string();
    let stored = w.state.family_requests.get(&id).expect("kept");
    assert_eq!(stored.kind, RequestKind::Song);

    let canceled = w
        .page(
            Method::DELETE,
            &format!("/api/family/me/requests/{id}"),
            "alex",
            None,
        )
        .await;
    assert_eq!(canceled.status, StatusCode::OK, "{}", canceled.body);
    assert_eq!(canceled.json()["state"], "Cancelled");

    // A request the service refuses says why: a listener cannot ask for a song by an unknown id.
    let unknown = w
        .page(
            Method::POST,
            "/api/family/me/requests",
            "alex",
            Some(json!({ "id": "nothing-like-this" })),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.body);

    // The owner decides through the same service.
    let again = w
        .page(
            Method::POST,
            "/api/family/me/requests",
            "alex",
            Some(json!({ "id": OUTSIDE_TOO })),
        )
        .await
        .json();
    let declined = w
        .page(
            Method::POST,
            &format!(
                "/api/family/requests/{}/decline",
                again["id"].as_str().expect("id")
            ),
            "winters",
            Some(json!({ "note": "Not now" })),
        )
        .await;
    assert_eq!(declined.status, StatusCode::OK, "{}", declined.body);
    assert_eq!(declined.json()["state"], "Declined");
    assert_eq!(declined.json()["note"], "Not now");
}

#[tokio::test]
async fn a_failed_flac_request_asked_again_as_mp3_is_tried_again() {
    let w = wired(true).await;
    let failed = w.state.family_requests.create(
        octo_core::family::request::FamilyRequest {
            id: String::new(),
            username: "alex".into(),
            kind: RequestKind::Song,
            target: OUTSIDE.into(),
            title: "Outside".into(),
            artist: "Somebody".into(),
            album: "Elsewhere".into(),
            cover_art: OUTSIDE.into(),
            quality: octo_core::family::abilities::RequestQuality::Flac,
            state: RequestState::Failed,
            created: Utc::now(),
            decided_by: None,
            decided: None,
            note: String::new(),
            library_song_id: None,
            failure: Some("No real FLAC copy found".into()),
            outcome: None,
        },
        Utc::now(),
    );
    let retried = w
        .page(
            Method::POST,
            "/api/family/me/requests",
            "alex",
            Some(json!({ "id": OUTSIDE, "quality": "Mp3" })),
        )
        .await;
    assert_eq!(retried.status, StatusCode::OK, "{}", retried.body);
    let retried = retried.json();
    assert_eq!(retried["quality"], "Mp3");
    assert_eq!(retried["title"], "Outside");
    assert_ne!(retried["id"], json!(failed.id));
    assert_eq!(w.state.family_requests.for_user("alex").len(), 2);
}

#[tokio::test]
async fn the_owner_hears_each_request_once_whatever_made_it() {
    let w = wired(true).await;
    w.start_family_work().await;

    // From the Family page.
    let web = w
        .page(
            Method::POST,
            "/api/family/me/requests",
            "alex",
            Some(json!({ "id": OUTSIDE })),
        )
        .await;
    assert_eq!(web.json()["state"], "Pending", "{}", web.body);

    // From a heart: Dee adds directly, but her library is full, so her heart asks.
    let heart = w.rest("dee", "star", &format!("id={OUTSIDE_TOO}")).await;
    assert_eq!(heart.subsonic()["status"], "ok", "{}", heart.body);
    assert!(w.state.family_saved.is_saved("dee", OUTSIDE_TOO));
    let dees = w.state.family_requests.for_user("dee");
    assert_eq!(dees.len(), 1, "{dees:?}");
    assert_eq!(dees[0].state, RequestState::Pending);

    // From an action playlist.
    let playlist = w
        .rest(
            "sam",
            "updatePlaylist",
            &format!("playlistId=pl-request&songIdToAdd={OUTSIDE}"),
        )
        .await;
    assert_eq!(playlist.subsonic()["status"], "ok", "{}", playlist.body);
    assert_eq!(w.state.family_requests.for_user("sam").len(), 1);

    let mut heard = Vec::new();
    for _ in 0..100 {
        heard = w.asked().await;
        if heard.len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    w.until_quiet().await;
    let mut heard_now = w.asked().await;
    heard_now.sort();
    assert_eq!(heard_now.len(), 3, "{heard_now:?} (first {heard:?})");
    assert_eq!(
        heard_now,
        [
            "Alex asked for Outside by Somebody (best quality)",
            "Dee asked for Outside too by Somebody (best quality)",
            "Sam asked for Outside by Somebody (best quality)",
        ]
    );

    // The member hears a decision once, on their own topic.
    let alex = w.state.family_requests.for_user("alex").pop().expect("alex's");
    let declined = w
        .page(
            Method::POST,
            &format!("/api/family/requests/{}/decline", alex.id),
            "winters",
            None,
        )
        .await;
    assert_eq!(declined.status, StatusCode::OK, "{}", declined.body);
    for _ in 0..100 {
        if !w.pushes("alex-topic").await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    w.until_quiet().await;
    let told = w.pushes("alex-topic").await;
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].0.starts_with("Declined: Outside"), "{told:?}");
    // Nothing new reached the owner for the decision.
    assert_eq!(w.asked().await.len(), 3);
}

// ------------------------------------------------------------------------------------------
// Members' own libraries, folders and files
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn storage_and_my_library_come_from_the_personal_library() {
    let w = wired(true).await;
    let pooled = w.pooled("Artist/Album/01 Song.flac", 3000, &["alex"]).await;
    let me = w.page(Method::GET, "/api/family/me", "alex", None).await.json();
    assert_eq!(me["storageUsedBytes"], 3000, "{me}");

    let library = w
        .page(Method::GET, "/api/family/me/library", "alex", None)
        .await
        .json();
    assert_eq!(library["total"], 1, "{library}");
    let song = &library["songs"][0];
    assert_eq!(song["title"], "Song");
    assert_eq!(song["artist"], "Artist");
    assert_eq!(song["album"], "Album");
    assert_eq!(song["size"], 3000);
    assert!(song["added"].is_string(), "{song}");
    assert_eq!(library["usedBytes"], 3000);

    // A shared song is not theirs to remove.
    let shared = FakeNavidromeState::song_id(1, "Shared Artist/Shared Album/01 Track.flac");
    let refused = w
        .rest("alex", "removeFromMyLibrary", &format!("id={shared}"))
        .await;
    assert_eq!(refused.subsonic()["error"]["code"], 50, "{}", refused.body);

    let id = song["id"].as_str().expect("an id").to_string();
    let removed = w
        .page(
            Method::DELETE,
            &format!("/api/family/me/library/{id}"),
            "alex",
            None,
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.body);
    let link = format!("{}/alex/Artist/Album/01 Song.flac", w.members());
    assert!(std::fs::symlink_metadata(&link).is_err(), "the link went");
    assert!(
        !Path::new(&pooled).exists(),
        "nobody has it, so it went to the trash"
    );
}

#[tokio::test]
async fn adding_a_member_makes_their_folder_and_library() {
    let w = wired(true).await;
    let added = w
        .page(
            Method::POST,
            "/api/family/members",
            "winters",
            Some(json!({ "username": "rae", "displayName": "Rae", "preset": "Listener" })),
        )
        .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.body);
    let folder = format!("{}/rae", w.members());
    assert!(Path::new(&folder).is_dir());
    assert!(
        w.nd.state
            .libraries
            .lock()
            .iter()
            .any(|library| library.path == folder),
        "Navidrome got a library over the folder"
    );
}

async fn remove_alex(choice: &str) -> (Wired, String, String) {
    let w = wired(true).await;
    let only = w.pooled("Artist/Album/01 Only.flac", 100, &["alex"]).await;
    let both = w.pooled("Artist/Album/02 Both.flac", 200, &["alex", "sam"]).await;
    let removed = w
        .page(
            Method::DELETE,
            "/api/family/members/alex",
            "winters",
            Some(json!({ "confirm": "alex", "files": choice })),
        )
        .await;
    assert_eq!(
        removed.status,
        StatusCode::NO_CONTENT,
        "{choice}: {}",
        removed.body
    );
    assert!(w.state.family_store.member("alex").is_none());
    // Sam still has the song they shared, as it was.
    assert!(Path::new(&both).is_file(), "{choice}");
    let sams = format!("{}/sam/Artist/Album/02 Both.flac", w.members());
    assert!(Path::new(&sams).is_file(), "{choice}");
    let refs = w.state.family_pool.get(&both).expect("still pooled").refs;
    assert_eq!(refs, ["sam"], "{choice}");
    assert!(!Path::new(&format!("{}/alex", w.members())).exists(), "{choice}");
    (w, only, both)
}

#[tokio::test]
async fn removing_a_member_can_keep_their_files_aside() {
    let (w, only, _) = remove_alex("keep").await;
    assert!(!Path::new(&only).exists());
    assert!(w.state.family_pool.get(&only).is_none());
    let kept = format!("{}/.kept/alex/Artist/Album/01 Only.flac", w.members());
    assert!(Path::new(&kept).is_file(), "kept aside at {kept}");
}

#[tokio::test]
async fn removing_a_member_can_move_their_files_into_the_shared_library() {
    let (w, only, _) = remove_alex("promote").await;
    assert!(!Path::new(&only).exists());
    let shared = format!("{}/Artist/Album/01 Only.flac", w.music());
    assert!(Path::new(&shared).is_file(), "moved to {shared}");
}

#[tokio::test]
async fn removing_a_member_can_send_their_files_to_the_trash() {
    let (w, only, _) = remove_alex("delete").await;
    assert!(!Path::new(&only).exists());
    assert!(w.state.family_pool.get(&only).is_none());
    assert!(!Path::new(&format!("{}/.kept", w.members())).exists());
    assert!(!Path::new(&format!("{}/Artist/Album/01 Only.flac", w.music())).exists());
}

#[tokio::test]
async fn turning_family_on_probes_links_and_both_pages_show_them() {
    let w = wired(false).await;
    let admin = format!("octo_browse={}", w.state.browse_sessions.create("winters"));
    let before = w.send(Method::GET, "/api/admin/family", Some(&admin), None).await;
    assert_eq!(before.json()["links"], Value::Null, "{}", before.body);
    let on = w
        .send(
            Method::POST,
            "/api/admin/family/enable",
            Some(&admin),
            Some(json!({ "enabled": true })),
        )
        .await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(on.json()["links"], "Symlink");
    let session = w
        .page(Method::GET, "/api/family/session", "winters", None)
        .await
        .json();
    assert_eq!(session["links"], "Symlink", "{session}");
    assert_eq!(session["linksMessage"], Value::Null);
    let summary = w
        .send(Method::GET, "/api/admin/family", Some(&admin), None)
        .await
        .json();
    assert_eq!(summary["links"], "Symlink", "{summary}");

    // Navidrome never showing a linked song switches the pool to copies; both pages say so.
    w.state.family_pool_service.fall_back_to_copies();
    let session = w
        .page(Method::GET, "/api/family/session", "winters", None)
        .await
        .json();
    assert_eq!(session["links"], "Copy", "{session}");
    let message = session["linksMessage"].as_str().expect("a message");
    assert!(message.starts_with("This disk cannot link files"), "{message}");
    assert!(message.contains("--vfs-links"), "{message}");
    let summary = w
        .send(Method::GET, "/api/admin/family", Some(&admin), None)
        .await
        .json();
    assert_eq!(summary["links"], "Copy");
    assert_eq!(summary["linksMessage"], session["linksMessage"]);
}

// ------------------------------------------------------------------------------------------
// Live streams, the unmarked count and family playlists
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn live_streams_come_from_the_stream_tracker() {
    let w = wired(true).await;
    let song = FakeNavidromeState::song_id(1, "Shared Artist/Shared Album/01 Track.flac");
    let body = w.state.family_audio.tracker().track(
        StreamStart {
            user: "alex".into(),
            device_id: Some("d_alex".into()),
            device_name: "alex's phone".into(),
            song_id: song.clone(),
            place: NetworkPlace::Away,
            purpose: StreamPurpose::Play,
            devices_at_once: 0,
            pace_bytes_per_second: None,
        },
        Body::from(vec![1u8; 2048]),
    );
    let mut body = body;
    // The stream counts from its first read.
    let first = body.frame().await.expect("a frame").expect("bytes");
    assert!(first.is_data());

    let load = w
        .page(Method::GET, "/api/family/load", "winters", None)
        .await
        .json();
    assert_eq!(load["streams"][0]["username"], "alex", "{load}");
    assert_eq!(load["streams"][0]["device"], "alex's phone");
    assert_eq!(load["streams"][0]["songTitle"], "Track");
    assert_eq!(load["streams"][0]["place"], "Away");
    let members = w
        .page(Method::GET, "/api/family/members", "winters", None)
        .await
        .json();
    let alex = members
        .as_array()
        .and_then(|list| list.iter().find(|m| m["username"] == "alex"))
        .expect("alex")
        .clone();
    assert_eq!(alex["playingNow"], true, "{alex}");
    let devices = w.rest("alex", "getFamilyDevices", "").await.subsonic();
    let phone = devices["familyDevices"]["device"]
        .as_array()
        .and_then(|list| list.iter().find(|d| d["id"] == "d_alex"))
        .expect("the phone")
        .clone();
    assert_eq!(phone["playing"]["songId"], json!(song), "{phone}");
    assert_eq!(phone["playing"]["title"], "Track");
    assert_eq!(phone["playing"]["artist"], "Shared Artist");

    drop(body);
    let load = w
        .page(Method::GET, "/api/family/load", "winters", None)
        .await
        .json();
    assert_eq!(load["streams"], json!([]), "{load}");
}

#[tokio::test]
async fn the_unmarked_count_reads_the_library_index() {
    let w = wired(true).await;
    w.pooled("Artist/Album/01 Own.flac", 10, &["alex"]).await;
    let unrated = w
        .page(Method::GET, "/api/family/unrated", "winters", None)
        .await
        .json();
    // The shared song has no mark; members' own songs are not counted.
    assert_eq!(unrated["count"], 1, "{unrated}");
}

#[tokio::test]
async fn family_playlists_follow_one_set_of_rules_everywhere() {
    let w = wired(true).await;
    let refused = w
        .page(
            Method::POST,
            "/api/family/playlists",
            "dee",
            Some(json!({ "name": "Dee's mix", "approveAdditions": true })),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let refused = w
        .rest("dee", "createFamilyPlaylist", "name=Mix&approveAdditions=true")
        .await;
    assert_eq!(refused.subsonic()["error"]["code"], 50, "{}", refused.body);

    let made = w
        .page(
            Method::POST,
            "/api/family/playlists",
            "dee",
            Some(json!({ "name": "Dee's mix" })),
        )
        .await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.body);
    let id = made.json()["id"].as_str().expect("an id").to_string();
    let path = format!("/api/family/playlists/{id}");
    let not_theirs = w
        .page(Method::PUT, &path, "sam", Some(json!({ "name": "Mine" })))
        .await;
    assert_eq!(not_theirs.status, StatusCode::FORBIDDEN, "{}", not_theirs.body);
    let no_approval = w
        .page(
            Method::PUT,
            &path,
            "dee",
            Some(json!({ "approveAdditions": true })),
        )
        .await;
    assert_eq!(no_approval.status, StatusCode::FORBIDDEN, "{}", no_approval.body);
    let renamed = w
        .page(Method::PUT, &path, "dee", Some(json!({ "name": "Dee's songs" })))
        .await;
    assert_eq!(renamed.json()["name"], "Dee's songs", "{}", renamed.body);
    let approval = w
        .page(
            Method::PUT,
            &path,
            "winters",
            Some(json!({ "approveAdditions": true })),
        )
        .await;
    assert_eq!(approval.json()["approveAdditions"], true, "{}", approval.body);
    let listed = w
        .page(Method::GET, "/api/family/playlists", "sam", None)
        .await
        .json();
    assert_eq!(listed[0]["name"], "Dee's songs", "{listed}");
    assert_eq!(listed[0]["songCount"], 0);
    let deleted = w.page(Method::DELETE, &path, "winters", None).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT, "{}", deleted.body);
}
