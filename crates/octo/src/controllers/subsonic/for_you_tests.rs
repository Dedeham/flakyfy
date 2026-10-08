//! Port of `ForYouControllerTests` in `octo.Tests/ForYouListsTests.cs` (f54467a): getPlaylists
//! marks each Made for you list in JSON and XML, getPlaylist serves one, and its cover is an
//! image. The stand-in Navidrome and Deezer are `services::library::for_you_lists::tests`'.

use std::time::Duration;

use octo_core::settings::{AppSettings, SubsonicSettings};
use serde_json::Value;

use super::test_support_6a2::{app, get};
use crate::app::{AppState, TestServices};
use crate::services::library::for_you_lists::tests::ForYouUpstream;

const AUTH: &str = "u=alice&t=token&s=salt&v=1.16.1&c=test";

#[tokio::test]
async fn get_playlists_marks_each_made_for_you_list_in_json_and_xml() {
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

    let mut rows: Vec<Value> = Vec::new();
    for _ in 0..100 {
        let reply = get(&app, &format!("/rest/getPlaylists.view?{AUTH}&f=json")).await;
        let body = reply.json();
        rows = body["subsonic-response"]["playlists"]["playlist"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter(|row| row.get("octoList").is_some())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if rows.len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(
        rows.iter()
            .map(|row| row["octoList"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["newReleases", "rediscover", "deepCuts"]
    );
    assert!(rows.iter().all(|row| row["readonly"] == true));
    let xml = get(&app, &format!("/rest/getPlaylists.view?{AUTH}")).await.text();
    assert!(xml.contains(r#"octoList="newReleases""#), "{xml}");

    let id = rows[0]["id"].as_str().expect("an id").to_string();
    let detail = get(&app, &format!("/rest/getPlaylist.view?{AUTH}&f=json&id={id}"))
        .await
        .json();
    let playlist = &detail["subsonic-response"]["playlist"];
    assert_eq!(playlist["octoList"], "newReleases");
    assert!(!playlist["entry"].as_array().expect("entries").is_empty());

    let cover = get(&app, &format!("/rest/getCoverArt.view?{AUTH}&f=json&id={id}")).await;
    assert!(cover.status.is_success(), "{}", cover.status);
    assert_eq!(cover.media_type().as_deref(), Some("image/jpeg"));
}
