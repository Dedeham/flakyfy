//! The unsure songs behind the dashboard's sign-in: who may read and answer, which file and video
//! can be heard, and what an answer does to Review.

use axum::http::{Method, StatusCode};
use flakyfy_core::models::domain::Song;
use flakyfy_core::settings::{AppSettings, LibraryActionSettings, NoticeKind, SubsonicSettings};

use crate::app::AppState;
use crate::controllers::admin::test_app::{Reply, app, send};
use crate::http::pipeline::App;
use crate::services::library::unsure_songs::{HeardVideo, UnsureReason};

struct Bench {
    state: AppState,
    app: App,
    token: String,
    dir: tempfile::TempDir,
    id: u64,
}

fn bench_with(allowed: &[&str]) -> Bench {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::for_tests(AppSettings {
        library_actions: LibraryActionSettings {
            enabled: true,
            review_enabled: true,
            allowed_users: allowed.iter().map(|u| u.to_string()).collect(),
            ..Default::default()
        },
        subsonic: SubsonicSettings {
            auto_detect_download_path: false,
            ..Default::default()
        },
        ..Default::default()
    });
    let root = dir.path().join("music");
    std::fs::create_dir_all(&root).unwrap();
    state
        .settings
        .set_raw("Library:DownloadPath", Some(&root.to_string_lossy()));
    std::fs::write(root.join("a.flac"), b"0123456789").unwrap();
    let id = keep(&state, "a.flac");
    let token = state.browse_sessions.create("admin");
    let app = app(state.clone());
    Bench {
        state,
        app,
        token,
        dir,
        id,
    }
}

fn keep(state: &AppState, path: &str) -> u64 {
    let unsure = state.library_youtube_check.unsure();
    let video = HeardVideo {
        video_id: "known-video".into(),
        title: Some("Song".into()),
        artist: Some("Someone".into()),
        album: None,
        seconds: Some(190),
        bit_error: 0.31,
        offset_seconds: 2.0,
        trusted: false,
        reason: UnsureReason::Name,
        rejected: false,
    };
    let song = Song {
        artist: "Demo Artist".into(),
        title: "Song".into(),
        album: "Demo Album".into(),
        ..Default::default()
    };
    unsure.record(path, &song, "1:1", video, chrono::Utc::now());
    unsure.song_at(path).unwrap().id
}

impl Bench {
    async fn get(&self, uri: &str) -> Reply {
        send(
            &self.app,
            Method::GET,
            uri,
            &[("X-Octo-Browse-Token", &self.token)],
            None,
        )
        .await
    }

    async fn get_range(&self, uri: &str, range: &str) -> Reply {
        send(
            &self.app,
            Method::GET,
            uri,
            &[("X-Octo-Browse-Token", &self.token), ("Range", range)],
            None,
        )
        .await
    }

    /// A write as the Navidrome admin: answers need the sign-in cookie, not just a browse token.
    async fn post(&self, uri: &str, json: &str) -> Reply {
        let cookie = format!("octo_browse={}", self.token);
        send(
            &self.app,
            Method::POST,
            uri,
            &[("X-Octo-Admin", "1"), ("Cookie", &cookie)],
            Some(json),
        )
        .await
    }

    fn decide_uri(&self) -> String {
        format!("/api/admin/youtube-check/unsure/{}/decide", self.id)
    }
}

const LIST: &str = "/api/admin/youtube-check/unsure";

#[tokio::test]
async fn the_list_needs_a_sign_in_and_names_the_song() {
    let bench = bench_with(&["admin"]);

    let refused = send(&bench.app, Method::GET, LIST, &[], None).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);

    let reply = bench.get(LIST).await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["songs"][0]["title"], "Song");
    assert_eq!(body["songs"][0]["videos"][0]["videoId"], "known-video");
    assert_eq!(body["suggestions"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn an_answer_needs_the_admin_header() {
    let bench = bench_with(&["admin"]);
    let reply = send(
        &bench.app,
        Method::POST,
        &bench.decide_uri(),
        &[("X-Octo-Browse-Token", &bench.token)],
        Some(r#"{"label":"right"}"#),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(
        bench
            .state
            .library_youtube_check
            .unsure()
            .song(bench.id)
            .unwrap()
            .decision
            .is_none()
    );
}

#[tokio::test]
async fn nobody_on_the_allowlist_means_nobody_answers() {
    let bench = bench_with(&[]);
    let reply = bench.post(&bench.decide_uri(), r#"{"label":"right"}"#).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(
        bench
            .state
            .library_youtube_check
            .unsure()
            .song(bench.id)
            .unwrap()
            .decision
            .is_none()
    );
}

#[tokio::test]
async fn an_answer_flakyfy_does_not_know_is_refused() {
    let bench = bench_with(&["admin"]);
    let reply = bench.post(&bench.decide_uri(), r#"{"label":"maybe"}"#).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_unknown_song_is_not_found() {
    let bench = bench_with(&["admin"]);
    let reply = bench
        .post(
            "/api/admin/youtube-check/unsure/999/decide",
            r#"{"label":"right"}"#,
        )
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    let reply = bench.get("/api/admin/youtube-check/unsure/999/audio").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_answer_of_wrong_only_raises_a_question_and_the_file_stays() {
    let bench = bench_with(&["admin"]);
    let reply = bench.post(&bench.decide_uri(), r#"{"label":"wrong"}"#).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());

    let entries = bench.state.notice_queue.for_user("admin", NoticeKind::Review);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_open());
    assert!(entries[0].reason.contains("You heard"));
    assert_eq!(
        std::fs::read(bench.dir.path().join("music/a.flac")).unwrap(),
        b"0123456789"
    );
}

#[tokio::test]
async fn an_answer_of_right_closes_an_open_question_from_a_library_check() {
    let bench = bench_with(&["admin"]);
    let full = bench.state.library_youtube_check.full_path_of("a.flac");
    let song = Song {
        artist: "Demo Artist".into(),
        title: "Song".into(),
        ..Default::default()
    };
    assert!(
        bench
            .state
            .notice_queue
            .add_library_question("admin", &full, &song, "asked")
    );

    let reply = bench.post(&bench.decide_uri(), r#"{"label":"right"}"#).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());

    let entries = bench.state.notice_queue.for_user("admin", NoticeKind::Review);
    assert!(!entries[0].is_open());
}

#[tokio::test]
async fn the_file_can_be_heard_with_a_range() {
    let bench = bench_with(&["admin"]);
    let uri = format!("/api/admin/youtube-check/unsure/{}/audio", bench.id);

    let whole = bench.get(&uri).await;
    assert_eq!(whole.status, StatusCode::OK);
    assert_eq!(whole.body, b"0123456789");
    assert_eq!(whole.header("content-type"), Some("audio/flac"));

    let part = bench.get_range(&uri, "bytes=2-4").await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.body, b"234");
    assert_eq!(part.header("content-range"), Some("bytes 2-4/10"));

    let refused = send(&bench.app, Method::GET, &uri, &[], None).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_path_outside_the_music_folder_is_never_served() {
    let bench = bench_with(&["admin"]);
    std::fs::write(bench.dir.path().join("secret.flac"), b"secret").unwrap();
    let id = keep(&bench.state, "../secret.flac");

    let reply = bench
        .get(&format!("/api/admin/youtube-check/unsure/{id}/audio"))
        .await;

    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_ne!(reply.body, b"secret");
}

#[tokio::test]
async fn only_the_song_s_own_videos_can_be_heard() {
    let bench = bench_with(&["admin"]);
    let reply = bench
        .get(&format!(
            "/api/admin/youtube-check/unsure/{}/preview?video=someone-else",
            bench.id
        ))
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_suggestion_nobody_made_cannot_be_accepted() {
    let bench = bench_with(&["admin"]);
    let reply = bench
        .post(
            "/api/admin/youtube-check/unsure/rules/accept",
            r#"{"id":"name|search|30|settle"}"#,
        )
        .await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn an_answer_can_be_taken_back_with_no_label() {
    let bench = bench_with(&["admin"]);
    bench.post(&bench.decide_uri(), r#"{"label":"version"}"#).await;

    let reply = bench
        .post(&bench.decide_uri(), r#"{"label":null,"videoId":null}"#)
        .await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert!(
        bench
            .state
            .library_youtube_check
            .unsure()
            .song(bench.id)
            .unwrap()
            .decision
            .is_none()
    );
}
