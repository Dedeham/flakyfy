//! The Family page's API, the dashboard's Family entry and the `octoFamily` calls, through the
//! whole pipeline. Navidrome's accounts and libraries, the members' folders and files,
//! requests, storage, the personal library, live streams and the explicit marks are fakes
//! installed as hooks; a wiremock Navidrome answers sign-ins and pings.

use crate::services::family::family_service::HANDOFF_TTL;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use chrono::{TimeDelta, Utc};
use http_body_util::BodyExt;
use octo_core::common::Clock;
use octo_core::family::abilities::{Preset, RequestQuality};
use octo_core::family::device::{Device, DeviceKind, NetworkPlace};
use octo_core::family::request::{FamilyRequest, RequestKind, RequestState};
use octo_core::settings::{AppSettings, FamilySettings, SubsonicSettings};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::app::{AppState, TestServices};
use crate::http::pipeline::{App, app_routes, build_with};
use crate::http::static_files::{StaticAssets, StaticRoots};
use crate::services::admin::SessionKind;
use crate::services::family::family_service::{
    FamilyError, FilesChoice, LinkMode, LiveStreams, LoadReport, LoadStream, MemberAccounts, MemberFiles,
    MyLibraryPage, MySong, NewRequest, PAIR_CAP, PAIR_REFUSED, PairGuard, PersonalFolders, Playing,
    RemoveFromMyLibrary, RequestService, StorageUsage, UnratedCount, valid_username,
};
use crate::services::family::navidrome_admin::{AdminError, NdLibrary, NdUser};
use crate::services::family::request_store::new_request_id;
use crate::services::family::{FamilyStore, Member, RequestStore, SharedLibraries, passwords};

const HOME: &str = "192.168.1.20";
const AWAY: &str = "203.0.113.9";
const ALEX_PASSWORD: &str = "correct horse";
const ALEX_APP: &str = "ABCD-EFGH-JKMN-PQRS";
const ALEX_SECRET: &str = "octo-app-secret-alex-000000000000";
const SAM_APP: &str = "SAMM-EFGH-JKMN-PQRS";

// ------------------------------------------------------------------------------------------
// Fakes
// ------------------------------------------------------------------------------------------

#[derive(Default)]
struct FakeAccounts {
    users: Mutex<Vec<NdUser>>,
    libraries: Mutex<Vec<NdLibrary>>,
    calls: Mutex<Vec<String>>,
    refuse_library: Mutex<Option<(u16, String)>>,
    next: AtomicI64,
}

fn nd_user(id: &str, name: &str, admin: bool) -> NdUser {
    let mut fields = Map::new();
    fields.insert("id".into(), json!(id));
    fields.insert("userName".into(), json!(name));
    fields.insert("name".into(), json!(name));
    fields.insert("isAdmin".into(), json!(admin));
    NdUser { fields }
}

impl FakeAccounts {
    fn calls(&self) -> Vec<String> {
        self.calls.lock().clone()
    }

    fn log(&self, call: String) {
        self.calls.lock().push(call);
    }
}

#[async_trait]
impl MemberAccounts for FakeAccounts {
    async fn users(&self) -> Result<Vec<NdUser>, AdminError> {
        Ok(self.users.lock().clone())
    }

    async fn create_user(&self, user_name: &str, name: &str, _password: &str) -> Result<String, AdminError> {
        let id = format!("u-{user_name}");
        self.users.lock().push(nd_user(&id, user_name, false));
        self.log(format!("create_user {user_name} {name}"));
        Ok(id)
    }

    async fn delete_user(&self, id: &str) -> Result<(), AdminError> {
        self.users.lock().retain(|u| u.id() != id);
        self.log(format!("delete_user {id}"));
        Ok(())
    }

    async fn set_password(&self, id: &str, _password: &str) -> Result<(), AdminError> {
        self.log(format!("set_password {id}"));
        Ok(())
    }

    async fn libraries(&self) -> Result<Vec<NdLibrary>, AdminError> {
        Ok(self.libraries.lock().clone())
    }

    async fn create_library(&self, name: &str, path: &str) -> Result<i64, AdminError> {
        if let Some((status, body)) = self.refuse_library.lock().clone() {
            return Err(AdminError::Status { status, body });
        }
        let id = 10 + self.next.fetch_add(1, Ordering::SeqCst);
        self.libraries.lock().push(NdLibrary {
            id,
            name: name.to_string(),
            path: path.to_string(),
        });
        self.log(format!("create_library {name} {path}"));
        Ok(id)
    }

    async fn delete_library(&self, id: i64) -> Result<(), AdminError> {
        self.libraries.lock().retain(|l| l.id != id);
        self.log(format!("delete_library {id}"));
        Ok(())
    }

    async fn set_user_libraries(&self, user_id: &str, library_ids: &[i64]) -> Result<(), AdminError> {
        self.log(format!("libraries {user_id} {library_ids:?}"));
        Ok(())
    }
}

#[derive(Default)]
struct FakeFolders {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl PersonalFolders for FakeFolders {
    async fn create(&self, username: &str) -> Result<String, String> {
        self.calls.lock().push(format!("create {username}"));
        Ok(format!("/members/{username}"))
    }

    async fn remove_if_empty(&self, username: &str) {
        self.calls.lock().push(format!("remove {username}"));
    }

    async fn probe_links(&self) -> LinkMode {
        LinkMode::Copy
    }
}

#[derive(Default)]
struct FakeFiles {
    released: Mutex<Vec<(String, FilesChoice)>>,
}

#[async_trait]
impl MemberFiles for FakeFiles {
    async fn release(&self, username: &str, choice: FilesChoice) -> Result<(), FamilyError> {
        self.released.lock().push((username.to_string(), choice));
        Ok(())
    }
}

struct FakeRequests {
    store: Arc<RequestStore>,
}

#[async_trait]
impl RequestService for FakeRequests {
    async fn create(&self, ask: NewRequest) -> Result<FamilyRequest, FamilyError> {
        if ask.target == "ext-refused" {
            return Err(FamilyError::Refused(
                "You have used all 10 of your requests this week.".into(),
            ));
        }
        Ok(self
            .store
            .create(request(&ask.username, &ask.target, ask.quality), Utc::now()))
    }

    async fn cancel(&self, id: &str, by: &str) -> Result<FamilyRequest, FamilyError> {
        let found = self
            .store
            .get(id)
            .filter(|r| r.username.eq_ignore_ascii_case(by))
            .ok_or_else(|| FamilyError::NotFound("There is no such request.".into()))?;
        if found.state != RequestState::Pending {
            return Err(FamilyError::Refused(
                "Only a waiting request can be canceled.".into(),
            ));
        }
        Ok(self
            .store
            .update(id, |r| r.state = RequestState::Cancelled)
            .expect("there"))
    }

    async fn decide(
        &self,
        id: &str,
        approve: bool,
        by: &str,
        note: &str,
    ) -> Result<FamilyRequest, FamilyError> {
        self.store
            .update(id, |r| {
                r.state = if approve {
                    RequestState::Approved
                } else {
                    RequestState::Declined
                };
                r.decided_by = Some(by.to_string());
                r.decided = Some(Utc::now());
                r.note = note.to_string();
            })
            .ok_or_else(|| FamilyError::NotFound("There is no such request.".into()))
    }

    async fn retry_as_mp3(&self, id: &str, by: &str) -> Result<FamilyRequest, FamilyError> {
        let failed = self
            .store
            .get(id)
            .filter(|r| r.username.eq_ignore_ascii_case(by))
            .ok_or_else(|| FamilyError::NotFound("There is no such request.".into()))?;
        Ok(self
            .store
            .create(request(by, &failed.target, RequestQuality::Mp3), Utc::now()))
    }
}

fn request(user: &str, target: &str, quality: RequestQuality) -> FamilyRequest {
    FamilyRequest {
        id: new_request_id(),
        username: user.to_string(),
        kind: RequestKind::Song,
        target: target.to_string(),
        title: format!("Song {target}"),
        artist: "Artist".into(),
        album: "Album".into(),
        cover_art: target.to_string(),
        quality,
        state: RequestState::Pending,
        created: Utc::now(),
        decided_by: None,
        decided: None,
        note: String::new(),
        library_song_id: None,
        failure: None,
        outcome: None,
    }
}

struct FakeStorage;

impl StorageUsage for FakeStorage {
    fn used_bytes(&self, username: &str) -> u64 {
        if username == "alex" { 2_500_000_000 } else { 0 }
    }
}

#[derive(Default)]
struct FakeLibrary {
    removed: Mutex<Vec<String>>,
}

#[async_trait]
impl RemoveFromMyLibrary for FakeLibrary {
    fn list(&self, _username: &str, offset: usize, size: usize) -> MyLibraryPage {
        let songs: Vec<MySong> = (0..3)
            .map(|n| MySong {
                id: format!("s{n}"),
                title: format!("Song {n}"),
                artist: "Artist".into(),
                album: "Album".into(),
                size: 5_000_000,
                added: None,
            })
            .skip(offset)
            .take(size)
            .collect();
        MyLibraryPage { songs, total: 3 }
    }

    async fn remove(&self, _username: &str, song_id: &str) -> Result<(), FamilyError> {
        if song_id == "shared-1" {
            return Err(FamilyError::Refused(
                "That song is in the family library, not yours.".into(),
            ));
        }
        self.removed.lock().push(song_id.to_string());
        Ok(())
    }
}

struct FakeStreams;

impl LiveStreams for FakeStreams {
    fn playing(&self, device_id: &str) -> Option<Playing> {
        (device_id == "d_app").then(|| Playing {
            song_id: "s1".into(),
            title: "Hyperballad".into(),
            artist: "Björk".into(),
        })
    }

    fn user_playing(&self, username: &str) -> bool {
        username == "alex"
    }

    fn count(&self) -> usize {
        1
    }

    fn load(&self) -> LoadReport {
        LoadReport {
            streams: vec![LoadStream {
                username: "alex".into(),
                device: "Pixel 9".into(),
                song_title: "Hyperballad".into(),
                kbps: 160,
                place: NetworkPlace::Away,
            }],
            upload_kbps: 160,
            ..Default::default()
        }
    }
}

struct FakeUnrated;

#[async_trait]
impl UnratedCount for FakeUnrated {
    fn unrated(&self) -> Option<u64> {
        Some(12)
    }

    async fn mark_now(&self, _by: &str) -> Result<String, FamilyError> {
        Ok("Octo is looking the songs up now.".into())
    }
}

// ------------------------------------------------------------------------------------------
// The family under test
// ------------------------------------------------------------------------------------------

struct Fam {
    app: App,
    state: AppState,
    nd: MockServer,
    accounts: Arc<FakeAccounts>,
    folders: Arc<FakeFolders>,
    files: Arc<FakeFiles>,
    library: Arc<FakeLibrary>,
    pending: String,
    _dir: tempfile::TempDir,
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }

    fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or_default().to_string()
    }

    /// The `subsonic-response` of a JSON Subsonic answer.
    fn subsonic(&self) -> Value {
        self.json()["subsonic-response"].clone()
    }

    /// The token a sign-in answer set in the cookie.
    fn cookie_token(&self) -> String {
        let cookie = self
            .headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("octo_family="))
            .expect("a family cookie")
            .to_string();
        cookie["octo_family=".len()..]
            .split(';')
            .next()
            .unwrap_or("")
            .to_string()
    }
}

fn device(id: &str, user: &str, kind: DeviceKind, sealed: String) -> Device {
    Device {
        id: id.into(),
        username: user.into(),
        name: "Pixel 9".into(),
        kind,
        secret: sealed,
        created: Utc::now(),
        last_seen: None,
        app: String::new(),
        place: NetworkPlace::Home,
        seen_apps: Vec::new(),
        navidrome_jwt: None,
        quality: Default::default(),
    }
}

fn member(
    state: &AppState,
    name: &str,
    preset: Preset,
    password: Option<&str>,
    library: Option<i64>,
) -> Member {
    Member {
        username: name.into(),
        display_name: name[..1].to_uppercase() + &name[1..],
        preset,
        edits: Map::new(),
        navidrome_user_id: format!("u-{name}"),
        navidrome_password: state.family_vault.seal("hidden").expect("sealed"),
        personal_library_id: library,
        shared_libraries: SharedLibraries::All,
        account_password: password.map(passwords::hash).unwrap_or_default(),
        created: Utc::now(),
        suspended: false,
        ntfy_topic: String::new(),
    }
}

async fn fam() -> Fam {
    fam_with(|_| {}, None).await
}

/// The family `winters` owns: the listener `alex` (an Octo app and an app password, a request
/// waiting) and the co-admin `sam`. `sign_in` is the dashboard's `Admin:SignIn`.
async fn fam_with(change: impl FnOnce(&mut AppSettings), sign_in: Option<&str>) -> Fam {
    fam_on(change, sign_in, None).await
}

/// [`fam_with`] with the YouTube helper at `shim` (None: an address that refuses at once).
async fn fam_on(change: impl FnOnce(&mut AppSettings), sign_in: Option<&str>, shim: Option<&str>) -> Fam {
    let nd = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/rest/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#,
            "application/json",
        ))
        .mount(&nd)
        .await;
    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .and(body_partial_json(
            json!({ "username": "winters", "password": "owner pass" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "id": "u-winters", "isAdmin": true, "token": "a.b.c", "username": "winters" }),
        ))
        .mount(&nd)
        .await;
    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .and(body_partial_json(
            json!({ "username": "bob", "password": "bob pass" }),
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({ "id": "u-bob", "isAdmin": false, "token": "d.e.f", "username": "bob" }),
            ),
        )
        .mount(&nd)
        .await;
    let dir = tempfile::tempdir().expect("a temp dir");
    // The status probes find nothing listening at once, rather than waiting on a name.
    let closed = format!("http://{}", crate::services::test_support::refused_address());
    let mut settings = AppSettings {
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
            ..Default::default()
        },
        family: FamilySettings {
            enabled: true,
            members_path: dir.path().join("members").to_string_lossy().into_owned(),
            ..Default::default()
        },
        ..Default::default()
    };
    change(&mut settings);
    let state = AppState::for_tests_with(
        settings,
        TestServices {
            config_dir: Some(dir.path().to_path_buf()),
            sign_in: sign_in.map(str::to_string),
            raw_settings: vec![
                (
                    "YouTube:ShimUrl".into(),
                    shim.map_or_else(|| closed.clone(), str::to_string),
                ),
                ("Soulseek:BaseUrl".into(), closed),
            ],
            ..Default::default()
        },
    );
    assert!(state.family_vault.create_key_if_missing().expect("a key"));
    let store = &state.family_store;
    store.set_owner("winters");
    store.upsert_member(member(
        &state,
        "alex",
        Preset::Listener,
        Some(ALEX_PASSWORD),
        Some(3),
    ));
    store.upsert_member(member(
        &state,
        "sam",
        Preset::CoAdmin,
        Some("sam password"),
        Some(4),
    ));
    let seal = |secret: &str| state.family_vault.seal(secret).expect("sealed");
    store.add_device(device("d_app", "alex", DeviceKind::OctoApp, seal(ALEX_SECRET)));
    store.add_device(device("d_sub", "alex", DeviceKind::SubsonicApp, seal(ALEX_APP)));
    store.add_device(device("d_sam", "sam", DeviceKind::SubsonicApp, seal(SAM_APP)));
    store.note_detected(
        "x_owner",
        "winters",
        "Feishin",
        NetworkPlace::Home,
        None,
        Utc::now(),
    );
    let pending = state
        .family_requests
        .create(
            request("alex", "ext-deezer-song-1", RequestQuality::Flac),
            Utc::now(),
        )
        .id;

    let accounts = Arc::new(FakeAccounts::default());
    *accounts.users.lock() = vec![
        nd_user("u-winters", "winters", true),
        nd_user("u-alex", "alex", false),
        nd_user("u-sam", "sam", false),
        nd_user("u-bob", "bob", false),
    ];
    *accounts.libraries.lock() = vec![
        NdLibrary {
            id: 1,
            name: "Music".into(),
            path: "/music".into(),
        },
        NdLibrary {
            id: 3,
            name: "Alex's library".into(),
            path: "/members/alex".into(),
        },
        NdLibrary {
            id: 4,
            name: "Sam's library".into(),
            path: "/members/sam".into(),
        },
    ];
    let folders = Arc::new(FakeFolders::default());
    let files = Arc::new(FakeFiles::default());
    let library = Arc::new(FakeLibrary::default());
    let requests = Arc::new(FakeRequests {
        store: state.family_requests.clone(),
    });
    state.family.wire(&state, |hooks| {
        hooks.accounts = accounts.clone();
        hooks.folders = folders.clone();
        hooks.files = files.clone();
        hooks.requests = requests;
        hooks.storage = Arc::new(FakeStorage);
        hooks.my_library = library.clone();
        hooks.streams = Arc::new(FakeStreams);
        hooks.unrated = Arc::new(FakeUnrated);
    });
    let app = build_with(
        state.clone(),
        app_routes(&StaticAssets::load(&StaticRoots::from_env())),
    );
    Fam {
        app,
        state,
        nd,
        accounts,
        folders,
        files,
        library,
        pending,
        _dir: dir,
    }
}

impl Fam {
    fn member_cookie(&self, user: &str) -> String {
        format!(
            "octo_family={}",
            self.state
                .browse_sessions
                .create_family(user, SessionKind::Member)
        )
    }

    fn owner_cookie(&self) -> String {
        format!(
            "octo_family={}",
            self.state
                .browse_sessions
                .create_family("winters", SessionKind::FamilyNavidrome)
        )
    }

    fn alex(&self) -> String {
        self.member_cookie("alex")
    }

    fn sam(&self) -> String {
        self.member_cookie("sam")
    }

    /// A request from `from`, with the cookie and JSON body given, and the family header on
    /// every write.
    async fn call(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        from: &str,
    ) -> Reply {
        let write = !matches!(method, Method::GET | Method::HEAD);
        let mut headers: Vec<(&str, String)> = Vec::new();
        if let Some(cookie) = cookie {
            headers.push(("cookie", cookie.to_string()));
        }
        if write {
            headers.push(("x-octo-family", "1".into()));
            headers.push(("x-octo-admin", "1".into()));
        }
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        if !body.is_empty() {
            headers.push(("content-type", "application/json".into()));
        }
        raw(&self.app, method, uri, &headers, body, from).await
    }

    async fn get(&self, uri: &str, cookie: Option<&str>) -> Reply {
        self.call(Method::GET, uri, cookie, None, HOME).await
    }

    async fn post(&self, uri: &str, cookie: Option<&str>, body: Value) -> Reply {
        self.call(Method::POST, uri, cookie, Some(body), HOME).await
    }

    async fn put(&self, uri: &str, cookie: Option<&str>, body: Value) -> Reply {
        self.call(Method::PUT, uri, cookie, Some(body), HOME).await
    }

    async fn delete(&self, uri: &str, cookie: Option<&str>, body: Option<Value>) -> Reply {
        self.call(Method::DELETE, uri, cookie, body, HOME).await
    }

    /// A Subsonic call as alex's app.
    async fn rest_as_alex(&self, endpoint: &str, query: &str) -> Reply {
        let uri = format!("/rest/{endpoint}?u=alex&p={ALEX_APP}&v=1.16.1&c=test&f=json&{query}");
        raw(&self.app, Method::GET, &uri, &[], String::new(), HOME).await
    }

    /// A Subsonic call as the owner (Navidrome checks the sign-in).
    async fn rest_as_owner(&self, endpoint: &str, query: &str) -> Reply {
        let uri = format!("/rest/{endpoint}?u=winters&p=owner%20pass&v=1.16.1&c=test&f=json&{query}");
        raw(&self.app, Method::GET, &uri, &[], String::new(), HOME).await
    }

    async fn rest_as_sam(&self, endpoint: &str, query: &str) -> Reply {
        let uri = format!("/rest/{endpoint}?u=sam&p={SAM_APP}&v=1.16.1&c=test&f=json&{query}");
        raw(&self.app, Method::GET, &uri, &[], String::new(), HOME).await
    }
}

async fn raw(
    app: &App,
    method: Method,
    uri: &str,
    headers: &[(&str, String)],
    body: String,
    from: &str,
) -> Reply {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, value.as_str());
    }
    let mut req = req.body(Body::from(body)).expect("a request");
    let address: SocketAddr = format!("{from}:50000").parse().expect("an address");
    req.extensions_mut().insert(ConnectInfo(address));
    let res = app.clone().oneshot(req).await.expect("infallible");
    let (parts, body) = res.into_parts();
    let body = body.collect().await.expect("a body").to_bytes();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

// ------------------------------------------------------------------------------------------
// Signing in and joining
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn auth_signs_a_member_in_and_refuses_a_wrong_password() {
    let f = fam().await;
    let wrong = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "alex", "password": "nope nope" }),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.error(), "Wrong username or password.");

    let right = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "Alex", "password": ALEX_PASSWORD }),
        )
        .await;
    assert_eq!(right.status, StatusCode::OK, "{}", right.body);
    assert_eq!(right.json(), json!({ "username": "alex", "role": "Listener" }));
    let cookie = right
        .headers
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("a cookie")
        .to_string();
    assert!(cookie.contains("path=/api/family"), "{cookie}");
    assert!(
        cookie.contains("samesite=strict") && cookie.contains("httponly"),
        "{cookie}"
    );
    let session = f
        .get(
            "/api/family/session",
            Some(&format!("octo_family={}", right.cookie_token())),
        )
        .await;
    assert_eq!(session.json()["username"], "alex");
}

#[tokio::test]
async fn auth_signs_the_owner_in_through_navidrome() {
    let f = fam().await;
    let owner = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "winters", "password": "owner pass" }),
        )
        .await;
    assert_eq!(owner.status, StatusCode::OK, "{}", owner.body);
    assert_eq!(owner.json(), json!({ "username": "winters", "role": "Owner" }));
    let session = f
        .get(
            "/api/family/session",
            Some(&format!("octo_family={}", owner.cookie_token())),
        )
        .await;
    assert_eq!(session.json()["manager"], true);
    // An account outside the family signs in to choose its own audio quality only.
    let bob = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "bob", "password": "bob pass" }),
        )
        .await;
    assert_eq!(bob.json(), json!({ "username": "bob", "role": "Unmanaged" }));
    let wrong = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "winters", "password": "guess" }),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_is_throttled_after_ten_wrong_tries() {
    let f = fam().await;
    for _ in 0..10 {
        let r = f
            .post(
                "/api/family/auth",
                None,
                json!({ "username": "alex", "password": "nope nope" }),
            )
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let r = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "alex", "password": ALEX_PASSWORD }),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(r.headers.contains_key("retry-after"));
}

#[tokio::test]
async fn session_answers_who_is_signed_in_or_401() {
    let f = fam().await;
    let r = f.get("/api/family/session", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.json()["signIn"], true);
    let r = f.get("/api/family/session", Some(&f.sam())).await;
    assert_eq!(r.status, StatusCode::OK);
    let session = r.json();
    assert_eq!(session["role"], "CoAdmin");
    assert_eq!(session["manager"], true);
    assert_eq!(session["approver"], true);
    assert_eq!(session["familyEnabled"], true);
}

#[tokio::test]
async fn sign_out_ends_the_session() {
    let f = fam().await;
    let cookie = f.alex();
    let r = f.post("/api/family/signout", Some(&cookie), json!({})).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(
        r.headers
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.starts_with("octo_family=;"))
    );
    assert_eq!(
        f.get("/api/family/session", Some(&cookie)).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn writes_need_the_family_header() {
    let f = fam().await;
    let headers = vec![
        ("cookie", f.alex()),
        ("content-type", "application/json".to_string()),
    ];
    let r = raw(
        &f.app,
        Method::PUT,
        "/api/family/me/quality",
        &headers,
        json!({ "home": "High" }).to_string(),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.error(), "Family changes must come from the Family page.");
}

#[tokio::test]
async fn invite_flow_end_to_end() {
    let f = fam().await;
    // The owner adds kim, gets a link, kim opens it and sets a password, then signs in.
    let added = f
        .post(
            "/api/family/members",
            Some(&f.owner_cookie()),
            json!({ "username": "Kim", "displayName": "Kim", "preset": "Kid" }),
        )
        .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.body);
    let link = added.json()["inviteLink"].as_str().expect("a link").to_string();
    assert!(link.starts_with("http://localhost/family/join#invite="), "{link}");
    let token = link.split("#invite=").nth(1).expect("a token").to_string();

    let short = f
        .post(
            "/api/family/join",
            None,
            json!({ "token": token, "password": "short" }),
        )
        .await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);
    assert_eq!(short.error(), "Use at least 8 characters.");

    let joined = f
        .post(
            "/api/family/join",
            None,
            json!({ "token": token, "password": "kim password", "displayName": "Kimberly" }),
        )
        .await;
    assert_eq!(joined.status, StatusCode::OK, "{}", joined.body);
    assert_eq!(joined.json(), json!({ "username": "kim" }));
    let cookie = format!("octo_family={}", joined.cookie_token());
    let me = f.get("/api/family/me", Some(&cookie)).await.json();
    assert_eq!(me["displayName"], "Kimberly");
    assert_eq!(me["role"], "Kid");
    assert_eq!(me["abilities"]["cleanOnly"], true);

    // The link works once.
    let again = f
        .post(
            "/api/family/join",
            None,
            json!({ "token": token, "password": "kim password 2" }),
        )
        .await;
    assert_eq!(again.status, StatusCode::FORBIDDEN);
    let signed = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "kim", "password": "kim password" }),
        )
        .await;
    assert_eq!(signed.status, StatusCode::OK);
}

// ------------------------------------------------------------------------------------------
// Me
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn me_answers_the_plan_and_refuses_without_a_sign_in() {
    let f = fam().await;
    assert_eq!(
        f.get("/api/family/me", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let me = f.get("/api/family/me", Some(&f.alex())).await.json();
    assert_eq!(me["username"], "alex");
    assert_eq!(me["role"], "Listener");
    assert_eq!(me["managed"], true);
    assert_eq!(me["abilities"]["addToLibrary"], "Request");
    assert_eq!(me["requestsThisWeek"], 1);
    assert_eq!(me["storageUsedBytes"], 2_500_000_000u64);
    assert_eq!(me["place"], "Home");
    assert_eq!(me["quality"]["home"], "Original");
    // The owner has a plan too, without a device or storage.
    let owner = f.get("/api/family/me", Some(&f.owner_cookie())).await.json();
    assert_eq!(owner["role"], "Owner");
    assert_eq!(owner["managed"], false);
    assert_eq!(owner["deviceId"], Value::Null);
}

#[tokio::test]
async fn my_devices_lists_mine_and_refuses_without_a_sign_in() {
    let f = fam().await;
    assert_eq!(
        f.get("/api/family/me/devices", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let list = f.get("/api/family/me/devices", Some(&f.alex())).await.json();
    let ids: Vec<&str> = list
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|d| d["id"].as_str())
        .collect();
    assert_eq!(ids, ["d_app", "d_sub"]);
    assert_eq!(list[0]["playing"]["title"], "Hyperballad");
    assert_eq!(list[0]["kind"], "OctoApp");
    // The owner's own devices are the ones Octo saw.
    let owner = f
        .get("/api/family/me/devices", Some(&f.owner_cookie()))
        .await
        .json();
    assert_eq!(owner[0]["id"], "x_owner");
}

#[tokio::test]
async fn add_my_device_gives_an_app_password_once_and_refuses_the_owner() {
    let f = fam().await;
    let r = f
        .post(
            "/api/family/me/devices",
            Some(&f.alex()),
            json!({ "name": "Work laptop", "kind": "SubsonicApp" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let added = r.json();
    let password = added["appPassword"].as_str().expect("a password");
    assert_eq!(password.len(), 19);
    assert_eq!(added["pairCode"], Value::Null);
    assert_eq!(added["username"], "alex");
    assert_eq!(added["server"], "http://localhost");
    let id = added["deviceId"].as_str().expect("an id");
    let stored = f.state.family_store.device(id).expect("kept");
    assert_eq!(stored.name, "Work laptop");
    assert_eq!(
        f.state.family_vault.open_sealed(&stored.secret).as_deref(),
        Some(password)
    );
    // The owner's account is Navidrome's own, so Octo gives it no app passwords.
    let owner = f
        .post(
            "/api/family/me/devices",
            Some(&f.owner_cookie()),
            json!({ "name": "x", "kind": "SubsonicApp" }),
        )
        .await;
    assert_eq!(owner.status, StatusCode::FORBIDDEN);
    let bad = f
        .post(
            "/api/family/me/devices",
            Some(&f.alex()),
            json!({ "name": "x", "kind": "Fax" }),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn change_my_device_sets_its_quality_mode_and_refuses_others() {
    let f = fam().await;
    let r = f
        .put(
            "/api/family/me/devices/d_app",
            Some(&f.alex()),
            json!({ "quality": "App", "name": "Alex's Pixel" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["quality"], "App");
    assert_eq!(r.json()["name"], "Alex's Pixel");
    let other = f
        .put(
            "/api/family/me/devices/d_sam",
            Some(&f.alex()),
            json!({ "quality": "App" }),
        )
        .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sign_out_my_device_removes_mine_only() {
    let f = fam().await;
    let other = f
        .delete("/api/family/me/devices/d_sam", Some(&f.alex()), None)
        .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
    assert!(f.state.family_store.device("d_sam").is_some());
    let mine = f
        .delete("/api/family/me/devices/d_sub", Some(&f.alex()), None)
        .await;
    assert_eq!(mine.status, StatusCode::NO_CONTENT);
    assert!(f.state.family_store.device("d_sub").is_none());
    // A device signed out stops signing in at once.
    let r = f.rest_as_alex("getFamily", "").await;
    assert_eq!(r.subsonic()["error"]["code"], 40);
}

#[tokio::test]
async fn my_requests_lists_asks_and_cancels() {
    let f = fam().await;
    assert_eq!(
        f.get("/api/family/me/requests", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let mine = f.get("/api/family/me/requests", Some(&f.alex())).await.json();
    assert_eq!(mine.as_array().expect("a list").len(), 1);
    assert_eq!(mine[0]["displayName"], "Alex");
    assert_eq!(mine[0]["quality"], "Flac");

    let asked = f
        .post(
            "/api/family/me/requests",
            Some(&f.alex()),
            json!({ "id": "ext-deezer-song-2", "quality": "Mp3" }),
        )
        .await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.body);
    let id = asked.json()["id"].as_str().expect("an id").to_string();
    assert_eq!(asked.json()["state"], "Pending");
    let refused = f
        .post(
            "/api/family/me/requests",
            Some(&f.alex()),
            json!({ "id": "ext-refused" }),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(
        refused.error(),
        "You have used all 10 of your requests this week."
    );

    let canceled = f
        .delete(&format!("/api/family/me/requests/{id}"), Some(&f.alex()), None)
        .await;
    assert_eq!(canceled.json()["state"], "Cancelled");
    // Someone else's request is not theirs to cancel.
    let sams = f
        .delete(
            &format!("/api/family/me/requests/{}", f.pending),
            Some(&f.sam()),
            None,
        )
        .await;
    assert_eq!(sams.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn my_saved_lists_and_forgets() {
    let f = fam().await;
    f.state.family_saved.add(
        "alex",
        serde_json::from_value(
            json!({ "Id": "ext-deezer-song-9", "Title": "Saved one", "Saved": "2026-10-20T18:00:00Z" }),
        )
        .expect("a saved song"),
    );
    let saved = f.get("/api/family/me/saved", Some(&f.alex())).await.json();
    assert_eq!(saved["songs"].as_array().map(Vec::len), Some(1));
    assert_eq!(saved["albums"], json!([]));
    assert_eq!(
        f.delete("/api/family/me/saved/ext-deezer-song-9", Some(&f.alex()), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.delete("/api/family/me/saved/ext-deezer-song-9", Some(&f.alex()), None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.get("/api/family/me/saved", Some(&f.owner_cookie()))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn my_library_pages_and_removes() {
    let f = fam().await;
    let page = f
        .get("/api/family/me/library?offset=1&size=1", Some(&f.alex()))
        .await
        .json();
    assert_eq!(page["total"], 3);
    assert_eq!(page["songs"][0]["id"], "s1");
    assert_eq!(page["usedBytes"], 2_500_000_000u64);
    assert_eq!(page["limitBytes"], 10_000_000_000u64);
    assert_eq!(
        f.delete("/api/family/me/library/s2", Some(&f.alex()), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(f.library.removed.lock().clone(), ["s2"]);
    let shared = f
        .delete("/api/family/me/library/shared-1", Some(&f.alex()), None)
        .await;
    assert_eq!(shared.status, StatusCode::FORBIDDEN);
    assert_eq!(
        f.get("/api/family/me/library", None).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn my_password_changes_with_the_current_one_only() {
    let f = fam().await;
    let wrong = f
        .put(
            "/api/family/me/password",
            Some(&f.alex()),
            json!({ "current": "nope nope", "next": "a new password" }),
        )
        .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let right = f
        .put(
            "/api/family/me/password",
            Some(&f.alex()),
            json!({ "current": ALEX_PASSWORD, "next": "a new password" }),
        )
        .await;
    assert_eq!(right.status, StatusCode::NO_CONTENT);
    let member = f.state.family_store.member("alex").expect("there");
    assert!(passwords::verify("a new password", &member.account_password));
}

#[tokio::test]
async fn my_quality_is_for_anyone_signed_in() {
    let f = fam().await;
    let r = f
        .put(
            "/api/family/me/quality",
            Some(&f.owner_cookie()),
            json!({ "away": "Standard" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        r.json(),
        json!({ "home": "Original", "away": "Standard", "familyLimitKbps": 0, "familyAwayLimitKbps": 0 })
    );
    assert_eq!(
        f.state.family_store.preference("winters").away.label(),
        "Standard"
    );
    let bad = f
        .put(
            "/api/family/me/quality",
            Some(&f.alex()),
            json!({ "home": "Loud" }),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        f.put("/api/family/me/quality", None, json!({ "home": "High" }))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn my_ntfy_topic_is_checked() {
    let f = fam().await;
    let r = f
        .put(
            "/api/family/me/ntfy",
            Some(&f.alex()),
            json!({ "topic": "alex-K3j9x2mQ7pL4vB8nR5tY1wZ0" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        f.state.family_store.member("alex").expect("there").ntfy_topic,
        "alex-K3j9x2mQ7pL4vB8nR5tY1wZ0"
    );
    let bad = f
        .put(
            "/api/family/me/ntfy",
            Some(&f.alex()),
            json!({ "topic": "https://ntfy.sh/x" }),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        f.put(
            "/api/family/me/ntfy",
            Some(&f.owner_cookie()),
            json!({ "topic": "x" })
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn my_last_fm_is_for_members() {
    let f = fam().await;
    let r = f.get("/api/family/me/lastfm", Some(&f.alex())).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["available"], false);
    let connect = f
        .post("/api/family/me/lastfm/connect", Some(&f.alex()), json!({}))
        .await;
    assert_eq!(connect.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        f.get("/api/family/me/lastfm", Some(&f.owner_cookie()))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

// ------------------------------------------------------------------------------------------
// Managing
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn members_are_listed_for_managers_only() {
    let f = fam().await;
    let r = f.get("/api/family/members", Some(&f.sam())).await;
    assert_eq!(r.status, StatusCode::OK);
    let alex = r.json()[0].clone();
    assert_eq!(alex["username"], "alex");
    assert_eq!(alex["devices"], 2);
    assert_eq!(alex["playingNow"], true);
    assert_eq!(alex["pendingRequests"], 1);
    assert_eq!(alex["storageLimitGb"], 10);
    assert_eq!(alex["sharedLibraries"], "All");
    assert_eq!(alex["edits"], json!({}));
    let refused = f.get("/api/family/members", Some(&f.alex())).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn add_member_makes_the_account_folder_and_library() {
    let f = fam().await;
    let refused = f
        .post(
            "/api/family/members",
            Some(&f.alex()),
            json!({ "username": "kim" }),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let r = f
        .post(
            "/api/family/members",
            Some(&f.sam()),
            json!({ "username": "kim", "displayName": "Kim", "preset": "Member", "sharedLibraries": [1] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["member"]["role"], "Member");
    assert_eq!(r.json()["member"]["joined"], false);
    let calls = f.accounts.calls();
    assert_eq!(
        calls,
        [
            "create_user kim Kim",
            "create_library Kim's library /members/kim",
            "libraries u-kim [1, 10]",
        ]
    );
    let kim = f.state.family_store.member("kim").expect("kept");
    assert_eq!(kim.personal_library_id, Some(10));
    assert!(
        f.state
            .family_vault
            .open_sealed(&kim.navidrome_password)
            .is_some()
    );
    // A co-admin cannot make co-admins, and names are checked.
    let co = f
        .post(
            "/api/family/members",
            Some(&f.sam()),
            json!({ "username": "zed", "preset": "CoAdmin" }),
        )
        .await;
    assert_eq!(co.status, StatusCode::FORBIDDEN);
    let bad = f
        .post(
            "/api/family/members",
            Some(&f.sam()),
            json!({ "username": "a b" }),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn navidrome_cannot_see_the_members_folder_is_said_plainly_and_cleaned_up() {
    let f = fam().await;
    *f.accounts.refuse_library.lock() = Some((400, "path does not exist or is not a directory".into()));
    let r = f
        .post(
            "/api/family/members",
            Some(&f.owner_cookie()),
            json!({ "username": "kim" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let members = f.state.settings.current().family.effective_members_path();
    assert_eq!(
        r.error(),
        format!(
            "Navidrome cannot see the members folder. Add the same {members} volume to the navidrome service."
        )
    );
    assert_eq!(f.accounts.calls(), ["create_user kim kim", "delete_user u-kim"]);
    assert_eq!(f.folders.calls.lock().clone(), ["create kim", "remove kim"]);
    assert!(f.state.family_store.member("kim").is_none());
}

#[tokio::test]
async fn adopting_an_account_needs_the_choice_and_sets_a_hidden_password() {
    let f = fam().await;
    let r = f
        .post(
            "/api/family/members",
            Some(&f.owner_cookie()),
            json!({ "username": "bob" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.error().contains("will have to sign in again"), "{}", r.error());
    let r = f
        .post(
            "/api/family/members",
            Some(&f.owner_cookie()),
            json!({ "username": "bob", "adopt": true }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(f.accounts.calls()[0], "set_password u-bob");
    assert_eq!(
        f.state
            .family_store
            .member("bob")
            .expect("kept")
            .navidrome_user_id,
        "u-bob"
    );
    // A Navidrome admin is never adopted.
    let admin = f
        .post(
            "/api/family/members",
            Some(&f.owner_cookie()),
            json!({ "username": "winters", "adopt": true }),
        )
        .await;
    assert_eq!(admin.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn edit_member_changes_abilities_and_libraries() {
    let f = fam().await;
    let r = f
        .put(
            "/api/family/members/alex",
            Some(&f.owner_cookie()),
            json!({ "edits": { "awayCap": 128, "offlineCopies": false }, "sharedLibraries": [1] }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["abilities"]["awayCap"], 128);
    assert_eq!(
        r.json()["edits"],
        json!({ "awayCap": 128, "offlineCopies": false })
    );
    let stored = f.state.family_store.member("alex").expect("there");
    assert_eq!(stored.edits.get("AwayCap"), Some(&json!(128)));
    assert!(
        f.accounts
            .calls()
            .contains(&"libraries u-alex [1, 3]".to_string())
    );
    let unknown = f
        .put(
            "/api/family/members/alex",
            Some(&f.owner_cookie()),
            json!({ "edits": { "flying": true } }),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    // Pausing someone ends their Family page sign-in.
    let cookie = f.alex();
    let paused = f
        .put(
            "/api/family/members/alex",
            Some(&f.sam()),
            json!({ "suspended": true }),
        )
        .await;
    assert_eq!(paused.json()["suspended"], true);
    assert_eq!(
        f.get("/api/family/me", Some(&cookie)).await.status,
        StatusCode::UNAUTHORIZED
    );
    let refused = f
        .put(
            "/api/family/members/sam",
            Some(&f.alex()),
            json!({ "suspended": true }),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.error(), "This account is paused by the family owner.");
}

#[tokio::test]
async fn delete_member_needs_typed_name() {
    let f = fam().await;
    let typo = f
        .delete(
            "/api/family/members/alex",
            Some(&f.owner_cookie()),
            Some(json!({ "confirm": "alx", "files": "delete" })),
        )
        .await;
    assert_eq!(typo.status, StatusCode::BAD_REQUEST);
    assert_eq!(typo.error(), "Type alex to confirm removing them.");
    assert!(f.accounts.calls().is_empty());
    assert!(f.files.released.lock().is_empty());
    let no_choice = f
        .delete(
            "/api/family/members/alex",
            Some(&f.owner_cookie()),
            Some(json!({ "confirm": "alex" })),
        )
        .await;
    assert_eq!(no_choice.status, StatusCode::BAD_REQUEST);
    let refused = f
        .delete(
            "/api/family/members/alex",
            Some(&f.alex()),
            Some(json!({ "confirm": "alex", "files": "keep" })),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);

    let removed = f
        .delete(
            "/api/family/members/alex",
            Some(&f.owner_cookie()),
            Some(json!({ "confirm": " Alex ", "files": "promote" })),
        )
        .await;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.body);
    assert_eq!(
        f.files.released.lock().clone(),
        [("alex".to_string(), FilesChoice::Promote)]
    );
    assert_eq!(f.accounts.calls(), ["delete_library 3", "delete_user u-alex"]);
    assert!(f.state.family_store.member("alex").is_none());
    assert!(f.state.family_store.device("d_app").is_none());
}

#[tokio::test]
async fn invite_member_sends_a_new_sign_in_link() {
    let f = fam().await;
    let r = f
        .post("/api/family/members/alex/invite", Some(&f.sam()), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["inviteDays"], 7);
    let link = r.json()["inviteLink"].as_str().expect("a link").to_string();
    let token = link.split("#invite=").nth(1).expect("a token");
    // The new link sets a new password: how a forgotten one is replaced.
    let joined = f
        .post(
            "/api/family/join",
            None,
            json!({ "token": token, "password": "brand new pass" }),
        )
        .await;
    assert_eq!(joined.status, StatusCode::OK);
    assert!(passwords::verify(
        "brand new pass",
        &f.state
            .family_store
            .member("alex")
            .expect("there")
            .account_password
    ));
    assert_eq!(
        f.post("/api/family/members/alex/invite", Some(&f.alex()), json!({}))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.post("/api/family/members/nobody/invite", Some(&f.sam()), json!({}))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn repair_member_sets_a_new_hidden_password() {
    let f = fam().await;
    let before = f
        .state
        .family_store
        .member("alex")
        .expect("there")
        .navidrome_password;
    let r = f
        .post(
            "/api/family/members/alex/repair",
            Some(&f.owner_cookie()),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    assert_eq!(f.accounts.calls()[0], "set_password u-alex");
    assert_ne!(
        f.state
            .family_store
            .member("alex")
            .expect("there")
            .navidrome_password,
        before
    );
    assert_eq!(
        f.post("/api/family/members/alex/repair", Some(&f.alex()), json!({}))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn requests_inbox_is_for_approvers() {
    let f = fam().await;
    let all = f.get("/api/family/requests?state=Pending", Some(&f.sam())).await;
    assert_eq!(all.status, StatusCode::OK);
    assert_eq!(all.json().as_array().map(Vec::len), Some(1));
    let none = f.get("/api/family/requests?state=Done", Some(&f.sam())).await;
    assert_eq!(none.json(), json!([]));
    assert_eq!(
        f.get("/api/family/requests", Some(&f.alex())).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.get("/api/family/requests?state=Lost", Some(&f.sam()))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn approve_decides_a_request() {
    let f = fam().await;
    let uri = format!("/api/family/requests/{}/approve", f.pending);
    assert_eq!(
        f.post(&uri, Some(&f.alex()), json!({})).await.status,
        StatusCode::FORBIDDEN
    );
    let r = f
        .post(&uri, Some(&f.owner_cookie()), json!({ "note": "Enjoy" }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["state"], "Approved");
    assert_eq!(r.json()["decidedBy"], "winters");
    assert_eq!(r.json()["note"], "Enjoy");
}

#[tokio::test]
async fn decline_decides_a_request_with_a_note() {
    let f = fam().await;
    let uri = format!("/api/family/requests/{}/decline", f.pending);
    assert_eq!(
        f.post(&uri, Some(&f.alex()), json!({})).await.status,
        StatusCode::FORBIDDEN
    );
    let r = f
        .post(&uri, Some(&f.sam()), json!({ "note": "We have it on vinyl" }))
        .await;
    assert_eq!(r.json()["state"], "Declined");
    assert_eq!(r.json()["note"], "We have it on vinyl");
    let missing = f
        .post("/api/family/requests/r_none/decline", Some(&f.sam()), json!({}))
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn devices_lists_everyone_for_managers() {
    let f = fam().await;
    let r = f.get("/api/family/devices", Some(&f.sam())).await;
    assert_eq!(r.status, StatusCode::OK);
    let ids: Vec<String> = r
        .json()
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|d| d["id"].as_str().map(str::to_string))
        .collect();
    assert_eq!(ids, ["d_app", "d_sub", "d_sam", "x_owner"]);
    assert_eq!(
        f.get("/api/family/devices", Some(&f.alex())).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn sign_out_device_is_for_managers_and_never_a_seen_device() {
    let f = fam().await;
    assert_eq!(
        f.delete("/api/family/devices/d_app", Some(&f.alex()), None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let seen = f
        .delete("/api/family/devices/x_owner", Some(&f.sam()), None)
        .await;
    assert_eq!(seen.status, StatusCode::FORBIDDEN);
    let r = f.delete("/api/family/devices/d_app", Some(&f.sam()), None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(f.state.family_store.device("d_app").is_none());
}

#[tokio::test]
async fn playlists_are_listed_made_changed_and_deleted() {
    let f = fam().await;
    let made = f
        .post(
            "/api/family/playlists",
            Some(&f.alex()),
            json!({ "name": "Road trip" }),
        )
        .await;
    assert_eq!(made.status, StatusCode::OK, "{}", made.body);
    let id = made.json()["id"].as_str().expect("an id").to_string();
    assert!(id.starts_with("of"));
    assert_eq!(made.json()["createdBy"], "alex");
    // Only managers ask for approval of additions.
    let approval = f
        .post(
            "/api/family/playlists",
            Some(&f.alex()),
            json!({ "name": "Strict", "approveAdditions": true }),
        )
        .await;
    assert_eq!(approval.status, StatusCode::FORBIDDEN);
    let list = f
        .get("/api/family/playlists", Some(&f.owner_cookie()))
        .await
        .json();
    assert_eq!(list[0]["name"], "Road trip");

    let renamed = f
        .put(
            &format!("/api/family/playlists/{id}"),
            Some(&f.alex()),
            json!({ "name": "Long drive" }),
        )
        .await;
    assert_eq!(renamed.json()["name"], "Long drive");
    let strict = f
        .put(
            &format!("/api/family/playlists/{id}"),
            Some(&f.sam()),
            json!({ "approveAdditions": true }),
        )
        .await;
    assert_eq!(strict.json()["approveAdditions"], true);
    assert_eq!(
        f.delete(&format!("/api/family/playlists/{id}"), Some(&f.alex()), None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.delete(&format!("/api/family/playlists/{id}"), Some(&f.sam()), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    // A kid cannot make family playlists.
    f.state
        .family_store
        .upsert_member(member(&f.state, "kim", Preset::Kid, None, None));
    let kid = f
        .post(
            "/api/family/playlists",
            Some(&f.member_cookie("kim")),
            json!({ "name": "Mine" }),
        )
        .await;
    assert_eq!(kid.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn load_is_for_managers() {
    let f = fam().await;
    let r = f.get("/api/family/load", Some(&f.owner_cookie())).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json(),
        json!({
            "streams": [{ "username": "alex", "device": "Pixel 9", "songTitle": "Hyperballad", "kbps": 160, "place": "Away" }],
            "shim": null,
            "queue": [],
            "uploadKbps": 160,
        })
    );
    assert_eq!(
        f.get("/api/family/load", Some(&f.alex())).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn load_reads_the_helper_slots_from_its_health() {
    let shim = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true, "active_streams": 4, "ytdlp_running": 3, "ytdlp_limit": 128,
        })))
        .mount(&shim)
        .await;
    let f = fam_on(|_| {}, None, Some(&shim.uri())).await;
    let r = f.get("/api/family/load", Some(&f.owner_cookie())).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json()["shim"],
        json!({ "active": 4, "limit": 128, "ytdlpRunning": 3 })
    );
    // A second look within the cache window reads the same answer without asking again.
    let again = f.get("/api/family/load", Some(&f.owner_cookie())).await;
    assert_eq!(again.json()["shim"], r.json()["shim"]);
    assert_eq!(shim.received_requests().await.expect("recorded").len(), 1);
}

#[tokio::test]
async fn load_says_null_for_the_helper_when_it_does_not_answer() {
    let shim = MockServer::start().await;
    Mock::given(path("/health"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&shim)
        .await;
    let f = fam_on(|_| {}, None, Some(&shim.uri())).await;
    let r = f.get("/api/family/load", Some(&f.owner_cookie())).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["shim"], Value::Null, "{}", r.body);
    assert_eq!(r.json()["uploadKbps"], 160);
}

#[tokio::test]
async fn enable_is_for_the_owner_and_reports_the_probe() {
    let f = fam_with(|s| s.family.enabled = false, None).await;
    let refused = f
        .post("/api/family/enable", Some(&f.alex()), json!({ "enabled": true }))
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let r = f
        .post(
            "/api/family/enable",
            Some(&f.owner_cookie()),
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let report = r.json();
    assert_eq!(report["enabled"], true);
    assert_eq!(report["links"], "Copy");
    let members = f.state.settings.current().family.effective_members_path();
    assert_eq!(report["membersPath"], members);
    assert!(
        report["message"]
            .as_str()
            .is_some_and(|m| m
                .starts_with("This disk cannot link files, so a song two members want takes space twice.")),
        "{report}"
    );
    assert!(std::path::Path::new(&members).is_dir());
    assert!(f.state.settings.current().family.enabled);
    // A co-admin is not the owner.
    let sam = f
        .post("/api/family/enable", Some(&f.sam()), json!({ "enabled": false }))
        .await;
    assert_eq!(sam.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn libraries_are_listed_for_managers() {
    let f = fam().await;
    let r = f.get("/api/family/libraries", Some(&f.sam())).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json()[0],
        json!({ "id": 1, "name": "Music", "path": "/music", "personal": false, "owner": null })
    );
    assert_eq!(r.json()[1]["owner"], "alex");
    assert_eq!(
        f.get("/api/family/libraries", Some(&f.alex())).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn unrated_card_counts_and_starts_marking() {
    let f = fam().await;
    let r = f.get("/api/family/unrated", Some(&f.owner_cookie())).await;
    assert_eq!(r.json(), json!({ "count": 12, "kids": false }));
    let mark = f
        .post("/api/family/unrated/mark", Some(&f.owner_cookie()), json!({}))
        .await;
    assert_eq!(mark.json()["message"], "Octo is looking the songs up now.");
    assert_eq!(
        f.get("/api/family/unrated", Some(&f.alex())).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn away_manager_refused_when_off() {
    let f = fam_with(|s| s.family.manage_from_away = false, None).await;
    let away = f
        .call(Method::GET, "/api/family/members", Some(&f.sam()), None, AWAY)
        .await;
    assert_eq!(away.status, StatusCode::FORBIDDEN);
    assert_eq!(
        away.error(),
        "Managing the family from away from home is off on this server."
    );
    let approve = f
        .call(
            Method::POST,
            &format!("/api/family/requests/{}/approve", f.pending),
            Some(&f.owner_cookie()),
            Some(json!({})),
            AWAY,
        )
        .await;
    assert_eq!(approve.status, StatusCode::FORBIDDEN);
    let home = f
        .call(Method::GET, "/api/family/members", Some(&f.sam()), None, HOME)
        .await;
    assert_eq!(home.status, StatusCode::OK);
    // A member's own things still work away from home.
    let me = f
        .call(Method::GET, "/api/family/me", Some(&f.alex()), None, AWAY)
        .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json()["place"], "Away");
    // And with the setting on, managing works away too.
    let open = fam().await;
    let away = open
        .call(Method::GET, "/api/family/members", Some(&open.sam()), None, AWAY)
        .await;
    assert_eq!(away.status, StatusCode::OK);
}

// ------------------------------------------------------------------------------------------
// Pairing
// ------------------------------------------------------------------------------------------

async fn pair_code(f: &Fam, name: &str) -> String {
    let r = f
        .post(
            "/api/family/me/devices",
            Some(&f.alex()),
            json!({ "name": name, "kind": "OctoApp" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let added = r.json();
    assert_eq!(added["appPassword"], Value::Null);
    assert!(added["expires"].is_string());
    let code = added["pairCode"].as_str().expect("a code").to_string();
    assert_eq!(code.len(), 6);
    code
}

async fn pair(f: &Fam, user: &str, code: &str, from: &str) -> Reply {
    let uri = format!(
        "/rest/octoFamilyPair.view?username={user}&code={code}&deviceName=Pixel%209&platform=Android&f=json"
    );
    raw(&f.app, Method::GET, &uri, &[], String::new(), from).await
}

#[tokio::test]
async fn pair_code_flow_end_to_end() {
    let f = fam().await;
    let code = pair_code(&f, "").await;
    let wrong = pair(
        &f,
        "alex",
        if code == "123456" { "654321" } else { "123456" },
        HOME,
    )
    .await;
    assert_eq!(wrong.subsonic()["error"]["code"], 40);
    assert_eq!(wrong.subsonic()["error"]["message"], PAIR_REFUSED);
    // Without the username the code means nothing.
    let nameless = pair(&f, "sam", &code, HOME).await;
    assert_eq!(nameless.subsonic()["error"]["code"], 40);

    let r = pair(&f, "alex", &code, HOME).await;
    let answer = r.subsonic();
    assert_eq!(answer["status"], "ok", "{}", r.body);
    assert_eq!(answer["type"], "octo");
    let paired = &answer["familyPair"];
    assert_eq!(paired["username"], "alex");
    assert_eq!(paired["server"], "http://localhost");
    let secret = paired["secret"].as_str().expect("a secret").to_string();
    assert_eq!(secret.len(), 32);
    let id = paired["deviceId"].as_str().expect("an id").to_string();
    let device = f.state.family_store.device(&id).expect("kept");
    assert_eq!(device.kind, DeviceKind::OctoApp);
    assert_eq!(device.name, "Pixel 9");
    assert_eq!(device.app, "Octo (Android)");
    assert_eq!(
        f.state.family_vault.open_sealed(&device.secret).as_deref(),
        Some(secret.as_str())
    );

    // The code is used up.
    assert_eq!(
        pair(&f, "alex", &code, HOME).await.subsonic()["error"]["code"],
        40
    );

    // The secret signs the app in, and the app knows which device it is.
    let uri = format!("/rest/getFamily?u=alex&p={secret}&v=1.16.1&c=Octo&f=json");
    let me = raw(&f.app, Method::GET, &uri, &[], String::new(), HOME)
        .await
        .subsonic();
    assert_eq!(me["family"]["me"]["deviceId"], id.as_str());

    // A form post pairs too, and another app can ask for an app password instead.
    let code = pair_code(&f, "Desktop").await;
    let body = format!("username=alex&code={code}&platform=Windows&kind=SubsonicApp&f=json");
    let form = raw(
        &f.app,
        Method::POST,
        "/rest/octoFamilyPair",
        &[("content-type", "application/x-www-form-urlencoded".to_string())],
        body,
        HOME,
    )
    .await;
    let paired = form.subsonic()["familyPair"].clone();
    assert_eq!(paired["secret"].as_str().map(str::len), Some(19), "{}", form.body);
    let device = f
        .state
        .family_store
        .device(paired["deviceId"].as_str().expect("an id"))
        .expect("kept");
    assert_eq!(device.kind, DeviceKind::SubsonicApp);
    assert_eq!(device.name, "Desktop");
}

/// Makes a device on `route` as `cookie`, ending the pairing code `replaces` names. Answers
/// the pairing code (empty for an app password) and the device id.
async fn add_replacing(f: &Fam, route: &str, cookie: &str, kind: &str, replaces: &str) -> (String, String) {
    let r = f
        .post(
            route,
            Some(cookie),
            json!({ "name": "", "kind": kind, "replaces": replaces }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let added = r.json();
    let id = added["deviceId"].as_str().expect("an id").to_string();
    (added["pairCode"].as_str().unwrap_or_default().to_string(), id)
}

#[tokio::test]
async fn replaces_ends_the_old_unused_code() {
    let f = fam().await;
    let mine = "/api/family/me/devices";
    let (old_code, old_id) = add_replacing(&f, mine, &f.alex(), "OctoApp", "").await;
    let (new_code, new_id) = add_replacing(&f, mine, &f.alex(), "OctoApp", &old_id).await;
    assert_ne!(old_id, new_id);
    if old_code != new_code {
        assert_eq!(
            pair(&f, "alex", &old_code, HOME).await.subsonic()["error"]["code"],
            40
        );
    }
    // The new code pairs, and its device takes the id the code reserved.
    let r = pair(&f, "alex", &new_code, HOME).await;
    assert_eq!(
        r.subsonic()["familyPair"]["deviceId"],
        new_id.as_str(),
        "{}",
        r.body
    );
    assert!(f.state.family_store.device(&new_id).is_some());
    assert!(f.state.family_store.device(&old_id).is_none());

    // Switching to an app password ends the code the dialog showed.
    let (code, id) = add_replacing(&f, mine, &f.alex(), "OctoApp", "").await;
    assert!(f.state.family_store.any_live_pair_code(Utc::now()));
    let (none, _) = add_replacing(&f, mine, &f.alex(), "SubsonicApp", &id).await;
    assert!(none.is_empty());
    assert!(!f.state.family_store.any_live_pair_code(Utc::now()));
    assert_eq!(
        pair(&f, "alex", &code, HOME).await.subsonic()["error"]["code"],
        40
    );

    // The app's own call ends it the same way.
    let (_, id) = add_replacing(&f, mine, &f.alex(), "OctoApp", "").await;
    let r = f
        .rest_as_alex("addFamilyDevice", &format!("kind=SubsonicApp&replaces={id}"))
        .await;
    assert_eq!(r.subsonic()["status"], "ok", "{}", r.body);
    assert!(!f.state.family_store.any_live_pair_code(Utc::now()));
}

#[tokio::test]
async fn replaces_never_touches_a_used_code() {
    let f = fam().await;
    let mine = "/api/family/me/devices";
    let (code, id) = add_replacing(&f, mine, &f.alex(), "OctoApp", "").await;
    let r = pair(&f, "alex", &code, HOME).await;
    assert_eq!(r.subsonic()["familyPair"]["deviceId"], id.as_str(), "{}", r.body);
    let secret = r.subsonic()["familyPair"]["secret"]
        .as_str()
        .expect("a secret")
        .to_string();
    // Naming the used code changes nothing about the device it paired.
    let (_, next) = add_replacing(&f, mine, &f.alex(), "OctoApp", &id).await;
    assert_ne!(next, id);
    add_replacing(&f, mine, &f.alex(), "SubsonicApp", &id).await;
    let device = f.state.family_store.device(&id).expect("still there");
    assert_eq!(device.username, "alex");
    let uri = format!("/rest/getFamily?u=alex&p={secret}&v=1.16.1&c=Octo&f=json");
    let me = raw(&f.app, Method::GET, &uri, &[], String::new(), HOME)
        .await
        .subsonic();
    assert_eq!(me["family"]["me"]["deviceId"], id.as_str());
}

#[tokio::test]
async fn replaces_never_touches_another_members_code() {
    let f = fam().await;
    let (code, id) = add_replacing(&f, "/api/family/me/devices", &f.alex(), "OctoApp", "").await;
    // Sam names Alex's code from Sam's own devices, both kinds, and as a manager for Sam.
    add_replacing(&f, "/api/family/me/devices", &f.sam(), "SubsonicApp", &id).await;
    add_replacing(&f, "/api/family/me/devices", &f.sam(), "OctoApp", &id).await;
    add_replacing(
        &f,
        "/api/family/members/sam/devices",
        &f.sam(),
        "SubsonicApp",
        &id,
    )
    .await;
    let r = pair(&f, "alex", &code, HOME).await;
    assert_eq!(r.subsonic()["status"], "ok", "{}", r.body);
    assert_eq!(r.subsonic()["familyPair"]["deviceId"], id.as_str());
}

#[tokio::test]
async fn global_pair_cap_locks_guessing() {
    let f = fam().await;
    let code = pair_code(&f, "").await;
    // Thirty wrong codes from three addresses, none against alex's own code.
    for n in 0..PAIR_CAP {
        let from = format!("10.0.{}.1", n % 3);
        let r = pair(&f, &format!("guess{n}"), "111111", &from).await;
        assert_eq!(r.subsonic()["error"]["code"], 40);
    }
    // Now even the right code is refused while a code is live, from anywhere.
    let r = pair(&f, "alex", &code, "10.9.9.9").await;
    assert_eq!(r.subsonic()["error"]["message"], PAIR_REFUSED);
    // The code itself was not touched: it still works once the window passes (below).
    assert!(f.state.family_store.any_live_pair_code(Utc::now()));
}

#[test]
fn pair_guard_counts_a_window_per_address_and_for_everyone() {
    let now = Arc::new(Mutex::new(Utc::now()));
    let clock = {
        let now = now.clone();
        Clock::new(move || *now.lock())
    };
    let guard = PairGuard::new(clock);
    for _ in 0..10 {
        guard.failed("10.0.0.1");
    }
    assert!(guard.address_blocked("10.0.0.1"));
    assert!(!guard.address_blocked("10.0.0.2"));
    for n in 0..20 {
        guard.failed(&format!("10.1.0.{n}"));
    }
    assert!(guard.capped());
    *now.lock() += TimeDelta::minutes(11);
    assert!(!guard.capped(), "the window passed");
    *now.lock() += TimeDelta::minutes(5);
    assert!(!guard.address_blocked("10.0.0.1"));
}

#[test]
fn usernames_are_checked() {
    for good in ["alex", "a.b", "kim_2", "sam-s", "ab"] {
        assert!(valid_username(good), "{good}");
    }
    for bad in ["a", "Alex", "a b", "émile", &"a".repeat(33), "alex/x"] {
        assert!(!valid_username(bad), "{bad}");
    }
}

// ------------------------------------------------------------------------------------------
// octoFamily
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn get_family_answers_me_and_the_manager_view() {
    let f = fam().await;
    let alex = f.rest_as_alex("getFamily", "").await.subsonic();
    assert_eq!(alex["status"], "ok");
    assert_eq!(alex["type"], "octo");
    assert_eq!(alex["family"]["me"]["deviceId"], "d_sub");
    assert_eq!(alex["family"]["me"]["role"], "Listener");
    assert!(alex["family"].get("manager").is_none());

    let owner = f.rest_as_owner("getFamily", "").await.subsonic();
    assert_eq!(owner["family"]["me"]["role"], "Owner");
    assert_eq!(owner["family"]["me"]["deviceId"], Value::Null);
    let manager = &owner["family"]["manager"];
    assert_eq!(manager["pendingRequests"], 1);
    assert_eq!(manager["liveStreams"], 1);
    assert_eq!(manager["members"][0]["username"], "alex");

    // A sign-in Navidrome refuses is refused.
    Mock::given(method("GET"))
        .and(path("/rest/ping"))
        .and(wiremock::matchers::query_param("u", "mallory"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":40,"message":"Wrong username or password"}}}"#,
            "application/json",
        ))
        .with_priority(1)
        .mount(&f.nd)
        .await;
    let uri = "/rest/getFamily?u=mallory&p=x&v=1.16.1&c=test&f=json";
    let r = raw(&f.app, Method::GET, uri, &[], String::new(), HOME).await;
    assert_eq!(r.subsonic()["error"]["code"], 40);
}

#[tokio::test]
async fn get_family_answers_xml() {
    let f = fam().await;
    let uri = format!("/rest/getFamily.view?u=alex&p={ALEX_APP}&v=1.16.1&c=test");
    let r = raw(&f.app, Method::GET, &uri, &[], String::new(), HOME).await;
    assert!(r.body.contains(r#"type="octo""#), "{}", r.body);
    assert!(
        r.body.contains("<family>") || r.body.contains("<family "),
        "{}",
        r.body
    );
    assert!(r.body.contains(r#"username="alex""#), "{}", r.body);
    assert!(r.body.contains("<abilities "), "{}", r.body);
    assert!(r.body.contains(r#"addToLibrary="Request""#), "{}", r.body);
}

#[tokio::test]
async fn set_family_quality_answers_the_quality() {
    let f = fam().await;
    let r = f
        .rest_as_owner("setFamilyQuality", "away=DataSaver")
        .await
        .subsonic();
    assert_eq!(
        r["quality"],
        json!({ "home": "Original", "away": "DataSaver", "familyLimitKbps": 0, "familyAwayLimitKbps": 0 })
    );
    let bad = f.rest_as_alex("setFamilyQuality", "home=Loud").await.subsonic();
    assert_eq!(bad["error"]["code"], 10);
}

#[tokio::test]
async fn signed_request_reaches_member_routes() {
    let f = fam().await;
    // The Octo app signs Family API calls like any Subsonic call: no cookie, no header.
    let token = octo_core::family::codes::subsonic_token(ALEX_SECRET, "c19b2d");
    let auth = format!("u=alex&t={token}&s=c19b2d&v=1.16.1&c=Octo");
    let me = raw(
        &f.app,
        Method::GET,
        &format!("/api/family/me?{auth}"),
        &[],
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(me.json()["username"], "alex");
    assert_eq!(me.json()["deviceId"], "d_app");
    let devices = raw(
        &f.app,
        Method::GET,
        &format!("/api/family/me/devices?{auth}"),
        &[],
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(devices.json()[0]["current"], true);
    let quality = raw(
        &f.app,
        Method::PUT,
        &format!("/api/family/me/quality?{auth}"),
        &[("content-type", "application/json".to_string())],
        json!({ "away": "Standard" }).to_string(),
        HOME,
    )
    .await;
    assert_eq!(quality.status, StatusCode::OK, "{}", quality.body);
    assert_eq!(quality.json()["away"], "Standard");
    // A wrong signature is refused, and managers' routes stay closed to a listener.
    let wrong = raw(
        &f.app,
        Method::GET,
        "/api/family/me?u=alex&t=00000000000000000000000000000000&s=c19b2d&v=1.16.1&c=Octo",
        &[],
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    let members = raw(
        &f.app,
        Method::GET,
        &format!("/api/family/members?{auth}"),
        &[],
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(members.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn signed_manager_can_edit_members() {
    let f = fam().await;
    let r = raw(
        &f.app,
        Method::PUT,
        &format!("/api/family/members/alex?u=sam&p={SAM_APP}&v=1.16.1&c=Octo"),
        &[("content-type", "application/json".to_string())],
        json!({ "edits": { "awayCap": 96 } }).to_string(),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["abilities"]["awayCap"], 96);
    // The owner signs with their Navidrome password, which Navidrome checks.
    let owner = raw(
        &f.app,
        Method::GET,
        "/api/family/members?u=winters&p=owner%20pass&v=1.16.1&c=Octo",
        &[],
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(owner.status, StatusCode::OK, "{}", owner.body);
}

#[tokio::test]
async fn unsigned_write_without_header_refused() {
    let f = fam().await;
    let r = raw(
        &f.app,
        Method::PUT,
        "/api/family/members/alex",
        &[
            ("cookie", f.owner_cookie()),
            ("content-type", "application/json".to_string()),
        ],
        json!({ "suspended": true }).to_string(),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.error(), "Family changes must come from the Family page.");
    assert!(!f.state.family_store.member("alex").expect("there").suspended);
}

#[tokio::test]
async fn manager_adds_a_device_for_a_member() {
    let f = fam().await;
    let before = f
        .state
        .family_store
        .member("alex")
        .expect("there")
        .account_password;
    let r = f
        .post(
            "/api/family/members/alex/devices",
            Some(&f.sam()),
            json!({ "name": "Alex's tablet", "kind": "SubsonicApp" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["username"], "alex");
    assert_eq!(r.json()["appPassword"].as_str().map(str::len), Some(19));
    assert_eq!(
        f.state
            .family_store
            .member("alex")
            .expect("there")
            .account_password,
        before
    );
    assert_eq!(
        f.post(
            "/api/family/members/sam/devices",
            Some(&f.alex()),
            json!({ "kind": "OctoApp" })
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    // The same through addFamilyDevice with a username.
    let code = f
        .rest_as_sam("addFamilyDevice", "name=Phone&kind=OctoApp&username=alex")
        .await
        .subsonic();
    assert_eq!(code["familyDeviceAdded"]["username"], "alex");
    assert_eq!(
        code["familyDeviceAdded"]["pairCode"].as_str().map(str::len),
        Some(6)
    );
    let refused = f
        .rest_as_alex("addFamilyDevice", "name=x&kind=OctoApp&username=sam")
        .await
        .subsonic();
    assert_eq!(refused["error"]["code"], 50);
}

#[tokio::test]
async fn set_family_device_quality_changes_own_devices_only() {
    let f = fam().await;
    let r = f
        .rest_as_alex("setFamilyDeviceQuality", "id=d_app&mode=App")
        .await
        .subsonic();
    assert_eq!(r["device"]["quality"], "App");
    assert_eq!(r["device"]["current"], false);
    let other = f
        .rest_as_alex("setFamilyDeviceQuality", "id=d_sam&mode=App")
        .await
        .subsonic();
    assert_eq!(other["error"]["code"], 70);
}

#[tokio::test]
async fn get_family_requests_mine_or_all_for_approvers() {
    let f = fam().await;
    let mine = f.rest_as_alex("getFamilyRequests", "").await.subsonic();
    assert_eq!(
        mine["familyRequests"]["request"].as_array().map(Vec::len),
        Some(1)
    );
    let all = f.rest_as_alex("getFamilyRequests", "all=true").await.subsonic();
    assert_eq!(all["error"]["code"], 50);
    let sam = f
        .rest_as_sam("getFamilyRequests", "all=true&state=Pending")
        .await
        .subsonic();
    assert_eq!(sam["familyRequests"]["request"][0]["username"], "alex");
}

#[tokio::test]
async fn request_copy_makes_a_request_for_members_only() {
    let f = fam().await;
    let r = f
        .rest_as_alex("requestCopy", "id=ext-deezer-song-7&quality=Flac")
        .await
        .subsonic();
    assert_eq!(r["familyRequest"]["state"], "Pending", "{r}");
    assert_eq!(r["familyRequest"]["quality"], "Flac");
    let missing = f.rest_as_alex("requestCopy", "").await.subsonic();
    assert_eq!(missing["error"]["code"], 10);
    let owner = f
        .rest_as_owner("requestCopy", "id=ext-deezer-song-7")
        .await
        .subsonic();
    assert_eq!(owner["error"]["code"], 50);
}

#[tokio::test]
async fn cancel_family_request_is_the_requesters() {
    let f = fam().await;
    let sam = f
        .rest_as_sam("cancelFamilyRequest", &format!("id={}", f.pending))
        .await
        .subsonic();
    assert_eq!(sam["error"]["code"], 70);
    let alex = f
        .rest_as_alex("cancelFamilyRequest", &format!("id={}", f.pending))
        .await
        .subsonic();
    assert_eq!(alex["familyRequest"]["state"], "Cancelled");
}

#[tokio::test]
async fn decide_family_request_needs_approve_requests() {
    let f = fam().await;
    let alex = f
        .rest_as_alex("decideFamilyRequest", &format!("id={}&approve=true", f.pending))
        .await
        .subsonic();
    assert_eq!(alex["error"]["code"], 50);
    let sam = f
        .rest_as_sam(
            "decideFamilyRequest",
            &format!("id={}&approve=false&note=Not%20now", f.pending),
        )
        .await
        .subsonic();
    assert_eq!(sam["familyRequest"]["state"], "Declined");
    assert_eq!(sam["familyRequest"]["note"], "Not now");
}

#[tokio::test]
async fn get_family_devices_mine_or_all_for_managers() {
    let f = fam().await;
    let mine = f.rest_as_alex("getFamilyDevices", "").await.subsonic();
    let list = mine["familyDevices"]["device"]
        .as_array()
        .expect("a list")
        .clone();
    assert_eq!(list.len(), 2);
    let current: Vec<&Value> = list.iter().filter(|d| d["current"] == true).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0]["id"], "d_sub");
    assert_eq!(
        f.rest_as_alex("getFamilyDevices", "all=true").await.subsonic()["error"]["code"],
        50
    );
    let all = f.rest_as_owner("getFamilyDevices", "all=true").await.subsonic();
    let ids: Vec<&str> = all["familyDevices"]["device"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|d| d["id"].as_str())
        .collect();
    // Members' devices, the owner's seen device, and the app making this call.
    assert_eq!(&ids[..4], ["d_app", "d_sub", "d_sam", "x_owner"]);
    assert_eq!(ids.len(), 5);
}

#[tokio::test]
async fn sign_out_family_device_own_or_any_member_for_managers() {
    let f = fam().await;
    let other = f.rest_as_alex("signOutFamilyDevice", "id=d_sam").await.subsonic();
    assert_eq!(other["error"]["code"], 70);
    let seen = f
        .rest_as_owner("signOutFamilyDevice", "id=x_owner")
        .await
        .subsonic();
    assert_eq!(seen["error"]["code"], 50);
    let r = f
        .rest_as_owner("signOutFamilyDevice", "id=d_app")
        .await
        .subsonic();
    assert_eq!(r["status"], "ok");
    assert!(f.state.family_store.device("d_app").is_none());
}

#[tokio::test]
async fn add_family_device_gives_a_code_or_a_password() {
    let f = fam().await;
    let code = f
        .rest_as_alex("addFamilyDevice", "name=Tablet&kind=OctoApp")
        .await
        .subsonic();
    let added = &code["familyDeviceAdded"];
    assert_eq!(added["kind"], "OctoApp");
    assert_eq!(added["pairCode"].as_str().map(str::len), Some(6));
    assert_eq!(added["appPassword"], Value::Null);
    let password = f
        .rest_as_alex("addFamilyDevice", "name=Car&kind=SubsonicApp")
        .await
        .subsonic();
    assert_eq!(
        password["familyDeviceAdded"]["appPassword"]
            .as_str()
            .map(str::len),
        Some(19)
    );
    let owner = f
        .rest_as_owner("addFamilyDevice", "name=x&kind=SubsonicApp")
        .await
        .subsonic();
    assert_eq!(owner["error"]["code"], 50);
}

#[tokio::test]
async fn remove_from_my_library_refuses_shared_songs() {
    let f = fam().await;
    let ok = f.rest_as_alex("removeFromMyLibrary", "id=s1").await.subsonic();
    assert_eq!(ok["status"], "ok");
    let shared = f
        .rest_as_alex("removeFromMyLibrary", "id=shared-1")
        .await
        .subsonic();
    assert_eq!(shared["error"]["code"], 50);
    assert_eq!(
        shared["error"]["message"],
        "That song is in the family library, not yours."
    );
    let owner = f.rest_as_owner("removeFromMyLibrary", "id=s1").await.subsonic();
    assert_eq!(owner["error"]["code"], 50);
}

#[tokio::test]
async fn create_family_playlist_answers_a_playlist_node() {
    let f = fam().await;
    let r = f
        .rest_as_alex("createFamilyPlaylist", "name=Road%20trip")
        .await
        .subsonic();
    assert_eq!(r["playlist"]["name"], "Road trip", "{r}");
    assert!(
        r["playlist"]["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("of"))
    );
    assert_eq!(r["playlist"]["owner"], "Family");
    let approval = f
        .rest_as_alex("createFamilyPlaylist", "name=Strict&approveAdditions=true")
        .await
        .subsonic();
    assert_eq!(approval["error"]["code"], 50);
}

#[tokio::test]
async fn family_extension_only_when_on() {
    for on in [true, false] {
        let f = fam_with(|s| s.family.enabled = on, None).await;
        Mock::given(method("GET"))
            .and(path("/rest/getOpenSubsonicExtensions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"subsonic-response":{"status":"ok","version":"1.16.1","openSubsonicExtensions":[]}}"#,
                "application/json",
            ))
            .mount(&f.nd)
            .await;
        let r = f.rest_as_owner("getOpenSubsonicExtensions", "").await;
        assert_eq!(
            r.body.contains(r#"{"name":"octoFamily","versions":[1]}"#),
            on,
            "{}",
            r.body
        );
    }
}

#[tokio::test]
async fn octo_family_calls_reach_navidrome_while_family_is_off() {
    let f = fam_with(|s| s.family.enabled = false, None).await;
    let r = f.rest_as_owner("getFamily", "").await;
    assert!(
        r.body.is_empty() || !r.body.contains("\"type\":\"octo\""),
        "{}",
        r.body
    );
    let relayed = f.nd.received_requests().await.unwrap_or_default();
    assert!(relayed.iter().any(|r| r.url.path() == "/rest/getFamily"));
}

// ------------------------------------------------------------------------------------------
// Edges
// ------------------------------------------------------------------------------------------

#[tokio::test]
async fn member_session_cannot_reach_admin_api() {
    let f = fam_with(|_| {}, Some("required")).await;
    let token = f.state.browse_sessions.create_family("alex", SessionKind::Member);
    // The family token in the dashboard's cookie or header is refused outright.
    for headers in [
        vec![("cookie", format!("octo_browse={token}"))],
        vec![("x-octo-browse-token", token.clone())],
    ] {
        let r = raw(
            &f.app,
            Method::GET,
            "/api/admin/settings",
            &headers,
            String::new(),
            HOME,
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
        assert_eq!(r.error(), "This page is for the server owner.");
    }
    // The owner's Family page sign-in is no dashboard sign-in either.
    let owner = f
        .state
        .browse_sessions
        .create_family("winters", SessionKind::FamilyNavidrome);
    let headers = vec![("cookie", format!("octo_browse={owner}"))];
    let r = raw(
        &f.app,
        Method::GET,
        "/api/admin/family",
        &headers,
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // And the family cookie alone opens nothing there.
    let headers = vec![("cookie", format!("octo_family={token}"))];
    let r = raw(
        &f.app,
        Method::GET,
        "/api/admin/settings",
        &headers,
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_family_never_relays() {
    let f = fam().await;
    for (method, uri) in [
        (Method::GET, "/api/family/nothing"),
        (Method::PATCH, "/api/family/me"),
        (Method::GET, "/api/family"),
        (Method::GET, "/family/nothing.html"),
    ] {
        let r = f.call(method.clone(), uri, Some(&f.alex()), None, HOME).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    let relayed: Vec<String> =
        f.nd.received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .map(|r| r.url.path().to_string())
            .filter(|p| p.contains("family"))
            .collect();
    assert!(relayed.is_empty(), "{relayed:?}");
    // With Family off, the page's API says so rather than relaying.
    let off = fam_with(|s| s.family.enabled = false, None).await;
    let r = off.get("/api/family/me", Some(&off.alex())).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.error(), "Family is off on this server.");
}

#[tokio::test]
async fn family_page_is_served() {
    let f = fam().await;
    for (uri, kind) in [
        ("/family", "text/html"),
        ("/family/", "text/html"),
        ("/family/index.html", "text/html"),
        ("/family/join", "text/html"),
        ("/family/family.js", "text/javascript"),
        ("/family/family.css", "text/css"),
    ] {
        let r = f.get(uri, None).await;
        assert_eq!(r.status, StatusCode::OK, "{uri}");
        assert_eq!(
            r.headers.get("content-type").and_then(|v| v.to_str().ok()),
            Some(kind),
            "{uri}"
        );
    }
    let page = f.get("/family", None).await.body;
    assert!(page.contains("<title>"), "{page}");
    let join = f.get("/family/join", None).await.body;
    assert!(join.contains("join.js") || join.contains("family.js"), "{join}");
}

#[tokio::test]
async fn cors_never_on_api_family() {
    let f = fam().await;
    let headers = vec![
        ("origin", "https://elsewhere.example".to_string()),
        ("cookie", f.alex()),
    ];
    let r = raw(
        &f.app,
        Method::GET,
        "/api/family/me",
        &headers,
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers.get("access-control-allow-origin").is_none());
    let preflight = vec![
        ("origin", "https://elsewhere.example".to_string()),
        ("access-control-request-method", "PUT".to_string()),
        ("access-control-request-headers", "x-octo-family".to_string()),
    ];
    let r = raw(
        &f.app,
        Method::OPTIONS,
        "/api/family/me/quality",
        &preflight,
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(r.headers.get("access-control-allow-origin").is_none());
    assert!(r.headers.get("access-control-allow-methods").is_none());
    // Subsonic answers keep their CORS, which web players need.
    let headers = vec![("origin", "https://elsewhere.example".to_string())];
    let r = raw(
        &f.app,
        Method::GET,
        "/rest/ping?u=winters&p=x&f=json",
        &headers,
        String::new(),
        HOME,
    )
    .await;
    assert_eq!(
        r.headers
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("*")
    );
}

#[tokio::test]
async fn status_answer_never_mentions_family() {
    let f = fam().await;
    let r = f.get("/api/admin/status", None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.json().get("family").is_none(), "{}", r.body);
    let off = fam_with(|s| s.family.enabled = false, None).await;
    let r = off.get("/api/admin/status", None).await;
    assert!(r.json().get("family").is_none(), "{}", r.body);
}

#[tokio::test]
async fn admin_family_entry_summarizes_and_turns_family_on() {
    let f = fam_with(|s| s.family.enabled = false, Some("required")).await;
    assert_eq!(
        f.get("/api/admin/family", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    let admin = format!("octo_browse={}", f.state.browse_sessions.create("winters"));
    let summary = f.get("/api/admin/family", Some(&admin)).await;
    assert_eq!(summary.status, StatusCode::OK, "{}", summary.body);
    let summary = summary.json();
    assert_eq!(summary["supported"], true);
    assert_eq!(summary["enabled"], false);
    assert_eq!(summary["members"].as_array().map(Vec::len), Some(2));
    assert_eq!(summary["pendingRequests"], 1);
    assert_eq!(summary["notifyRequests"], true);

    let on = f
        .post(
            "/api/admin/family/enable",
            Some(&admin),
            json!({ "enabled": true }),
        )
        .await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(on.json()["enabled"], true);
    assert_eq!(on.json()["links"], "Copy");
    // Another Navidrome admin is not the family's owner.
    let other = format!("octo_browse={}", f.state.browse_sessions.create("other-admin"));
    let refused = f
        .post(
            "/api/admin/family/enable",
            Some(&other),
            json!({ "enabled": false }),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(refused.error(), "Only winters can turn Family on or off.");
}

#[tokio::test]
async fn admin_family_notifications_switch() {
    let f = fam_with(|_| {}, Some("required")).await;
    let admin = format!("octo_browse={}", f.state.browse_sessions.create("winters"));
    let r = f
        .put(
            "/api/admin/family/notifications",
            Some(&admin),
            json!({ "devices": false }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        r.json(),
        json!({ "notifyRequests": true, "notifyDevices": false })
    );
    assert!(!f.state.settings.current().notifications.notify_family_devices);
    let member = format!(
        "octo_browse={}",
        f.state.browse_sessions.create_family("alex", SessionKind::Member)
    );
    let refused = f
        .put(
            "/api/admin/family/notifications",
            Some(&member),
            json!({ "devices": true }),
        )
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn family_off_keeps_the_owner_able_to_sign_in_and_turn_it_on() {
    let f = fam_with(|s| s.family.enabled = false, None).await;
    let r = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "winters", "password": "owner pass" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let cookie = format!("octo_family={}", r.cookie_token());
    let session = f.get("/api/family/session", Some(&cookie)).await.json();
    assert_eq!(session["familyEnabled"], false);
    assert_eq!(session["owner"], true);
    // Members cannot sign in while it is off.
    let alex = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "alex", "password": ALEX_PASSWORD }),
        )
        .await;
    assert_eq!(alex.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn new_device_tells_the_owner_once() {
    let f = fam().await;
    let store: &Arc<FamilyStore> = &f.state.family_store;
    let mut events = f.state.family_devices.subscribe();
    let r = f.rest_as_alex("getFamily", "").await;
    assert_eq!(r.subsonic()["status"], "ok");
    let event = events.try_recv().expect("a new device");
    assert_eq!(event.device_id, "d_sub");
    assert!(store.device("d_sub").expect("there").last_seen.is_some());
}

#[tokio::test]
async fn the_family_page_says_plainly_when_family_is_off() {
    let f = fam_with(|s| s.family.enabled = false, None).await;
    for uri in [
        "/family",
        "/family/",
        "/family/index.html",
        "/family/join",
        "/family/join.html",
    ] {
        let r = f.get(uri, None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{uri}");
        assert!(
            r.body.contains("Family is off on this server."),
            "{uri}: {}",
            r.body
        );
        assert!(!r.body.contains("family.js"), "{uri}: {}", r.body);
    }
    // The page's own files are still served like any other.
    assert_eq!(f.get("/family/family.css", None).await.status, StatusCode::OK);
    // With Family on, the page is the page.
    let on = fam().await;
    let r = on.get("/family/", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains("Family is off on this server."), "{}", r.body);
}

#[tokio::test]
async fn the_family_page_names_the_setting_for_an_untrusted_proxy() {
    let f = fam().await;
    let through = |cookie: String| {
        vec![
            ("cookie", cookie),
            ("x-forwarded-for", "198.51.100.7".to_string()),
        ]
    };
    let r = raw(
        &f.app,
        Method::GET,
        "/api/family/session",
        &through(f.owner_cookie()),
        String::new(),
        "172.18.0.5",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let session = r.json();
    let warning = session["proxyWarning"].as_str().expect("a warning");
    assert!(warning.contains("Family:TrustedProxies"), "{warning}");
    assert!(warning.contains("172.18.0.5"), "{warning}");
    assert_eq!(session["place"], "Away");
    // A member is not told how to change the server.
    let r = raw(
        &f.app,
        Method::GET,
        "/api/family/session",
        &through(f.alex()),
        String::new(),
        "172.18.0.5",
    )
    .await;
    assert!(r.json()["proxyWarning"].is_null(), "{}", r.body);
    // Without a proxy in the way there is nothing to say.
    let r = f.get("/api/family/session", Some(&f.owner_cookie())).await;
    assert!(r.json()["proxyWarning"].is_null(), "{}", r.body);
}

// ------------------------------------------------------------------------------------------
// Sign-in throttles across addresses
// ------------------------------------------------------------------------------------------

/// Wrong passwords for one account from many addresses count together, so the owner's
/// Navidrome password cannot be guessed from address after address.
#[tokio::test]
async fn auth_tries_for_one_account_from_many_addresses_count_together() {
    let f = fam().await;
    let wrong = json!({ "username": "winters", "password": "guess" });
    for n in 0..crate::services::family::member_bridge::ACCOUNT_FAILURES {
        let from = format!("203.0.{}.{}", n / 200, n % 200 + 1);
        let r = f
            .call(Method::POST, "/api/family/auth", None, Some(wrong.clone()), &from)
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
    }
    let right = json!({ "username": "winters", "password": "owner pass" });
    let r = f
        .call(
            Method::POST,
            "/api/family/auth",
            None,
            Some(right),
            "198.51.100.9",
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.body);
    assert!(r.headers.contains_key("retry-after"));
    // Another account is not held back.
    let bob = json!({ "username": "bob", "password": "bob pass" });
    let r = f
        .call(Method::POST, "/api/family/auth", None, Some(bob), "198.51.100.9")
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
}

/// The owner signed in from an address before: a flood of wrong passwords for the owner's
/// account from new addresses holds back every new address (100 tries in 15 minutes, as
/// before) and never that one. Its own wrong tries still count: 10 from it, then it waits too.
#[tokio::test]
async fn auth_tries_from_new_addresses_never_lock_the_owner_out_of_a_known_address() {
    use crate::services::admin::SignInThrottle;
    use crate::services::family::member_bridge::ACCOUNT_FAILURES;
    let f = fam().await;
    let known = "198.51.100.50";
    let right = json!({ "username": "winters", "password": "owner pass" });
    let r = f
        .call(Method::POST, "/api/family/auth", None, Some(right.clone()), known)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let wrong = json!({ "username": "winters", "password": "guess" });
    for n in 0..ACCOUNT_FAILURES {
        let from = format!("203.0.{}.{}", n / 200, n % 200 + 1);
        let r = f
            .call(Method::POST, "/api/family/auth", None, Some(wrong.clone()), &from)
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
    }
    let r = f
        .call(
            Method::POST,
            "/api/family/auth",
            None,
            Some(right.clone()),
            "198.51.100.9",
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::TOO_MANY_REQUESTS,
        "a new address waits: {}",
        r.body
    );
    let r = f
        .call(Method::POST, "/api/family/auth", None, Some(right.clone()), known)
        .await;
    assert_eq!(r.status, StatusCode::OK, "the known address does not: {}", r.body);
    for _ in 0..SignInThrottle::MAX_FAILURES {
        let r = f
            .call(Method::POST, "/api/family/auth", None, Some(wrong.clone()), known)
            .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
    }
    let r = f
        .call(Method::POST, "/api/family/auth", None, Some(right), known)
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.body);
}

/// A device on the home network naming a new X-Forwarded-For hop on every try still counts
/// against the address that connected.
#[tokio::test]
async fn auth_tries_naming_new_forwarded_hops_count_against_the_peer() {
    let f = fam().await;
    for n in 0..crate::services::family::member_bridge::PEER_FAILURES {
        // A different account each time, so only the peer's count adds up.
        let wrong = json!({ "username": format!("guess-{n}"), "password": "guess" });
        let hop = format!("203.0.{}.{}", n / 200, n % 200 + 1);
        let r = raw(
            &f.app,
            Method::POST,
            "/api/family/auth",
            &[
                ("x-octo-family", "1".into()),
                ("content-type", "application/json".into()),
                ("x-forwarded-for", hop),
            ],
            wrong.to_string(),
            HOME,
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
    }
    let right = json!({ "username": "winters", "password": "owner pass" });
    let r = raw(
        &f.app,
        Method::POST,
        "/api/family/auth",
        &[
            ("x-octo-family", "1".into()),
            ("content-type", "application/json".into()),
            ("x-forwarded-for", "198.51.100.200".into()),
        ],
        right.to_string(),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.body);
}

/// A sign-in, a join or a password change takes a small body only.
#[tokio::test]
async fn sign_in_routes_take_a_small_body_only() {
    let f = fam().await;
    let huge = json!({ "username": "alex", "password": "x".repeat(64 * 1024) });
    for uri in ["/api/family/auth", "/api/family/join"] {
        let r = f.post(uri, None, huge.clone()).await;
        assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{uri}: {}", r.body);
    }
    let r = f
        .put(
            "/api/family/me/password",
            Some(&f.alex()),
            json!({ "current": "x".repeat(64 * 1024), "next": "y" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body);
}

/// A password over the cap is wrong without being hashed.
#[tokio::test]
async fn a_password_over_the_cap_is_wrong() {
    let f = fam().await;
    let r = f
        .post(
            "/api/family/auth",
            None,
            json!({ "username": "alex", "password": "x".repeat(passwords::MAX_BYTES + 1) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
}

#[test]
fn pair_guard_counts_per_peer_and_per_username_across_addresses() {
    let guard = PairGuard::new(Clock::system());
    use crate::services::family::family_service::{PAIR_ACCOUNT_FAILURES, PAIR_PEER_FAILURES};
    for n in 0..PAIR_ACCOUNT_FAILURES {
        guard.failed_from(&format!("10.0.0.{n}"), &format!("10.0.0.{n}"), "Alex");
    }
    assert!(
        guard.blocked("10.9.9.9", "10.9.9.9", "alex"),
        "the username waits"
    );
    assert!(!guard.blocked("10.9.9.9", "10.9.9.9", "sam"));
    let guard = PairGuard::new(Clock::system());
    for n in 0..PAIR_PEER_FAILURES {
        guard.failed_from(&format!("203.0.113.{n}"), "192.168.1.5", &format!("user-{n}"));
    }
    assert!(
        guard.blocked("198.51.100.1", "192.168.1.5", "someone"),
        "the peer waits, whatever hop it names"
    );
    assert!(!guard.blocked("198.51.100.1", "192.168.1.6", "someone"));
}

// ------------------------------------------------------------------------------------------
// The outside address, links for anywhere and for home, and the dashboard's handoff
// ------------------------------------------------------------------------------------------

/// A write on the Family page's API from `from`, with `host` as the Host header and any
/// other headers given.
async fn write_from(
    f: &Fam,
    method: Method,
    uri: &str,
    cookie: &str,
    body: Value,
    host: &str,
    extra: &[(&str, String)],
    from: &str,
) -> Reply {
    let mut headers = vec![
        ("cookie", cookie.to_string()),
        ("x-octo-family", "1".to_string()),
        ("content-type", "application/json".to_string()),
        ("host", host.to_string()),
    ];
    headers.extend(extra.iter().cloned());
    raw(&f.app, method, uri, &headers, body.to_string(), from).await
}

async fn alex_code_from(f: &Fam, host: &str) -> Value {
    let r = write_from(
        f,
        Method::POST,
        "/api/family/me/devices",
        &f.alex(),
        json!({ "name": "", "kind": "OctoApp" }),
        host,
        &[],
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.json()
}

async fn session_from(f: &Fam, cookie: &str, host: &str, extra: &[(&str, String)], from: &str) -> Value {
    let mut headers = vec![("cookie", cookie.to_string()), ("host", host.to_string())];
    headers.extend(extra.iter().cloned());
    let r = raw(
        &f.app,
        Method::GET,
        "/api/family/session",
        &headers,
        String::new(),
        from,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.json()
}

#[tokio::test]
async fn home_only_when_public_url_unset_and_lan() {
    let f = fam().await;
    let added = alex_code_from(&f, "192.168.50.119:5274").await;
    assert_eq!(added["homeOnly"], true, "{added}");
    assert_eq!(added["publicUrl"], Value::Null);
    assert_eq!(added["server"], "http://192.168.50.119:5274");
    let code = added["pairCode"].as_str().expect("a code");
    assert_eq!(
        added["links"]["home"],
        format!("http://192.168.50.119:5274/family/join#u=alex&c={code}")
    );
    assert_eq!(added["links"]["anywhere"], Value::Null);
    let session = session_from(&f, &f.owner_cookie(), "octo.lan:5274", &[], HOME).await;
    assert_eq!(session["homeOnly"], true);
    // A name the outside world can reach is not home only, even without an outside address.
    let added = alex_code_from(&f, "music.example.com").await;
    assert_eq!(added["homeOnly"], false, "{added}");
    // With an outside address set, nothing is home only.
    let set = fam_with(|s| s.server.public_url = "https://music.example.com".into(), None).await;
    let added = alex_code_from(&set, "192.168.50.119:5274").await;
    assert_eq!(added["homeOnly"], false, "{added}");
    let session = session_from(&set, &set.owner_cookie(), "192.168.50.119:5274", &[], HOME).await;
    assert_eq!(session["homeOnly"], false);
    assert_eq!(session["publicUrl"], "https://music.example.com");
}

#[tokio::test]
async fn suggested_public_url_only_from_trusted_proxy() {
    let f = fam().await;
    let through = [
        ("x-forwarded-host", "music.example.com".to_string()),
        ("x-forwarded-proto", "https".to_string()),
    ];
    // From a peer that is not a trusted proxy the headers mean nothing.
    let session = session_from(&f, &f.owner_cookie(), "192.168.1.20:5274", &through, HOME).await;
    assert_eq!(session["suggestedPublicUrl"], Value::Null);
    // Through the trusted proxy (127.0.0.0/8 by default) the address is offered to the owner.
    let session = session_from(&f, &f.owner_cookie(), "octo:5274", &through, "127.0.0.1").await;
    assert_eq!(session["suggestedPublicUrl"], "https://music.example.com");
    let admin = f.get("/api/admin/family", None).await.json();
    assert_eq!(admin["suggestedPublicUrl"], "https://music.example.com");
    assert_eq!(admin["publicUrl"], Value::Null);
    // Members are never offered it, and it is never applied on its own.
    let alex = session_from(&f, &f.alex(), "192.168.1.20:5274", &[], HOME).await;
    assert_eq!(alex["suggestedPublicUrl"], Value::Null);
    assert_eq!(f.state.settings.current().server.public_url, "");
    assert_eq!(alex_code_from(&f, "192.168.1.20:5274").await["homeOnly"], true);
    // A home address through the proxy is never suggested over it.
    let lan = [("x-forwarded-host", "192.168.1.2:5274".to_string())];
    session_from(&f, &f.owner_cookie(), "octo:5274", &lan, "127.0.0.1").await;
    assert_eq!(
        f.state.family.suggested_public_url().as_deref(),
        Some("https://music.example.com")
    );
}

#[tokio::test]
async fn public_address_save_validates() {
    let f = fam().await;
    let put = |cookie: String, url: &'static str| {
        let f = &f;
        async move {
            f.put("/api/family/public-address", Some(&cookie), json!({ "url": url }))
                .await
        }
    };
    assert_eq!(
        put(f.alex(), "https://music.example.com").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        put(f.sam(), "https://music.example.com").await.status,
        StatusCode::FORBIDDEN
    );
    for bad in [
        "ftp://music.example.com",
        "https://me:secret@music.example.com",
        "https://music.example.com/?x=1",
        "https://",
    ] {
        let r = put(f.owner_cookie(), bad).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.body);
    }
    assert_eq!(f.state.settings.current().server.public_url, "");
    let r = put(f.owner_cookie(), "  music.example.com/octo/ ").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["publicUrl"], "https://music.example.com/octo");
    let r = put(f.owner_cookie(), "").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["publicUrl"], Value::Null);
    assert_eq!(f.state.settings.current().server.public_url, "");
}

#[tokio::test]
async fn public_address_put_writes_the_status_page_setting() {
    let f = fam().await;
    let r = f
        .put(
            "/api/family/public-address",
            Some(&f.owner_cookie()),
            json!({ "url": "music.example.com/" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    // The one setting the dashboard's Status page shows, normalized as it shows it.
    assert_eq!(
        f.state.settings.current().server.public_url,
        "https://music.example.com"
    );
    let settings = f.get("/api/admin/settings", None).await;
    assert_eq!(settings.status, StatusCode::OK, "{}", settings.body);
    assert_eq!(
        settings.json()["Server"]["PublicUrl"],
        "https://music.example.com",
        "{}",
        settings.body
    );
    // Links use it from now on.
    let added = alex_code_from(&f, "192.168.1.20:5274").await;
    assert_eq!(added["server"], "https://music.example.com");
}

#[tokio::test]
async fn check_reports_reachable() {
    let f = fam().await;
    // GET /family/hello answers this Octo's id to anyone, even with Family off.
    let hello = f.get("/family/hello", None).await;
    assert_eq!(hello.status, StatusCode::OK, "{}", hello.body);
    assert_eq!(hello.json()["instance"], f.state.family.instance());
    let off = fam_with(|s| s.family.enabled = false, Some("required")).await;
    assert_eq!(off.get("/family/hello", None).await.status, StatusCode::OK);

    let this = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/octo/family/hello"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "octo": "family", "instance": f.state.family.instance() })),
        )
        .mount(&this)
        .await;
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/family/hello"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "octo": "family", "instance": "another" })),
        )
        .mount(&other)
        .await;
    let check = |url: String| {
        let f = &f;
        async move {
            f.post(
                "/api/family/public-address/check",
                Some(&f.owner_cookie()),
                json!({ "url": url }),
            )
            .await
        }
    };
    let r = check(format!("{}/octo/", this.uri())).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["result"], "Reachable", "{}", r.body);
    assert_eq!(r.json()["reachable"], true);
    let r = check(other.uri()).await;
    assert_eq!(r.json()["result"], "NotThisServer", "{}", r.body);
    let closed = format!("http://{}", crate::services::test_support::refused_address());
    let r = check(closed).await;
    assert_eq!(r.json()["result"], "CouldNotReach", "{}", r.body);
    assert!(
        r.json()["message"]
            .as_str()
            .is_some_and(|m| m.contains("mobile data")),
        "{}",
        r.body
    );
    // Only the owner checks.
    let r = f
        .post(
            "/api/family/public-address/check",
            Some(&f.alex()),
            json!({ "url": other.uri() }),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn links_carry_home_when_public_differs() {
    let f = fam_with(|s| s.server.public_url = "https://music.example.com".into(), None).await;
    let added = alex_code_from(&f, "192.168.1.5:5274").await;
    let code = added["pairCode"].as_str().expect("a code");
    assert_eq!(
        added["links"]["anywhere"],
        format!("https://music.example.com/family/join#u=alex&c={code}&home=http%3A%2F%2F192.168.1.5%3A5274")
    );
    assert_eq!(
        added["links"]["home"],
        format!("http://192.168.1.5:5274/family/join#u=alex&c={code}")
    );
    let invite = write_from(
        &f,
        Method::POST,
        "/api/family/members/alex/invite",
        &f.sam(),
        json!({}),
        "192.168.1.5:5274",
        &[],
        HOME,
    )
    .await
    .json();
    let link = invite["inviteLink"].as_str().expect("a link");
    assert!(
        link.starts_with("https://music.example.com/family/join#invite="),
        "{link}"
    );
    assert!(link.ends_with("&home=http%3A%2F%2F192.168.1.5%3A5274"), "{link}");
    // Through the outside address, home is still the home address seen last.
    let added = alex_code_from(&f, "music.example.com").await;
    assert_eq!(added["servers"]["home"], "http://192.168.1.5:5274", "{added}");
    // When the outside address is where the request came from and no home was seen, the
    // anywhere link names no home.
    let fresh = fam_with(|s| s.server.public_url = "https://music.example.com".into(), None).await;
    let added = alex_code_from(&fresh, "music.example.com").await;
    let anywhere = added["links"]["anywhere"].as_str().expect("a link");
    assert!(!anywhere.contains("home="), "{anywhere}");
}

#[tokio::test]
async fn subpath_public_url_links() {
    let f = fam_with(|s| s.server.public_url = "example.com/octo/".into(), None).await;
    let added = alex_code_from(&f, "192.168.1.5:5274").await;
    assert_eq!(added["server"], "https://example.com/octo");
    let code = added["pairCode"].as_str().expect("a code");
    let anywhere = added["links"]["anywhere"].as_str().expect("a link");
    assert!(
        anywhere.starts_with(&format!(
            "https://example.com/octo/family/join#u=alex&c={code}&home="
        )),
        "{anywhere}"
    );
    let invite = f
        .post("/api/family/members/alex/invite", Some(&f.sam()), json!({}))
        .await
        .json();
    let link = invite["inviteLink"].as_str().expect("a link");
    assert!(
        link.starts_with("https://example.com/octo/family/join#invite="),
        "{link}"
    );
    // An app password's server is the prefixed address too.
    let r = f
        .post(
            "/api/family/me/devices",
            Some(&f.alex()),
            json!({ "name": "", "kind": "SubsonicApp" }),
        )
        .await
        .json();
    assert_eq!(r["server"], "https://example.com/octo");
    assert_eq!(r["servers"]["anywhere"], "https://example.com/octo");
}

#[tokio::test]
async fn links_offer_anywhere_and_home() {
    let f = fam_with(|s| s.server.public_url = "https://music.example.com".into(), None).await;
    let added = alex_code_from(&f, "192.168.1.5:5274").await;
    assert_eq!(added["anywhereAvailable"], true, "{added}");
    assert_eq!(added["awayAllowed"], true);
    assert_eq!(added["homeOnly"], false);
    // The old single field is the default choice: anywhere.
    assert_eq!(added["server"], "https://music.example.com");
    assert_eq!(added["servers"]["anywhere"], "https://music.example.com");
    assert_eq!(added["servers"]["home"], "http://192.168.1.5:5274");
    // Both links carry the one code.
    let code = added["pairCode"].as_str().expect("a code");
    for which in ["anywhere", "home"] {
        let link = added["links"][which].as_str().expect("a link");
        assert!(link.contains(&format!("#u=alex&c={code}")), "{link}");
    }
    // An app password gives servers, not join links.
    let r = write_from(
        &f,
        Method::POST,
        "/api/family/me/devices",
        &f.alex(),
        json!({ "name": "", "kind": "SubsonicApp" }),
        "192.168.1.5:5274",
        &[],
        HOME,
    )
    .await
    .json();
    assert_eq!(r["links"], Value::Null);
    assert_eq!(r["servers"]["home"], "http://192.168.1.5:5274");
    // The apps' call answers the same shape.
    let r = f.rest_as_alex("addFamilyDevice", "kind=OctoApp").await.subsonic();
    let added = &r["familyDeviceAdded"];
    assert_eq!(added["anywhereAvailable"], true, "{r}");
    assert!(added["links"]["anywhere"].is_string(), "{r}");
    assert!(added["links"]["home"].is_string(), "{r}");
    // So does an invite.
    let invite = f
        .post("/api/family/members/alex/invite", Some(&f.sam()), json!({}))
        .await
        .json();
    assert_eq!(invite["anywhereAvailable"], true);
    assert_eq!(invite["inviteLink"], invite["links"]["anywhere"]);
    assert!(
        invite["links"]["home"]
            .as_str()
            .is_some_and(|l| l.starts_with("http://localhost/family/join#invite=")),
        "{invite}"
    );
}

#[tokio::test]
async fn anywhere_unavailable_without_public_url() {
    let f = fam().await;
    let added = alex_code_from(&f, "192.168.1.5:5274").await;
    assert_eq!(added["anywhereAvailable"], false);
    assert_eq!(added["awayAllowed"], true);
    assert_eq!(added["links"]["anywhere"], Value::Null);
    assert_eq!(added["servers"]["anywhere"], Value::Null);
    assert_eq!(added["server"], "http://192.168.1.5:5274");
    let invite = f
        .post("/api/family/members/alex/invite", Some(&f.sam()), json!({}))
        .await
        .json();
    assert_eq!(invite["anywhereAvailable"], false);
    assert_eq!(invite["links"]["anywhere"], Value::Null);
    assert_eq!(invite["inviteLink"], invite["links"]["home"]);
    let r = f.rest_as_alex("addFamilyDevice", "kind=OctoApp").await.subsonic();
    assert_eq!(r["familyDeviceAdded"]["anywhereAvailable"], false, "{r}");
}

#[tokio::test]
async fn member_without_away_defaults_home() {
    let f = fam_with(|s| s.server.public_url = "https://music.example.com".into(), None).await;
    let added = write_from(
        &f,
        Method::POST,
        "/api/family/members",
        &f.owner_cookie(),
        json!({ "username": "kim", "displayName": "Kim", "preset": "Member", "away": false }),
        "192.168.1.5:5274",
        &[],
        HOME,
    )
    .await;
    assert_eq!(added.status, StatusCode::OK, "{}", added.body);
    let added = added.json();
    assert_eq!(added["awayAllowed"], false);
    assert_eq!(added["anywhereAvailable"], false);
    assert_eq!(added["links"]["anywhere"], Value::Null);
    let link = added["inviteLink"].as_str().expect("a link");
    assert!(
        link.starts_with("http://192.168.1.5:5274/family/join#invite="),
        "{link}"
    );
    let kim = f.state.family_store.member("kim").expect("added");
    assert!(!kim.abilities().away);
    // A device a manager adds for kim defaults home too.
    let device = write_from(
        &f,
        Method::POST,
        "/api/family/members/kim/devices",
        &f.owner_cookie(),
        json!({ "name": "", "kind": "OctoApp" }),
        "192.168.1.5:5274",
        &[],
        HOME,
    )
    .await
    .json();
    assert_eq!(device["anywhereAvailable"], false, "{device}");
    assert_eq!(device["server"], "http://192.168.1.5:5274");
    // Left on (the default), a new member's links default to anywhere.
    let lee = f
        .post(
            "/api/family/members",
            Some(&f.owner_cookie()),
            json!({ "username": "lee", "displayName": "Lee", "preset": "Member" }),
        )
        .await
        .json();
    assert_eq!(lee["anywhereAvailable"], true, "{lee}");
    assert!(
        f.state
            .family_store
            .member("lee")
            .expect("added")
            .abilities()
            .away
    );
}

#[tokio::test]
async fn handoff_signs_the_owner_in_once() {
    let f = fam_with(|_| {}, Some("required")).await;
    let admin = format!("octo_browse={}", f.state.browse_sessions.create("winters"));
    let open = f.post("/api/admin/family/open", Some(&admin), json!({})).await;
    assert_eq!(open.status, StatusCode::OK, "{}", open.body);
    let token = open.json()["token"].as_str().expect("a token").to_string();
    let r = f
        .post("/api/family/handoff", None, json!({ "token": token }))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.json()["username"], "winters");
    let cookie = format!("octo_family={}", r.cookie_token());
    let session = f.get("/api/family/session", Some(&cookie)).await.json();
    assert_eq!(session["owner"], true, "{session}");
    assert_eq!(session["username"], "winters");
    // Once only.
    let again = f
        .post("/api/family/handoff", None, json!({ "token": token }))
        .await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    assert!(again.headers.get("set-cookie").is_none());
}

#[tokio::test]
async fn handoff_token_expires() {
    let f = fam().await;
    let token = f
        .state
        .family
        .mint_handoff("winters", chrono::TimeDelta::seconds(-1));
    let r = f
        .post("/api/family/handoff", None, json!({ "token": token }))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = f
        .post("/api/family/handoff", None, json!({ "token": "made-up" }))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // A live one works for 60 seconds.
    assert_eq!(HANDOFF_TTL, chrono::TimeDelta::seconds(60));
}

#[tokio::test]
async fn handoff_refused_for_non_admin_sessions() {
    let f = fam_with(|_| {}, Some("required")).await;
    assert_eq!(
        f.post("/api/admin/family/open", None, json!({})).await.status,
        StatusCode::UNAUTHORIZED
    );
    let member = format!(
        "octo_browse={}",
        f.state.browse_sessions.create_family("alex", SessionKind::Member)
    );
    let r = f.post("/api/admin/family/open", Some(&member), json!({})).await;
    assert_ne!(r.status, StatusCode::OK, "{}", r.body);
    // A Family page sign-in is not a dashboard session.
    let r = f
        .post("/api/admin/family/open", Some(&f.owner_cookie()), json!({}))
        .await;
    assert_ne!(r.status, StatusCode::OK, "{}", r.body);
    // With the dashboard's sign-in off nobody is known, so there is no one to hand off.
    let open = fam().await;
    let r = open.post("/api/admin/family/open", None, json!({})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.body);
}
