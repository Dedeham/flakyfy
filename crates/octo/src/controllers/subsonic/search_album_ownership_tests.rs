//! A "drake" search through the real controller. search3 listed HABIBTI, HABIBTI (FOMO) and "$ome
//! $exy $ongs 4 U" as not in the library while the library held them, and What A Time To Be Alive
//! as if none of it were owned. See `services::library::album_ownership::tests::drake_search`.

use std::sync::Arc;

use octo_core::common::clock::Clock;
use octo_core::settings::{AppSettings, SubsonicSettings};
use octo_subsonic::xml::XElement;
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request as MockRequest, Respond, ResponseTemplate};

use super::test_app::{app, get, query_of};
use crate::app::{AppState, TestServices};
use crate::services::library::album_ownership::tests::drake_search as drake;
use crate::services::library::library_snapshot::LibrarySnapshot;
use crate::services::library::{CatalogTrack, quality_upgrade_worker::LibrarySongRow};

const AUTH: &str = "u=alice&t=good&s=salt&v=1.16.1&c=test";
const NS: &str = "http://subsonic.org/restapi";

fn catalog() -> Vec<(i64, &'static str, &'static str, Vec<CatalogTrack>)> {
    vec![
        (983217461, "HABIBTI", "Drake", drake::habibti()),
        (1110918492, "HABIBTI (FOMO)", "Drake", drake::habibti_fomo()),
        (
            712442261,
            "$ome $exy $ongs 4 U",
            "PARTYNEXTDOOR",
            drake::some_sexy_songs(),
        ),
        (11250516, "What A Time To Be Alive", "Drake", drake::what_a_time()),
        (983225601, "MAID OF HONOUR", "Drake", drake::maid_of_honour()),
    ]
}

fn json_reply(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_string().into_bytes(), "application/json")
}

fn xml_reply(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "application/xml")
}

/// The catalog (Deezer) and the library server (Navidrome), each on its own mock server.
#[derive(Clone)]
struct FakeDeezer;

impl Respond for FakeDeezer {
    fn respond(&self, request: &MockRequest) -> ResponseTemplate {
        let path = request.url.path().to_string();
        let catalog = catalog();
        if path.starts_with("/search/album") {
            return json_reply(json!({
                "data": catalog.iter().map(|(id, title, artist, tracks)| json!({
                    "id": id, "title": title, "record_type": "album",
                    "nb_tracks": tracks.len(), "artist": {"name": artist},
                })).collect::<Vec<_>>(),
            }));
        }
        for (id, title, artist, tracks) in &catalog {
            if path == format!("/album/{id}") {
                return json_reply(json!({
                    "id": id, "title": title, "nb_tracks": tracks.len(),
                    "release_date": "2026-01-01", "artist": {"name": artist},
                }));
            }
            if path == format!("/album/{id}/tracks") {
                return json_reply(json!({
                    "total": tracks.len(),
                    "data": tracks.iter().enumerate().map(|(i, track)| json!({
                        "title": track.title, "duration": track.duration, "track_position": i + 1,
                        "disk_number": 1, "isrc": track.isrc, "artist": {"name": track.artist},
                    })).collect::<Vec<_>>(),
                }));
            }
        }
        if path.starts_with("/search/artist") {
            return json_reply(json!({"data": [{"id": 246791, "name": "Drake", "nb_fan": 1000}]}));
        }
        if path.starts_with("/artist/246791/albums") {
            return json_reply(json!({
                "data": catalog.iter().filter(|(_, _, artist, _)| *artist == "Drake").map(|(id, title, _, _)| json!({
                    "id": id, "title": title, "record_type": "album", "release_date": "2026-01-01",
                })).collect::<Vec<_>>(),
            }));
        }
        ResponseTemplate::new(404)
    }
}

#[derive(Clone)]
struct FakeNavidrome;

impl Respond for FakeNavidrome {
    fn respond(&self, request: &MockRequest) -> ResponseTemplate {
        let path = request.url.path().to_string();
        let query = query_of(request);
        let is_json = query.get("f").map(String::as_str) == Some("json");
        if path.ends_with("/rest/getArtist") {
            return json_reply(
                json!({"subsonic-response": {"status": "ok", "version": "1.16.1", "artist": {
                "id": "ar-drake", "name": "Drake", "albumCount": 2, "album": [
                    {"id": "al-fomo", "name": "HABIBTI (FOMO)", "artist": "Drake", "songCount": 15},
                    {"id": "al-takecare", "name": "Take Care", "artist": "Drake", "songCount": 2}]}}}),
            );
        }
        if path.ends_with("/rest/search3") {
            // Navidrome's own answer for "drake": the Drake-credited part of the split album,
            // not the expanded HABIBTI (credited apart) nor the other part.
            return if is_json {
                json_reply(
                    json!({"subsonic-response": {"status": "ok", "version": "1.16.1", "searchResult3": {
                    "album": [{"id": "al-takecare", "name": "Take Care", "artist": "Drake", "songCount": 2},
                              {"id": "3jmLzEVlwtrlq8XfVjei9k", "name": "$ome $exy $ongs 4 U", "artist": "Drake", "songCount": 5}]}}}),
                )
            } else {
                xml_reply(
                    r#"<subsonic-response xmlns="http://subsonic.org/restapi" status="ok" version="1.16.1"><searchResult3><album id="al-takecare" name="Take Care" artist="Drake" songCount="2"/><album id="3jmLzEVlwtrlq8XfVjei9k" name="$ome $exy $ongs 4 U" artist="Drake" songCount="5"/></searchResult3></subsonic-response>"#
                        .to_string(),
                )
            };
        }
        if path.ends_with("/rest/getAlbum") {
            let id = query.get("id").cloned().unwrap_or_default();
            let rows: Vec<&LibrarySongRow> = drake::LIBRARY
                .iter()
                .filter(|row| row.album_id.as_deref() == Some(id.as_str()))
                .collect();
            if !rows.is_empty() {
                let first = rows[0];
                let album = first.album.clone().unwrap_or_default();
                let album_artist = first.album_artist.clone().unwrap_or_default();
                let escape = |text: &str| text.replace('&', "&amp;");
                if !is_json {
                    let songs: String = rows
                        .iter()
                        .enumerate()
                        .map(|(i, row)| {
                            format!(
                                r#"<song id="{}" title="{}" track="{}"/>"#,
                                row.id,
                                row.title,
                                i + 1
                            )
                        })
                        .collect();
                    return xml_reply(format!(
                        r#"<subsonic-response xmlns="http://subsonic.org/restapi" status="ok" version="1.16.1"><album id="{id}" name="{}" artist="{}" songCount="{}">{songs}</album></subsonic-response>"#,
                        escape(&album),
                        escape(&album_artist),
                        rows.len()
                    ));
                }
                return json_reply(
                    json!({"subsonic-response": {"status": "ok", "version": "1.16.1", "album": {
                        "id": id, "name": album, "artist": album_artist, "songCount": rows.len(),
                        "song": rows.iter().enumerate().map(|(i, row)| json!({
                            "id": row.id, "title": row.title, "album": row.album, "albumId": row.album_id,
                            "artist": row.artist, "track": i + 1, "duration": row.duration, "suffix": "flac",
                            "path": row.path,
                        })).collect::<Vec<_>>(),
                    }}}),
                );
            }
        }
        if path.ends_with("/rest/ping") {
            return json_reply(json!({"subsonic-response": {"status": "ok", "version": "1.16.1"}}));
        }
        ResponseTemplate::new(404)
    }
}

/// The app over both fakes, with the library snapshot read from the drake library (or not at
/// all).
struct Web {
    state: AppState,
    _servers: (MockServer, MockServer),
}

impl Web {
    async fn new(library_readable: bool) -> Web {
        let navidrome = MockServer::start().await;
        Mock::given(any())
            .respond_with(FakeNavidrome)
            .mount(&navidrome)
            .await;
        let deezer = MockServer::start().await;
        Mock::given(any()).respond_with(FakeDeezer).mount(&deezer).await;

        let snapshot = Arc::new(LibrarySnapshot::with(
            Arc::new(move |start, count| {
                Box::pin(async move {
                    if !library_readable {
                        return None;
                    }
                    let rows: Vec<LibrarySongRow> = drake::LIBRARY
                        .iter()
                        .skip(start as usize)
                        .take(count as usize)
                        .cloned()
                        .collect();
                    let held = (count as usize).min(drake::LIBRARY.len().saturating_sub(start as usize));
                    Some((rows, held))
                })
            }),
            Clock::system(),
        ));
        let state = AppState::for_tests_with(
            AppSettings {
                subsonic: SubsonicSettings {
                    url: Some(navidrome.uri()),
                    auto_detect_download_path: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            TestServices {
                deezer_base_url: Some(deezer.uri()),
                library_snapshot: Some(snapshot),
                ..Default::default()
            },
        );
        Web {
            state,
            _servers: (navidrome, deezer),
        }
    }

    async fn text(&self, uri: &str) -> String {
        let reply = get(&app(&self.state), uri).await;
        assert!(reply.status.is_success(), "{uri}: {}", reply.text());
        reply.text()
    }

    async fn json(&self, uri: &str) -> Value {
        serde_json::from_str(&self.text(uri).await).expect("JSON")
    }

    async fn search_albums(&self) -> Vec<Value> {
        let body = self
            .json(&format!(
                "/rest/search3.view?{AUTH}&f=json&query=drake&songCount=0&artistCount=0&albumCount=20"
            ))
            .await;
        body["subsonic-response"]["searchResult3"]["album"]
            .as_array()
            .expect("albums")
            .clone()
    }
}

fn text_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_default()
}

#[tokio::test]
async fn search_lists_held_albums_as_the_librarys_and_counts_what_is_held_of_the_rest() {
    let web = Web::new(true).await;

    let albums = web.search_albums().await;
    let rows: Vec<(&str, &str, bool)> = albums
        .iter()
        .map(|a| {
            (
                text_of(a, "name"),
                text_of(a, "id"),
                a["isExternal"].as_bool().unwrap_or(false),
            )
        })
        .collect();

    // The library's own two, then the library albums standing in for HABIBTI (both editions)
    // and for "$ome $exy $ongs 4 U" (the part credited to PARTYNEXTDOOR), then the two outside
    // albums the library does not hold whole.
    assert_eq!(
        rows[..4],
        [
            ("Take Care", "al-takecare", false),
            ("$ome $exy $ongs 4 U", drake::SEXY_DRAKE_PART_ID, false),
            ("HABIBTI (FOMO)", drake::FOMO_ID, false),
            ("$ome $exy $ongs 4 U", drake::SEXY_PND_PART_ID, false),
        ],
        "{rows:?}"
    );
    assert_eq!(
        rows[4..].iter().map(|r| r.0).collect::<Vec<_>>(),
        ["What A Time To Be Alive", "MAID OF HONOUR"]
    );
    assert!(rows[4..].iter().all(|r| r.2));

    let wattba = &albums[4];
    assert_eq!(wattba["ownedCount"], 2);
    assert_eq!(wattba["songCount"], 11);
    assert_eq!(albums[5]["ownedCount"], 0);
    // An added library album is listed as Navidrome lists it, without its songs.
    assert!(albums[2].get("song").is_none());
    assert!(albums[2].get("ownedCount").is_none());
}

#[tokio::test]
async fn search_in_xml_says_the_same() {
    let web = Web::new(true).await;

    let xml = XElement::parse(
        &web.text(&format!(
            "/rest/search3.view?{AUTH}&query=drake&songCount=0&artistCount=0&albumCount=20"
        ))
        .await,
    )
    .expect("XML");
    let result = xml.first_descendant(Some(NS), "searchResult3").expect("a result");
    let albums: Vec<&XElement> = result.elements_named(Some(NS), "album").collect();

    assert_eq!(
        albums
            .iter()
            .map(|a| a.attribute("name").unwrap_or_default())
            .collect::<Vec<_>>(),
        [
            "Take Care",
            "$ome $exy $ongs 4 U",
            "HABIBTI (FOMO)",
            "$ome $exy $ongs 4 U",
            "What A Time To Be Alive",
            "MAID OF HONOUR"
        ]
    );
    assert_eq!(albums[2].elements_named(Some(NS), "song").count(), 0);
    assert_eq!(albums[4].attribute("ownedCount"), Some("2"));
    assert_eq!(albums[4].attribute("songCount"), Some("11"));
}

#[tokio::test]
async fn search_without_the_library_is_as_before() {
    let web = Web::new(false).await;

    let albums = web.search_albums().await;

    // Only the exact name and artist the search itself listed is left out, as before.
    assert_eq!(
        albums.iter().map(|a| text_of(a, "name")).collect::<Vec<_>>(),
        [
            "Take Care",
            "$ome $exy $ongs 4 U",
            "HABIBTI",
            "HABIBTI (FOMO)",
            "$ome $exy $ongs 4 U",
            "What A Time To Be Alive",
            "MAID OF HONOUR"
        ]
    );
    assert!(albums.iter().all(|a| a.get("ownedCount").is_none()));
}

#[tokio::test]
async fn artist_page_leaves_out_what_its_albums_hold_whole_and_counts_the_rest() {
    let web = Web::new(true).await;

    let body = web
        .json(&format!("/rest/getArtist.view?{AUTH}&f=json&id=ar-drake"))
        .await;
    let albums = body["subsonic-response"]["artist"]["album"]
        .as_array()
        .expect("albums");
    let by_name = |name: &str| {
        albums
            .iter()
            .find(|a| text_of(a, "name") == name)
            .unwrap_or_else(|| panic!("{name}"))
    };

    // HABIBTI is all inside the page's own HABIBTI (FOMO); the catalog's HABIBTI (FOMO) is that album.
    assert_eq!(
        albums.iter().map(|a| text_of(a, "name")).collect::<Vec<_>>(),
        [
            "HABIBTI (FOMO)",
            "Take Care",
            "What A Time To Be Alive",
            "MAID OF HONOUR"
        ]
    );
    assert_eq!(by_name("What A Time To Be Alive")["ownedCount"], 2);
    assert_eq!(by_name("MAID OF HONOUR")["ownedCount"], 0);
    assert_eq!(by_name("HABIBTI (FOMO)")["isExternal"], false);
}

#[tokio::test]
async fn outside_album_gives_the_songs_the_library_holds_as_the_librarys_copies() {
    let web = Web::new(true).await;

    let wattba_id = web
        .search_albums()
        .await
        .into_iter()
        .find(|a| text_of(a, "name") == "What A Time To Be Alive")
        .map(|a| text_of(&a, "id").to_string())
        .expect("listed");

    let body = web
        .json(&format!("/rest/getAlbum.view?{AUTH}&f=json&id={wattba_id}"))
        .await;
    let album = &body["subsonic-response"]["album"];
    let songs = album["song"].as_array().expect("songs");

    assert_eq!(songs.len(), 11);
    assert_eq!(album["songCount"], 11);
    assert_eq!(album["ownedCount"], 2);
    assert_eq!(
        songs.iter().map(|s| text_of(s, "title")).collect::<Vec<_>>(),
        drake::what_a_time()
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>()
    );

    // The two the library holds are its own files, numbered as this album numbers them.
    let owned: Vec<&Value> = songs.iter().filter(|s| s["isExternal"] == false).collect();
    assert_eq!(
        owned.iter().map(|s| text_of(s, "title")).collect::<Vec<_>>(),
        ["Live From The Gutter", "Jumpman"]
    );
    assert!(
        owned
            .iter()
            .all(|s| text_of(s, "id").starts_with(&format!("s-{}-", drake::WATTBA_ID)))
    );
    assert_eq!(
        owned
            .iter()
            .map(|s| s["track"].as_i64().unwrap_or(0))
            .collect::<Vec<_>>(),
        [3, 9]
    );
    assert!(owned.iter().all(|s| text_of(s, "path").ends_with(".flac")));
    assert!(
        songs
            .iter()
            .filter(|s| s["isExternal"] != false)
            .all(|s| s["isExternal"] == true)
    );

    // XML says the same.
    let xml = XElement::parse(
        &web.text(&format!("/rest/getAlbum.view?{AUTH}&id={wattba_id}"))
            .await,
    )
    .expect("XML");
    let xml_album = xml.elements_named(Some(NS), "album").next().expect("an album");
    assert_eq!(xml_album.attribute("ownedCount"), Some("2"));
    assert_eq!(
        xml_album
            .elements_named(Some(NS), "song")
            .filter(|s| matches!(s.attribute("title"), Some("Jumpman" | "Live From The Gutter")))
            .map(|s| s.attribute("isExternal").unwrap_or_default())
            .collect::<Vec<_>>(),
        ["false", "false"]
    );
}

#[tokio::test]
async fn outside_album_the_library_holds_none_of_is_as_before_with_its_count() {
    let web = Web::new(true).await;

    let maid_id = web
        .search_albums()
        .await
        .into_iter()
        .find(|a| text_of(a, "name") == "MAID OF HONOUR")
        .map(|a| text_of(&a, "id").to_string())
        .expect("listed");
    let body = web
        .json(&format!("/rest/getAlbum.view?{AUTH}&f=json&id={maid_id}"))
        .await;
    let album = &body["subsonic-response"]["album"];

    assert_eq!(album["ownedCount"], 0);
    assert!(
        album["song"]
            .as_array()
            .expect("songs")
            .iter()
            .all(|s| s["isExternal"] == true)
    );
}
