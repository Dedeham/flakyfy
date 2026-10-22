//! The YouTube check's list through the assembled app: refused without a Navidrome admin
//! sign-in, empty before a check has heard anything, and the picks it will not take.

use axum::http::{Method, StatusCode};
use octo_core::settings::AppSettings;
use serde_json::{Value, json};

use super::test_app::{Reply, app, send};
use crate::app::AppState;
use crate::http::pipeline::App;

struct Setup {
    app: App,
    token: String,
}

fn setup() -> Setup {
    let state = AppState::for_tests(AppSettings::default());
    let token = state.browse_sessions.create("admin");
    Setup {
        app: app(state),
        token,
    }
}

impl Setup {
    async fn get(&self, uri: &str) -> Reply {
        let cookie = format!("octo_browse={}", self.token);
        send(&self.app, Method::GET, uri, &[("Cookie", &cookie)], None).await
    }

    async fn post(&self, uri: &str, body: Value) -> Reply {
        let cookie = format!("octo_browse={}", self.token);
        send(
            &self.app,
            Method::POST,
            uri,
            &[("X-Octo-Admin", "1"), ("Cookie", &cookie)],
            Some(&body.to_string()),
        )
        .await
    }
}

const WRITES: [&str; 3] = [
    "/api/admin/youtube-check/songs/replace",
    "/api/admin/youtube-check/songs/keep",
    "/api/admin/youtube-check/songs/dismiss",
];

#[tokio::test]
async fn the_list_and_its_actions_need_a_sign_in() {
    let setup = setup();

    let reply = send(
        &setup.app,
        Method::GET,
        "/api/admin/youtube-check/songs",
        &[],
        None,
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    for uri in WRITES {
        let reply = send(
            &setup.app,
            Method::POST,
            uri,
            &[("X-Octo-Admin", "1")],
            Some(r#"{"ids":["a.flac"]}"#),
        )
        .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{uri}");
    }
}

#[tokio::test]
async fn before_a_check_has_heard_anything_the_list_is_empty() {
    let setup = setup();

    let reply = setup.get("/api/admin/youtube-check/songs?filter=all").await;

    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["songs"], json!([]));
    assert_eq!(
        (
            body["matching"].clone(),
            body["wrong"].clone(),
            body["unclear"].clone()
        ),
        (json!(0), json!(0), json!(0))
    );
    // A page size past the ceiling is cut to it.
    let reply = setup.get("/api/admin/youtube-check/songs?limit=100000").await;
    assert_eq!(reply.json()["limit"], 200);
}

#[tokio::test]
async fn keep_and_dismiss_need_songs_and_only_count_those_on_the_list() {
    let setup = setup();
    for uri in &WRITES[1..] {
        let reply = setup.post(uri, json!({})).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(reply.json()["error"], "Say which songs.", "{uri}");

        let reply = setup.post(uri, json!({ "ids": ["not-on-it.flac"] })).await;
        assert_eq!(reply.status, StatusCode::OK, "{uri}");
        assert_eq!(reply.json()["settled"], 0, "{uri}");
    }
}

#[tokio::test]
async fn replacing_is_refused_while_library_actions_cannot_change_files() {
    let setup = setup();

    let reply = setup.post(WRITES[0], json!({ "ids": ["a.flac"] })).await;

    // Library actions are off in a fresh setup: a refusal that says why, never a queue.
    assert!(reply.status.is_client_error(), "{}", reply.status);
    assert!(
        reply.json()["error"]
            .as_str()
            .is_some_and(|words| !words.is_empty())
    );
}

#[test]
fn only_a_real_video_id_becomes_an_address() {
    use super::youtube_check_songs::reference_url;

    assert_eq!(
        reference_url("dQw4w9WgXcQ").as_deref(),
        Some("https://music.youtube.com/watch?v=dQw4w9WgXcQ")
    );
    for bad in [
        "",
        "short",
        "has space in it",
        "a/b?c=d&e",
        "javascript:alert(1)",
        &"x".repeat(40),
    ] {
        assert_eq!(reference_url(bad), None, "{bad}");
    }
}
