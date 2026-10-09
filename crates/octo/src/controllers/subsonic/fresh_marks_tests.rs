//! Lists drawn earlier show the listener's hearts and ratings as they are now: a heart given
//! after a list was made shows on it at once, an unheart takes it off, and a rating set through
//! Octo shows too. Hearts are read from Navidrome at most once a minute.

use std::time::Duration;

use octo_core::settings::{AppSettings, SubsonicSettings};
use serde_json::Value;

use super::test_app::{app, get};
use crate::app::{AppState, TestServices};
use crate::http::pipeline::App;
use crate::services::library::for_you_lists::tests::ForYouUpstream;

pub(crate) const AUTH: &str = "u=alice&t=token&s=salt&v=1.16.1&c=test";

pub(crate) async fn setup() -> (std::sync::Arc<ForYouUpstream>, App) {
    let upstream = ForYouUpstream::start().await;
    let state = AppState::for_tests_with(
        AppSettings {
            subsonic: SubsonicSettings {
                url: Some(upstream.navidrome_url()),
                auto_detect_download_path: false,
                ..Default::default()
            },
            ..Default::default()
        },
        TestServices {
            deezer_base_url: Some(upstream.deezer_url()),
            ..Default::default()
        },
    );
    let app = app(&state);
    (upstream, app)
}

/// The playlist rows of a getPlaylists answer.
pub(crate) async fn playlists(app: &App) -> Vec<Value> {
    get(app, &format!("/rest/getPlaylists.view?{AUTH}&f=json"))
        .await
        .json()["subsonic-response"]["playlists"]["playlist"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The id of a Made for you list, once it has been made.
async fn list_id(app: &App, kind: &str) -> String {
    for _ in 0..100 {
        if let Some(row) = playlists(app)
            .await
            .into_iter()
            .find(|row| row["octoList"] == kind)
        {
            return row["id"].as_str().expect("an id").to_string();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{kind} was never made");
}

pub(crate) async fn entries(app: &App, id: &str) -> Vec<Value> {
    get(app, &format!("/rest/getPlaylist.view?{AUTH}&f=json&id={id}"))
        .await
        .json()["subsonic-response"]["playlist"]["entry"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn entry<'a>(entries: &'a [Value], id: &str) -> &'a Value {
    entries.iter().find(|e| e["id"] == id).expect("the song")
}

/// A library song of the list.
fn library_song(entries: &[Value]) -> String {
    entries
        .iter()
        .find(|e| e.get("isExternal").and_then(Value::as_bool) != Some(true))
        .and_then(|e| e["id"].as_str())
        .expect("a library song")
        .to_string()
}

#[tokio::test]
async fn a_heart_given_after_the_list_was_made_shows_on_it_and_an_unheart_takes_it_off() {
    let (upstream, app) = setup().await;
    let id = list_id(&app, "deepCuts").await;
    let before = entries(&app, &id).await;
    let song = library_song(&before);
    assert!(entry(&before, &song).get("starred").is_none());

    let starred = get(&app, &format!("/rest/star.view?{AUTH}&f=json&id={song}")).await;
    assert!(starred.status.is_success());
    let after = entries(&app, &id).await;
    assert!(
        entry(&after, &song)["starred"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{:?}",
        entry(&after, &song)
    );

    let unstarred = get(&app, &format!("/rest/unstar.view?{AUTH}&f=json&id={song}")).await;
    assert!(unstarred.status.is_success());
    let again = entries(&app, &id).await;
    assert!(entry(&again, &song).get("starred").is_none());
    assert_eq!(upstream.hearts.lock().len(), 0);
}

#[tokio::test]
async fn hearts_are_read_once_a_minute_not_on_every_open() {
    let (upstream, app) = setup().await;
    let id = list_id(&app, "deepCuts").await;
    entries(&app, &id).await;
    let read = upstream.calls_ending("getStarred2");
    assert!(read >= 1);

    entries(&app, &id).await;
    entries(&app, &id).await;

    assert_eq!(upstream.calls_ending("getStarred2"), read);
}

#[tokio::test]
async fn a_rating_set_through_octo_shows_on_the_list() {
    let (_upstream, app) = setup().await;
    let id = list_id(&app, "deepCuts").await;
    let song = library_song(&entries(&app, &id).await);

    let rated = get(
        &app,
        &format!("/rest/setRating.view?{AUTH}&f=json&id={song}&rating=4"),
    )
    .await;
    assert!(rated.status.is_success());

    assert_eq!(entry(&entries(&app, &id).await, &song)["userRating"], 4);
}

#[tokio::test]
async fn a_heart_on_navidrome_shows_once_the_minute_is_up() {
    let (upstream, app) = setup().await;
    let id = list_id(&app, "deepCuts").await;
    let song = library_song(&entries(&app, &id).await);
    // Hearted in another app, straight on Navidrome: Octo hears of it on its next read.
    upstream
        .hearts
        .lock()
        .push((song.clone(), "2026-10-09T10:00:00.000Z".to_string()));

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();

    assert_eq!(
        entry(&entries(&app, &id).await, &song)["starred"],
        "2026-10-09T10:00:00.000Z"
    );
}
