//! Replace now: an open Review question answered with Wrong song from the dashboard, as the
//! signed-in Navidrome admin and held to the same gates as the Wrong song playlist.

use axum::http::{Method, StatusCode};
use octo_core::models::domain::Song;
use octo_core::settings::{AppSettings, LibraryAction, LibraryActionDefinition, LibraryActionSettings};
use serde_json::json;

use super::test_app::{Reply, app, send};
use crate::app::AppState;
use crate::services::library::notice_queue::NoticeQueue;

const FILE: &str = "/music/Massive Attack - Teardrop.flac";

/// Library actions on for the admin, Wrong song on, no dry run.
fn replacing() -> LibraryActionSettings {
    LibraryActionSettings {
        enabled: true,
        dry_run: false,
        allowed_users: vec!["admin".into()],
        actions: vec![LibraryActionDefinition {
            action: LibraryAction::WrongSong,
            name: String::new(),
            enabled: true,
            rating: None,
        }],
        ..Default::default()
    }
}

/// An app with `actions`, an open Review question about Teardrop for the admin, and the admin's
/// sign-in: the state, the sign-in token and the question's key.
fn asked(actions: LibraryActionSettings) -> (AppState, String, String) {
    let state = AppState::for_tests(AppSettings {
        library_actions: actions,
        ..Default::default()
    });
    let song = Song {
        artist: "Massive Attack".into(),
        title: "Teardrop".into(),
        album: "Mezzanine".into(),
        ..Default::default()
    };
    assert!(state.notice_queue.add_library_question(
        "admin",
        FILE,
        &song,
        "Sounds nothing like the song on YouTube"
    ));
    let token = state.browse_sessions.create("admin");
    (state, token, NoticeQueue::review_key("admin", FILE))
}

async fn replace_now(state: &AppState, headers: &[(&str, &str)], key: &str) -> Reply {
    let body = json!({ "key": key }).to_string();
    send(
        &app(state.clone()),
        Method::POST,
        "/api/admin/library-actions/replace-now",
        headers,
        Some(&body),
    )
    .await
}

fn signed_in(token: &str) -> [(&'static str, String); 2] {
    [
        ("X-Octo-Admin", "1".to_string()),
        ("Cookie", format!("octo_browse={token}")),
    ]
}

async fn as_admin(state: &AppState, token: &str, key: &str) -> Reply {
    let headers = signed_in(token);
    let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
    replace_now(state, &headers, key).await
}

#[tokio::test]
async fn the_questions_say_their_key_whether_they_are_open_and_the_song_navidrome_shows() {
    let (state, token, key) = asked(replacing());
    let cookie = format!("octo_browse={token}");
    let list = |state: AppState| {
        let cookie = cookie.clone();
        async move {
            send(
                &app(state),
                Method::GET,
                "/api/admin/notices",
                &[("Cookie", &cookie)],
                None,
            )
            .await
            .json()["entries"][0]
                .clone()
        }
    };

    let entry = list(state.clone()).await;
    assert_eq!(entry["key"], key.as_str());
    assert_eq!(entry["open"], true);
    assert!(entry.get("navidromeId").is_none(), "{entry}");

    state.notice_queue.set_navidrome_id(&key, "nd-1");
    assert_eq!(list(state).await["navidromeId"], "nd-1");
}

#[tokio::test]
async fn replace_now_needs_the_dashboard_sign_in() {
    let (state, _, key) = asked(replacing());
    state.notice_queue.set_navidrome_id(&key, "nd-1");

    let refused = replace_now(&state, &[("X-Octo-Admin", "1")], &key).await;

    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn replace_now_keeps_to_the_wrong_song_gates() {
    for (actions, words) in [
        (
            LibraryActionSettings::default(),
            "Library actions are off. Turn them on to replace songs.",
        ),
        (
            LibraryActionSettings {
                allowed_users: vec!["bob".into()],
                ..replacing()
            },
            "admin is not on the library actions allowlist.",
        ),
        (
            LibraryActionSettings {
                actions: Vec::new(),
                ..replacing()
            },
            "The Wrong song action is off under Library actions. Turn it on to replace songs.",
        ),
        (
            LibraryActionSettings {
                dry_run: true,
                ..replacing()
            },
            "Dry run is on, so nothing is replaced.",
        ),
    ] {
        let (state, token, key) = asked(actions);
        state.notice_queue.set_navidrome_id(&key, "nd-1");

        let refused = as_admin(&state, &token, &key).await;

        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{words}");
        assert_eq!(refused.json()["error"], words);
    }
}

#[tokio::test]
async fn replace_now_answers_only_an_open_question_navidrome_shows() {
    let (state, token, key) = asked(replacing());

    let unknown = as_admin(&state, &token, "no such question").await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    assert_eq!(unknown.json()["error"], "That question is not open in Review.");

    let unscanned = as_admin(&state, &token, &key).await;
    assert_eq!(unscanned.status, StatusCode::CONFLICT);

    state.notice_queue.set_navidrome_id(&key, "nd-1");
    let accepted = as_admin(&state, &token, &key).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text());
    assert_eq!(accepted.json()["ok"], true);
}
