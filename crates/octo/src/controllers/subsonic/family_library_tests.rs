//! A family member's library writes through the Subsonic API: the request and remove
//! playlists, and library actions on their own songs.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use http_body_util::BodyExt;
use octo_core::family::abilities::{Abilities, Preset, RequestQuality};
use octo_core::family::request::RequestState;
use octo_core::models::domain::Song;
use octo_core::soulseek::soulseek_metadata_service::{RoutingKind, SoulseekRouting};
use wiremock::matchers::{path, query_param};
use wiremock::{Mock, MockServer};

use crate::app::{AppState, TestServices};
use crate::controllers::subsonic::extensions::apply_library_action;
use crate::controllers::subsonic::playlists::mutate_playlist;
use crate::controllers::subsonic::test_app::{NAVIDROME_OK, navidrome_json, query_of, settings};
use crate::services::common::test_fakes::FakeMetadata;
use crate::services::family::FamilyCaller;
use crate::services::family::test_support::member;

struct Fixture {
    state: AppState,
    navidrome: MockServer,
    _members: tempfile::TempDir,
    songs: Vec<String>,
}

fn playlist(id: &str, name: &str, entries: &[&str]) -> String {
    let entries: Vec<String> = entries.iter().map(|e| format!(r#"{{"id":"{e}"}}"#)).collect();
    format!(
        r#"{{"subsonic-response":{{"status":"ok","playlist":{{"id":"{id}","name":"{name}","entry":[{}]}}}}}}"#,
        entries.join(",")
    )
}

impl Fixture {
    async fn new() -> Fixture {
        let navidrome = MockServer::start().await;
        for (id, name) in [
            ("pl-ask", "Request a copy"),
            ("pl-flac", "Request a copy in FLAC"),
            ("pl-remove", "Remove from my library"),
            ("pl-trip", "Road trip"),
        ] {
            Mock::given(path("/rest/getPlaylist"))
                .and(query_param("id", id))
                .respond_with(navidrome_json(&playlist(id, name, &[])))
                .mount(&navidrome)
                .await;
        }
        Mock::given(path("/rest/ping"))
            .respond_with(navidrome_json(NAVIDROME_OK))
            .mount(&navidrome)
            .await;
        Mock::given(path("/rest/updatePlaylist"))
            .respond_with(navidrome_json(NAVIDROME_OK))
            .mount(&navidrome)
            .await;
        let members = tempfile::tempdir().expect("a temp dir");
        let mut app = settings(&navidrome.uri());
        app.family.enabled = true;
        app.family.members_path = members.path().to_string_lossy().into_owned();
        let metadata = Arc::new(FakeMetadata::default());
        let state = AppState::for_tests_with(
            app,
            TestServices {
                metadata: Some(metadata.clone()),
                ..Default::default()
            },
        );
        let mut songs = Vec::new();
        for title in ["Teardrop", "Angel"] {
            let id = state.external_id_registry.register(SoulseekRouting {
                kind: RoutingKind::Song,
                artist: Some("Massive Attack".into()),
                title: Some(title.into()),
                ..Default::default()
            });
            metadata.songs.lock().insert(
                ("soulseek".into(), id.clone()),
                Song {
                    title: title.into(),
                    artist: "Massive Attack".into(),
                    ..Default::default()
                },
            );
            songs.push(id);
        }
        state.family_store.set_owner("winters");
        state
            .family_store
            .upsert_member(member("sam", Preset::Listener, 4));
        Fixture {
            state,
            navidrome,
            _members: members,
            songs,
        }
    }

    fn sam() -> FamilyCaller {
        FamilyCaller::member("sam", Preset::Listener, Abilities::preset(Preset::Listener))
    }

    fn request(&self, endpoint: &str, query: &str, caller: Option<FamilyCaller>) -> Request {
        let mut request = Request::builder()
            .uri(format!(
                "/rest/{endpoint}.view?{query}&u=sam&t=tok&s=salt&v=1.16.1&c=test&f=json"
            ))
            .body(Body::empty())
            .expect("a request");
        if let Some(caller) = caller {
            request.extensions_mut().insert(caller);
        }
        request
    }

    async fn update(&self, query: &str) -> serde_json::Value {
        let response = mutate_playlist(
            State(self.state.clone()),
            self.request("updatePlaylist", query, Some(Self::sam())),
        )
        .await;
        let body = response.into_body().collect().await.expect("read").to_bytes();
        serde_json::from_slice(&body).expect("JSON")
    }

    /// Every updatePlaylist Navidrome received, as its query.
    async fn updates(&self) -> Vec<std::collections::HashMap<String, String>> {
        self.navidrome
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .filter(|r| r.url.path() == "/rest/updatePlaylist")
            .map(query_of)
            .collect()
    }
}

#[tokio::test]
async fn adding_to_request_playlist_creates_requests() {
    let fixture = Fixture::new().await;
    let answer = fixture
        .update(&format!(
            "playlistId=pl-ask&songIdToAdd={}&songIdToAdd={}",
            fixture.songs[0], fixture.songs[1]
        ))
        .await;
    assert_eq!(answer["subsonic-response"]["status"], "ok");
    let requests = fixture.state.family_request_service.requests_of("sam");
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| r.quality == RequestQuality::Best && r.state == RequestState::Pending)
    );
    assert_eq!(requests[0].title, "Teardrop");
    assert!(
        fixture.updates().await.is_empty(),
        "nothing was added, so nothing to empty"
    );
}

#[tokio::test]
async fn flac_playlist_requests_flac() {
    let fixture = Fixture::new().await;
    fixture
        .update(&format!("playlistId=pl-flac&songIdToAdd={}", fixture.songs[0]))
        .await;
    let requests = fixture.state.family_request_service.requests_of("sam");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].quality, RequestQuality::Flac);
}

#[tokio::test]
async fn songs_already_in_a_request_playlist_are_asked_for_and_it_is_emptied() {
    let fixture = Fixture::new().await;
    Mock::given(path("/rest/getPlaylist"))
        .and(query_param("id", "pl-full"))
        .respond_with(navidrome_json(&playlist(
            "pl-full",
            "Request a copy",
            &[&fixture.songs[0], &fixture.songs[1]],
        )))
        .mount(&fixture.navidrome)
        .await;
    fixture.update("playlistId=pl-full&name=Request%20a%20copy").await;
    assert_eq!(fixture.state.family_request_service.requests_of("sam").len(), 2);
    let updates = fixture.updates().await;
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].get("playlistId").map(String::as_str), Some("pl-full"));
    assert!(updates[0].contains_key("songIndexToRemove"));
}

#[tokio::test]
async fn other_playlists_are_relayed_as_always() {
    let fixture = Fixture::new().await;
    fixture
        .update(&format!("playlistId=pl-trip&songIdToAdd={}", fixture.songs[0]))
        .await;
    assert!(fixture.state.family_request_service.requests_of("sam").is_empty());
    let updates = fixture.updates().await;
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].get("songIdToAdd"), Some(&fixture.songs[0]));
}

#[tokio::test]
async fn the_remove_playlist_never_takes_shared_songs() {
    let fixture = Fixture::new().await;
    let answer = fixture.update("playlistId=pl-remove&songIdToAdd=shared-1").await;
    assert_eq!(answer["subsonic-response"]["status"], "ok");
    assert!(fixture.updates().await.is_empty(), "nothing relayed");
}

#[tokio::test]
async fn a_member_removes_only_their_own_songs_with_library_actions() {
    let fixture = Fixture::new().await;
    let response = apply_library_action(
        State(fixture.state.clone()),
        fixture.request("libraryAction", "id=shared-1&action=remove", Some(Fixture::sam())),
    )
    .await
    .expect("answered");
    let body = response.into_body().collect().await.expect("read").to_bytes();
    let answer: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    let envelope = &answer["subsonic-response"];
    assert_eq!(envelope["status"], "failed", "{answer}");
    // Not a song in Sam's own library: never theirs to remove.
    let code = envelope["error"]["code"].as_i64();
    assert!(matches!(code, Some(50) | Some(70)), "{answer}");

    let edit = apply_library_action(
        State(fixture.state.clone()),
        fixture.request("libraryAction", "id=shared-1&action=retag", Some(Fixture::sam())),
    )
    .await
    .expect("answered");
    let body = edit.into_body().collect().await.expect("read").to_bytes();
    let answer: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(answer["subsonic-response"]["error"]["code"], 50);
}
