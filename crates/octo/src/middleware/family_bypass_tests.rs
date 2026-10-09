//! Every way a member could get audio past their family rules, or change what they may not,
//! tried through the whole pipeline against a Navidrome stood in for by wiremock. Each test
//! names the path it closes; the last ones show that the owner, and everyone with Family off,
//! get exactly what they had before.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::http::{Method, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use http_body_util::BodyExt;
use octo_core::family::abilities::Preset;
use octo_core::family::device::{Device, DeviceKind, NetworkPlace};
use octo_core::family::quality::{DeviceQualityMode, QualityPreference, StreamQuality};
use octo_core::models::domain::Song;
use octo_core::settings::{AppSettings, FamilySettings, SubsonicSettings};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::app::{AppState, TestServices};
use crate::http::pipeline::{App, build_with};
use crate::services::family::member_credentials::tests::member;

const APP_SECRET: &str = "app-secret-1";
const APP_PASSWORD: &str = "ABCD-EFGH-JKMN-PQRS";
const HOME: &str = "192.168.1.5";
const AWAY: &str = "203.0.113.9";
const LIBRARY_AUDIO: &str = "library-audio-bytes";

struct Family {
    server: MockServer,
    state: AppState,
    app: App,
    dir: tempfile::TempDir,
}

/// What Alex can do: a preset and edits over it.
struct Alex {
    preset: Preset,
    edits: Value,
}

impl Alex {
    fn listener() -> Self {
        Alex {
            preset: Preset::Listener,
            edits: json!({}),
        }
    }

    fn with(edits: Value) -> Self {
        Alex {
            preset: Preset::Listener,
            edits,
        }
    }
}

/// The owner `winters` and Alex (an Octo app `d_app` and a Subsonic app `d_sub`), over a
/// Navidrome that answers every call.
async fn family(enabled: bool, alex: Alex, configure: impl FnOnce(&mut AppSettings)) -> Family {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut settings = AppSettings {
        subsonic: SubsonicSettings {
            url: Some(server.uri()),
            admin_username: Some("admin".into()),
            admin_password: Some("pw".into()),
            auto_detect_download_path: false,
            ..Default::default()
        },
        family: FamilySettings {
            enabled,
            members_path: dir.path().join("members").to_string_lossy().into_owned(),
            ..Default::default()
        },
        ..Default::default()
    };
    configure(&mut settings);
    let state = AppState::for_tests_with(
        settings,
        TestServices {
            config_dir: Some(dir.path().join("config")),
            ..Default::default()
        },
    );
    assert!(state.family_vault.create_key_if_missing().expect("a key"));
    let store = &state.family_store;
    store.set_owner("winters");
    let mut member_alex = member("alex", state.family_vault.seal("hidden-1").expect("sealed"));
    member_alex.preset = alex.preset;
    if let Value::Object(edits) = alex.edits {
        member_alex.edits = edits;
    }
    store.upsert_member(member_alex);
    let mut sam = member("sam", state.family_vault.seal("hidden-2").expect("sealed"));
    sam.personal_library_id = Some(7);
    store.upsert_member(sam);
    for (id, kind, secret, name) in [
        ("d_app", DeviceKind::OctoApp, APP_SECRET, "Alex's phone"),
        ("d_sub", DeviceKind::SubsonicApp, APP_PASSWORD, "Pixel 9"),
    ] {
        store.add_device(Device {
            id: id.into(),
            username: "alex".into(),
            name: name.into(),
            kind,
            secret: state.family_vault.seal(secret).expect("sealed"),
            created: Utc::now(),
            last_seen: None,
            app: String::new(),
            place: NetworkPlace::Home,
            seen_apps: Vec::new(),
            navidrome_jwt: None,
            quality: Default::default(),
        });
    }
    navidrome(&server).await;
    let app = build_with(state.clone(), crate::controllers::subsonic::routes());
    Family {
        server,
        state,
        app,
        dir,
    }
}

/// Navidrome: the admin sign-in, audio for `stream` and `download`, the extension list with
/// `transcoding`, a transcode decision that plays the file as it is, and ok for the rest.
async fn navidrome(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "admin-jwt", "isAdmin": true, "username": "admin",
            "subsonicToken": "tok", "subsonicSalt": "salt"
        })))
        .mount(server)
        .await;
    for endpoint in ["/rest/stream", "/rest/download"] {
        Mock::given(method("GET"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_raw(LIBRARY_AUDIO, "audio/mpeg"))
            .with_priority(1)
            .mount(server)
            .await;
    }
    Mock::given(path("/rest/getOpenSubsonicExtensions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1","openSubsonicExtensions":[{"name":"transcoding","versions":[1]},{"name":"formPost","versions":[1]}]}}"#,
            "application/json",
        ))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(path_regex("^/rest/getTranscode"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"subsonic-response":{"status":"ok","transcodeDecision":{"canDirectPlay":true}}}"#,
            "application/json",
        ))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(path_regex("^/(rest|api)/"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#,
            "application/json",
        ))
        .with_priority(10)
        .mount(server)
        .await;
}

impl Family {
    /// What Navidrome was asked, for one path.
    async fn asked(&self, at: &str) -> Vec<wiremock::Request> {
        self.server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.url.path().eq_ignore_ascii_case(at))
            .collect()
    }

    /// The query Navidrome got for the one `stream` it was asked for.
    async fn streamed(&self) -> Vec<(String, String)> {
        let asked = self.asked("/rest/stream").await;
        assert_eq!(asked.len(), 1, "one stream relayed");
        pairs(asked[0].url.query().unwrap_or(""))
    }

    /// The library song `lib-9` at `<dir>/music/A/B/song.flac` (or in Sam's own library),
    /// read into the library index.
    async fn index_song(&self, in_sams_library: bool) -> String {
        let (library_id, library_path) = if in_sams_library {
            (7, self.dir.path().join("members").join("sam"))
        } else {
            (1, self.dir.path().join("music"))
        };
        let file = library_path.join("A").join("B").join("song.flac");
        std::fs::create_dir_all(file.parent().expect("a folder")).expect("folders");
        std::fs::write(&file, b"the-original-flac-file").expect("the file");
        Mock::given(method("GET"))
            .and(path("/api/song"))
            .and(query_param("_start", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": "lib-9", "albumId": "al-9", "artistId": "ar-9", "libraryId": library_id,
                "libraryPath": library_path.to_string_lossy(), "path": "A/B/song.flac",
                "explicitStatus": "", "size": 22, "title": "Song"
            }])))
            .with_priority(1)
            .mount(&self.server)
            .await;
        assert_eq!(self.state.library_index.refresh().await.expect("read"), 1);
        file.to_string_lossy().into_owned()
    }
}

struct Reply {
    status: StatusCode,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.body))
    }

    fn error_code(&self) -> Option<i64> {
        self.json()["subsonic-response"]["error"]["code"].as_i64()
    }
}

fn request(method: Method, uri: &str, headers: &[(&str, &str)], peer: &str) -> Request {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut req = req.body(Body::empty()).expect("request");
    let address: SocketAddr = format!("{peer}:50000").parse().expect("an address");
    req.extensions_mut().insert(ConnectInfo(address));
    req
}

async fn send(f: &Family, method: Method, uri: &str, headers: &[(&str, &str)], peer: &str) -> Reply {
    let res = f
        .app
        .clone()
        .oneshot(request(method, uri, headers, peer))
        .await
        .expect("infallible");
    let status = res.status();
    let body = res.into_body().collect().await.expect("body").to_bytes();
    Reply {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

async fn get(f: &Family, uri: &str, peer: &str) -> Reply {
    send(f, Method::GET, uri, &[], peer).await
}

fn pairs(query: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

const ALEX: &str = "u=alex&p=app-secret-1&v=1.16.1&c=Octo&f=json";
const ALEX_SUB: &str = "u=alex&p=ABCD-EFGH-JKMN-PQRS&v=1.16.1&c=Symfonium&f=json";
const OWNER: &str = "u=winters&t=abc&s=def&v=1.16.1&c=Feishin&f=json";

/// A Navidrome browser token for `sub`, as the web player would carry it.
fn navidrome_jwt(sub: &str) -> String {
    let payload = json!({ "iss": "ND", "sub": sub, "exp": Utc::now().timestamp() + 3600 });
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(payload.to_string()),
        URL_SAFE_NO_PAD.encode("navidrome-signature")
    )
}

fn capped() -> Alex {
    Alex::with(json!({ "streamCap": 192 }))
}

// ---------------------------------------------------------------------------------------
// The paths
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn transcode_decision_and_stream_are_refused_with_a_ceiling() {
    let f = family(true, capped(), |_| {}).await;
    for endpoint in ["getTranscodeDecision", "getTranscodeStream"] {
        let r = get(
            &f,
            &format!("/rest/{endpoint}?{ALEX}&mediaId=lib-1&mediaType=song"),
            HOME,
        )
        .await;
        assert_eq!(r.error_code(), Some(0), "{endpoint}: {}", r.body);
    }
    assert!(f.asked("/rest/getTranscodeDecision.view").await.is_empty());
    assert!(f.asked("/rest/getTranscodeStream").await.is_empty());
    // The extension list does not offer it either, so a client goes to `stream`.
    let r = get(&f, &format!("/rest/getOpenSubsonicExtensions?{ALEX}"), HOME).await;
    assert!(!r.body.contains("\"transcoding\""), "{}", r.body);
    assert!(r.body.contains("formPost"), "{}", r.body);
}

#[tokio::test]
async fn transcode_decisions_stay_for_a_member_without_a_ceiling() {
    let f = family(true, Alex::listener(), |_| {}).await;
    let r = get(
        &f,
        &format!("/rest/getTranscodeDecision?{ALEX}&mediaId=lib-1"),
        HOME,
    )
    .await;
    assert_eq!(
        r.json()["subsonic-response"]["transcodeDecision"]["canDirectPlay"],
        true
    );
    let r = get(&f, &format!("/rest/getOpenSubsonicExtensions?{ALEX}"), HOME).await;
    assert!(r.body.contains("\"transcoding\""), "{}", r.body);
}

#[tokio::test]
async fn local_file_shortcut_relays_the_library_copy_with_the_ceiling() {
    let f = family(true, capped(), |s| s.subsonic.wait_for_lossless_on_play = true).await;
    let file = f.index_song(false).await;
    let song = Song {
        title: "Song".into(),
        artist: "Artist".into(),
        external_provider: Some("deezer".into()),
        external_id: Some("1".into()),
        ..Default::default()
    };
    f.state
        .local_library
        .register_downloaded_song(&song, &file)
        .await
        .expect("mapped");
    let r = get(
        &f,
        &format!("/rest/stream?{ALEX}&id=ext-deezer-song-1&format=raw"),
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, LIBRARY_AUDIO, "the library copy, never the file");
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "id"), Some("lib-9"));
    assert_eq!(value(&sent, "maxBitRate"), Some("192"));
    assert_eq!(value(&sent, "format"), Some("opus"));
}

#[tokio::test]
async fn serve_acquired_relays_the_library_copy_with_the_ceiling() {
    // Only a member who adds songs at once has a play keep a copy, so only their play joins
    // the acquisition.
    let direct = Alex::with(json!({ "addToLibrary": "Direct", "streamCap": 192 }));
    let f = family(true, direct, |s| s.subsonic.wait_for_lossless_on_play = true).await;
    let file = f.index_song(false).await;
    // The acquisition this play joins has already landed the file.
    let landed = f
        .state
        .track_acquisition_queue
        .enqueue("deezer", "1", false, false, true, None, true, None, false, None);
    assert!(landed.try_set_result(file));
    let r = get(&f, &format!("/rest/stream?{ALEX}&id=ext-deezer-song-1"), HOME).await;
    assert_eq!(r.body, LIBRARY_AUDIO, "the library copy, never the file");
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "id"), Some("lib-9"));
    assert_eq!(value(&sent, "maxBitRate"), Some("192"));
}

#[tokio::test]
async fn a_capped_listener_plays_from_the_source_and_never_gets_the_file() {
    let f = family(true, capped(), |s| s.subsonic.wait_for_lossless_on_play = true).await;
    let file = f.index_song(false).await;
    let landed = f
        .state
        .track_acquisition_queue
        .enqueue("deezer", "1", false, false, true, None, true, None, false, None);
    assert!(landed.try_set_result(file));
    let r = get(&f, &format!("/rest/stream?{ALEX}&id=ext-deezer-song-1"), HOME).await;
    assert!(!r.body.contains("the-original-flac-file"), "{}", r.body);
}

#[tokio::test]
async fn a_file_in_another_members_library_is_never_served() {
    // The owner has no ceiling, so only the library a file is in keeps it back.
    let f = family(true, Alex::listener(), |s| {
        s.subsonic.wait_for_lossless_on_play = true
    })
    .await;
    let file = f.index_song(true).await;
    let song = Song {
        external_provider: Some("deezer".into()),
        external_id: Some("1".into()),
        ..Default::default()
    };
    f.state
        .local_library
        .register_downloaded_song(&song, &file)
        .await
        .expect("mapped");
    let landed = f
        .state
        .track_acquisition_queue
        .enqueue("deezer", "1", false, false, true, None, true, None, false, None);
    assert!(landed.try_set_result(file));
    for who in [OWNER, ALEX] {
        let r = get(&f, &format!("/rest/stream?{who}&id=ext-deezer-song-1"), HOME).await;
        assert!(!r.body.contains("the-original-flac-file"), "{who}: {}", r.body);
    }
}

#[tokio::test]
async fn download_is_refused_when_downloads_are_off() {
    let f = family(true, Alex::listener(), |_| {}).await;
    let r = get(&f, &format!("/rest/download?{ALEX}&id=lib-1"), HOME).await;
    assert_eq!(r.error_code(), Some(50));
    assert!(r.body.contains("Downloads are off"), "{}", r.body);
    assert!(f.asked("/rest/download").await.is_empty());
}

#[tokio::test]
async fn an_allowed_download_takes_the_ceiling() {
    let f = family(
        true,
        Alex::with(json!({ "downloadFiles": true, "streamCap": 192 })),
        |_| {},
    )
    .await;
    let r = get(&f, &format!("/rest/download?{ALEX}&id=lib-1"), HOME).await;
    assert_eq!(r.body, LIBRARY_AUDIO);
    let asked = f.asked("/rest/download").await;
    let sent = pairs(asked[0].url.query().unwrap_or(""));
    assert_eq!(value(&sent, "format"), Some("opus"));
    assert_eq!(value(&sent, "bitrate"), Some("192"));
}

#[tokio::test]
async fn create_share_is_refused_when_shares_are_off() {
    let f = family(true, Alex::listener(), |_| {}).await;
    for endpoint in ["createShare", "updateShare"] {
        let r = get(&f, &format!("/rest/{endpoint}?{ALEX}&id=lib-1"), HOME).await;
        assert_eq!(r.error_code(), Some(50), "{endpoint}");
        assert!(f.asked(&format!("/rest/{endpoint}")).await.is_empty());
    }
    // A share write through Navidrome's own API is refused too.
    let jwt = navidrome_jwt("alex");
    let r = send(
        &f,
        Method::POST,
        "/api/share",
        &[("X-Nd-Authorization", &format!("Bearer {jwt}"))],
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn native_player_write_is_refused() {
    let f = family(true, Alex::listener(), |_| {}).await;
    let jwt = format!("Bearer {}", navidrome_jwt("alex"));
    let r = send(
        &f,
        Method::PUT,
        "/api/player/p1",
        &[("X-Nd-Authorization", &jwt)],
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = send(
        &f,
        Method::PUT,
        "/api/user/u-alex",
        &[("X-Nd-Authorization", &jwt)],
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.body.contains("Change this in Octo"), "{}", r.body);
    assert!(f.asked("/api/player/p1").await.is_empty());
    assert!(f.asked("/api/user/u-alex").await.is_empty());
    // Reading is fine.
    let r = send(
        &f,
        Method::GET,
        "/api/player",
        &[("X-Nd-Authorization", &jwt)],
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn unknown_rest_endpoint_is_refused_for_strict_members() {
    let f = family(true, capped(), |_| {}).await;
    let r = get(&f, &format!("/rest/getSomethingNew?{ALEX}"), HOME).await;
    assert_eq!(r.error_code(), Some(0));
    assert!(f.asked("/rest/getSomethingNew").await.is_empty());
    // Changing the account's password in Navidrome would break the family's sign-in.
    let r = get(
        &f,
        &format!("/rest/changePassword?{ALEX}&username=alex&password=x"),
        HOME,
    )
    .await;
    assert_eq!(r.error_code(), Some(50));
    // A kid is strict too; a member with no limit is not.
    let kid = family(
        true,
        Alex {
            preset: Preset::Kid,
            edits: json!({}),
        },
        |_| {},
    )
    .await;
    let r = get(&kid, &format!("/rest/getSomethingNew?{ALEX}"), HOME).await;
    assert_eq!(r.error_code(), Some(0));
    let open = family(true, Alex::listener(), |_| {}).await;
    let r = get(&open, &format!("/rest/getSomethingNew?{ALEX}"), HOME).await;
    assert_eq!(r.json()["subsonic-response"]["status"], "ok");
    assert_eq!(open.asked("/rest/getSomethingNew").await.len(), 1);
}

#[tokio::test]
async fn quickconnect_authorize_is_refused() {
    let f = family(true, Alex::listener(), |_| {}).await;
    let jwt = format!("Bearer {}", navidrome_jwt("alex"));
    let r = send(
        &f,
        Method::POST,
        "/api/quickconnect/authorize?code=123456",
        &[("X-Nd-Authorization", &jwt)],
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(f.asked("/api/quickconnect/authorize").await.is_empty());
}

#[tokio::test]
async fn radio_away_off_refuses_the_stream() {
    let f = family(true, Alex::with(json!({ "away": false })), |_| {}).await;
    let token = f
        .state
        .last_fm_radio_stream_sessions
        .issue("alex", "station-1", [("u", "alex")], None);
    let r = get(&f, &format!("/radio/stream/{token}"), AWAY).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.body.contains("away from home"), "{}", r.body);
    // At home the stream goes on to the radio (which has no such station here).
    let r = get(&f, &format!("/radio/stream/{token}"), HOME).await;
    assert_ne!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn away_off_refuses_a_stream() {
    let f = family(true, Alex::with(json!({ "away": false })), |_| {}).await;
    let r = get(&f, &format!("/rest/stream?{ALEX}&id=lib-1"), AWAY).await;
    assert_eq!(r.error_code(), Some(50));
    assert!(f.asked("/rest/stream").await.is_empty());
    let r = get(&f, &format!("/rest/stream?{ALEX}&id=lib-1"), HOME).await;
    assert_eq!(r.body, LIBRARY_AUDIO);
}

#[tokio::test]
async fn kid_explicit_stream_is_refused() {
    let f = family(
        true,
        Alex {
            preset: Preset::Kid,
            edits: json!({}),
        },
        |_| {},
    )
    .await;
    Mock::given(path("/rest/getSong"))
        .and(query_param("id", "lib-x"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "subsonic-response": {"status": "ok", "song": {"id": "lib-x", "explicitStatus": "explicit", "bitRate": 320}}
        })))
        .with_priority(1)
        .mount(&f.server)
        .await;
    let r = get(&f, &format!("/rest/stream?{ALEX}&id=lib-x"), HOME).await;
    assert_eq!(r.error_code(), Some(50));
    assert!(r.body.contains("Not available on this profile"), "{}", r.body);
    assert!(f.asked("/rest/stream").await.is_empty());
    // Asked once, then known for an hour.
    let _ = get(&f, &format!("/rest/stream?{ALEX}&id=lib-x"), HOME).await;
    assert_eq!(f.asked("/rest/getSong").await.len(), 1);
    // A song with no mark plays.
    let r = get(&f, &format!("/rest/stream?{ALEX}&id=lib-1"), HOME).await;
    assert_eq!(r.body, LIBRARY_AUDIO);
}

#[tokio::test]
async fn owner_is_unaffected() {
    let f = family(true, capped(), |_| {}).await;
    let r = get(&f, &format!("/rest/stream?{OWNER}&id=lib-1&format=raw"), AWAY).await;
    assert_eq!(r.body, LIBRARY_AUDIO);
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "format"), Some("raw"));
    assert_eq!(value(&sent, "maxBitRate"), None);
    let r = get(
        &f,
        &format!("/rest/getTranscodeDecision?{OWNER}&mediaId=lib-1"),
        HOME,
    )
    .await;
    assert_eq!(
        r.json()["subsonic-response"]["transcodeDecision"]["canDirectPlay"],
        true
    );
    let r = get(&f, &format!("/rest/download?{OWNER}&id=lib-1"), HOME).await;
    assert_eq!(r.body, LIBRARY_AUDIO);
    let r = get(&f, &format!("/rest/getSomethingNew?{OWNER}"), HOME).await;
    assert_eq!(r.json()["subsonic-response"]["status"], "ok");
    let jwt = format!("Bearer {}", navidrome_jwt("winters"));
    let r = send(
        &f,
        Method::PUT,
        "/api/player/p1",
        &[("X-Nd-Authorization", &jwt)],
        HOME,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = get(&f, &format!("/rest/getOpenSubsonicExtensions?{OWNER}"), HOME).await;
    assert!(r.body.contains("\"transcoding\""), "{}", r.body);
}

#[tokio::test]
async fn family_off_is_unaffected() {
    let f = family(false, capped(), |_| {}).await;
    f.state.family_store.set_preference(
        "winters",
        QualityPreference {
            home: StreamQuality::DataSaver,
            away: StreamQuality::DataSaver,
        },
    );
    let query = format!("{ALEX}&id=lib-1&format=raw");
    let r = get(&f, &format!("/rest/stream?{query}"), AWAY).await;
    assert_eq!(r.body, LIBRARY_AUDIO);
    let asked = f.asked("/rest/stream").await;
    let sent = pairs(asked[0].url.query().unwrap_or(""));
    assert_eq!(value(&sent, "format"), Some("raw"));
    assert_eq!(value(&sent, "maxBitRate"), None);
    let r = get(
        &f,
        &format!("/rest/getTranscodeDecision?{ALEX}&mediaId=lib-1"),
        HOME,
    )
    .await;
    assert_eq!(
        r.json()["subsonic-response"]["transcodeDecision"]["canDirectPlay"],
        true
    );
    let r = get(&f, &format!("/rest/getSomethingNew?{ALEX}"), HOME).await;
    assert_eq!(r.json()["subsonic-response"]["status"], "ok");
    assert!(f.state.family_audio.tracker().is_empty());
}

// ---------------------------------------------------------------------------------------
// Quality
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn account_away_preference_caps_raw() {
    let f = family(true, Alex::listener(), |_| {}).await;
    f.state.family_store.set_preference(
        "alex",
        QualityPreference {
            home: StreamQuality::Original,
            away: StreamQuality::Standard,
        },
    );
    let r = get(&f, &format!("/rest/stream?{ALEX}&id=lib-1&format=raw"), AWAY).await;
    assert_eq!(r.body, LIBRARY_AUDIO);
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "format"), Some("opus"));
    assert_eq!(value(&sent, "maxBitRate"), Some("160"));
    assert_eq!(
        sent.iter().filter(|(k, _)| k == "format").count(),
        1,
        "raw is gone: {sent:?}"
    );
}

#[tokio::test]
async fn home_original_preference_keeps_raw() {
    let f = family(true, Alex::listener(), |_| {}).await;
    f.state.family_store.set_preference(
        "alex",
        QualityPreference {
            home: StreamQuality::Original,
            away: StreamQuality::Standard,
        },
    );
    let _ = get(&f, &format!("/rest/stream?{ALEX}&id=lib-1&format=raw"), HOME).await;
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "format"), Some("raw"));
    assert_eq!(value(&sent, "maxBitRate"), None);
}

#[tokio::test]
async fn app_mode_ignores_account_preference() {
    let f = family(true, Alex::listener(), |_| {}).await;
    f.state.family_store.set_preference(
        "alex",
        QualityPreference {
            home: StreamQuality::DataSaver,
            away: StreamQuality::DataSaver,
        },
    );
    assert!(
        f.state
            .family_store
            .set_device_quality("d_app", DeviceQualityMode::App)
    );
    let _ = get(&f, &format!("/rest/stream?{ALEX}&id=lib-1&format=raw"), AWAY).await;
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "format"), Some("raw"));
    assert_eq!(value(&sent, "maxBitRate"), None);
}

#[tokio::test]
async fn family_limit_wins_over_original_preference() {
    let f = family(true, capped(), |_| {}).await;
    let _ = get(
        &f,
        &format!("/rest/stream?{ALEX}&id=lib-1&format=raw&maxBitRate=320"),
        HOME,
    )
    .await;
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "format"), Some("opus"));
    assert_eq!(value(&sent, "maxBitRate"), Some("192"));
}

#[tokio::test]
async fn client_lower_request_wins() {
    let f = family(true, capped(), |_| {}).await;
    let _ = get(
        &f,
        &format!("/rest/stream?{ALEX}&id=lib-1&format=mp3&maxBitRate=96"),
        HOME,
    )
    .await;
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "format"), Some("mp3"));
    assert_eq!(value(&sent, "maxBitRate"), Some("96"));
}

#[tokio::test]
async fn owner_preference_applies() {
    let f = family(true, Alex::listener(), |_| {}).await;
    f.state.family_store.set_preference(
        "winters",
        QualityPreference {
            home: StreamQuality::High,
            away: StreamQuality::Original,
        },
    );
    let _ = get(&f, &format!("/rest/stream?{OWNER}&id=lib-1"), HOME).await;
    let sent = f.streamed().await;
    assert_eq!(value(&sent, "format"), Some("opus"));
    assert_eq!(value(&sent, "maxBitRate"), Some("256"));
    // The owner's own ceiling also keeps transcode decisions from them.
    let r = get(
        &f,
        &format!("/rest/getTranscodeDecision?{OWNER}&mediaId=lib-1"),
        HOME,
    )
    .await;
    assert_eq!(r.error_code(), Some(0));
}

#[tokio::test]
async fn family_off_never_rewrites_quality() {
    let f = family(false, Alex::listener(), |_| {}).await;
    f.state.family_store.set_preference(
        "winters",
        QualityPreference {
            home: StreamQuality::DataSaver,
            away: StreamQuality::DataSaver,
        },
    );
    let query = format!("{OWNER}&id=lib-1&format=raw");
    let _ = get(&f, &format!("/rest/stream?{query}"), HOME).await;
    let asked = f.asked("/rest/stream").await;
    assert_eq!(asked[0].url.query(), Some(query.as_str()));
}

// ---------------------------------------------------------------------------------------
// Devices at once
// ---------------------------------------------------------------------------------------

/// A stream that stays open (its body unread past the first chunk) while the test runs.
async fn open_stream(f: &Family, query: &str, headers: &[(&str, &str)]) -> Body {
    let res = f
        .app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/rest/stream?{query}"),
            headers,
            HOME,
        ))
        .await
        .expect("infallible");
    let mut body = res.into_body();
    let first = body.frame().await.expect("a chunk").expect("ok");
    assert_eq!(first.into_data().expect("data"), LIBRARY_AUDIO.as_bytes());
    body
}

#[tokio::test]
async fn a_stream_through_the_gate_is_tracked() {
    let f = family(true, Alex::listener(), |_| {}).await;
    let body = open_stream(&f, &format!("{ALEX}&id=lib-1"), &[]).await;
    let live = f.state.family_audio.tracker().live();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].username, "alex");
    assert_eq!(live[0].device_id.as_deref(), Some("d_app"));
    assert_eq!(live[0].song_id, "lib-1");
    assert_eq!(live[0].bytes, LIBRARY_AUDIO.len() as u64);
    drop(body);
    assert!(f.state.family_audio.tracker().is_empty());
}

#[tokio::test]
async fn the_newest_device_wins_and_the_other_is_told() {
    let f = family(true, Alex::with(json!({ "devicesAtOnce": 1 })), |_| {}).await;
    let mut phone = open_stream(&f, &format!("{ALEX}&id=lib-1"), &[]).await;
    let _pixel = open_stream(
        &f,
        &format!("{ALEX_SUB}&id=lib-2"),
        &[("X-Octo-Device-Name", "Pixel 9")],
    )
    .await;
    // The phone's stream was ended.
    let ended = tokio::time::timeout(Duration::from_secs(2), phone.frame())
        .await
        .expect("ends at once");
    assert!(
        matches!(ended, Some(Err(_))),
        "the phone's body ends with an error"
    );
    // And while the Pixel plays, the phone is told where the music is now.
    let r = get(&f, &format!("/rest/stream?{ALEX}&id=lib-1"), HOME).await;
    assert_eq!(r.error_code(), Some(50));
    assert!(r.body.contains("Playing on Pixel 9 now"), "{}", r.body);
}

#[tokio::test]
async fn offline_purpose_never_counts() {
    let f = family(true, Alex::with(json!({ "devicesAtOnce": 1 })), |_| {}).await;
    let _phone = open_stream(&f, &format!("{ALEX}&id=lib-1"), &[]).await;
    let _copy = open_stream(
        &f,
        &format!("{ALEX_SUB}&id=lib-2"),
        &[("X-Octo-Purpose", "offline")],
    )
    .await;
    // The phone keeps playing next to the copy.
    let live = f.state.family_audio.tracker().live();
    let devices: Vec<Option<&str>> = live.iter().map(|s| s.device_id.as_deref()).collect();
    assert_eq!(devices, [Some("d_app"), Some("d_sub")]);
    assert_eq!(
        f.state.family_audio.tracker().bumped_by("alex", Some("d_app")),
        None
    );
    // Without offline copies the copy is refused.
    let f = family(true, Alex::with(json!({ "offlineCopies": false })), |_| {}).await;
    let r = send(
        &f,
        Method::GET,
        &format!("/rest/stream?{ALEX}&id=lib-1"),
        &[("X-Octo-Purpose", "offline")],
        HOME,
    )
    .await;
    assert_eq!(r.error_code(), Some(50));
}

#[tokio::test]
async fn radio_streams_count_on_the_device_that_opened_them() {
    let f = family(true, Alex::listener(), |_| {}).await;
    let token = "r".repeat(48);
    let answer =
        format!(r#"{{"internetRadioStation":[{{"streamUrl":"http://octo/radio/stream/{token}"}}]}}"#);
    let mut caller = crate::services::family::FamilyCaller::member(
        "alex",
        Preset::Listener,
        octo_core::family::abilities::Abilities::preset(Preset::Listener),
    );
    caller.device_id = Some("d_sub".into());
    f.state
        .family_audio
        .note_radio_sessions(answer.as_bytes(), &caller);
    assert_eq!(
        f.state.family_audio.radio_opener(&token).as_deref(),
        Some("d_sub")
    );
    assert!(f.state.family_audio.radio_refusal("alex", &token).is_none());
}
