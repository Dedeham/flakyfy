//! `stream` with songs downloaded ahead: Octo's apps get the ready FLAC for a play that starts on
//! it, other apps only with Wait for lossless on play, a play keeps the source it started on, and
//! a caller whose audio has a ceiling gets the preview.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use octo_core::family::abilities::{Abilities, Preset};
use octo_core::family::quality::StreamQuality;
use octo_core::models::domain::Song;
use octo_core::settings::{AppSettings, PrefetchSettings};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer};

use super::media::stream;
use super::test_app::*;
use crate::app::AppState;
use crate::services::family::FamilyCaller;
use crate::services::prefetch::{AheadSidecar, file_name, write_sidecar};

/// The FLAC downloaded ahead, as its bytes.
const FLAC: &[u8] = b"fLaC-the-whole-song";
/// The preview the outside source streams.
const PREVIEW: &[u8] = &[1, 2, 3];

struct World {
    state: AppState,
    _navidrome: MockServer,
    folder: tempfile::TempDir,
    streams: Arc<FakeStreams>,
}

async fn world(change: impl FnOnce(&mut AppSettings)) -> World {
    let navidrome = MockServer::start().await;
    Mock::given(any())
        .respond_with(navidrome_json(NAVIDROME_OK))
        .mount(&navidrome)
        .await;
    let folder = tempfile::tempdir().expect("a temp dir");
    let mut settings = settings(&navidrome.uri());
    settings.prefetch = PrefetchSettings {
        enabled: true,
        folder: folder.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    settings.soulseek.base_url = Some(format!(
        "http://{}",
        crate::services::test_support::refused_address()
    ));
    change(&mut settings);
    let streams = Arc::new(FakeStreams::streaming("soulseek", "song1", PREVIEW, "audio/mp4"));
    let library = Arc::new(FakeLibrary::with_external("outside-1", "soulseek", "song1"));
    let state = state_with(settings, |inner| {
        inner.download_service = streams.clone();
        inner.local_library = library.clone();
    });
    World {
        state,
        _navidrome: navidrome,
        folder,
        streams,
    }
}

impl World {
    /// The song is ready in the folder now.
    fn land(&self) -> PathBuf {
        let path = self.folder.path().join(file_name("soulseek", "song1", "flac"));
        std::fs::write(&path, FLAC).expect("the file");
        let song = Song {
            artist: "Artist".into(),
            title: "Song".into(),
            ..Default::default()
        };
        write_sidecar(
            &path,
            &AheadSidecar::of("soulseek", "song1", &path, &song, None, chrono::Utc::now()),
        )
        .expect("the facts");
        self.state.prefetch.store().adopt(&path);
        path
    }

    async fn play(&self, client: &str, range: Option<&str>) -> Reply {
        let app = app(&self.state);
        let uri = format!("/rest/stream?id=outside-1&{}", auth("alice", "good", client));
        let headers: Vec<(&str, &str)> = range.map(|r| ("Range", r)).into_iter().collect();
        send(&app, Method::GET, &uri, &headers, Body::empty()).await
    }
}

fn is_flac(reply: &Reply) -> bool {
    FLAC.ends_with(&reply.body)
        && !reply.body.is_empty()
        && reply.header("content-type") == Some("audio/flac")
}

fn is_preview(reply: &Reply) -> bool {
    reply.body.as_ref() == PREVIEW
}

#[tokio::test]
async fn an_octo_app_gets_the_flac_of_a_song_downloaded_ahead() {
    let w = world(|_| {}).await;
    w.land();
    let reply = w.play("Octo", None).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(is_flac(&reply), "{:?}", reply.body);
    assert_eq!(reply.body.as_ref(), FLAC);
    assert!(
        w.streams.stream_calls().is_empty(),
        "the preview was never asked for"
    );
}

#[tokio::test]
async fn another_app_gets_the_preview_unless_wait_for_lossless_is_on() {
    let w = world(|_| {}).await;
    w.land();
    let reply = w.play("Symfonium", None).await;
    assert!(is_preview(&reply), "{:?}", reply.body);

    let w = world(|s| s.subsonic.wait_for_lossless_on_play = true).await;
    w.land();
    let reply = w.play("Symfonium", None).await;
    assert_eq!(reply.body.as_ref(), FLAC);
}

#[tokio::test]
async fn prefetch_off_leaves_an_octo_app_on_the_preview() {
    let w = world(|s| s.prefetch.enabled = false).await;
    w.land();
    let reply = w.play("Octo", None).await;
    assert!(is_preview(&reply), "{:?}", reply.body);
}

#[tokio::test]
async fn a_seek_after_the_file_lands_stays_on_the_preview() {
    let w = world(|_| {}).await;
    let started = w.play("Octo", None).await;
    assert!(is_preview(&started));
    w.land();
    let seek = w.play("Octo", Some("bytes=2-")).await;
    assert!(is_preview(&seek), "{:?}", seek.body);
    assert_eq!(
        w.streams
            .stream_calls()
            .last()
            .and_then(|c| c.2.clone())
            .as_deref(),
        Some("bytes=2-")
    );
}

#[tokio::test]
async fn a_later_range_with_no_play_started_on_the_flac_gets_the_preview() {
    let w = world(|_| {}).await;
    w.land();
    let reply = w.play("Octo", Some("bytes=500000-")).await;
    assert!(is_preview(&reply), "{:?}", reply.body);
}

#[tokio::test]
async fn a_play_that_started_on_the_flac_keeps_it_for_its_later_ranges() {
    let w = world(|_| {}).await;
    w.land();
    let first = w.play("Octo", None).await;
    assert!(is_flac(&first));
    let later = w.play("Octo", Some("bytes=5-")).await;
    assert_eq!(later.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(later.body.as_ref(), &FLAC[5..]);
    assert!(w.streams.stream_calls().is_empty());
}

#[tokio::test]
async fn a_caller_whose_audio_has_a_ceiling_gets_the_preview() {
    let members = tempfile::tempdir().expect("a temp dir");
    let path = members.path().to_string_lossy().into_owned();
    let w = world(move |s| {
        s.family.enabled = true;
        s.family.members_path = path;
    })
    .await;
    w.land();
    let mut caller = FamilyCaller::member("alex", Preset::Member, Abilities::preset(Preset::Member));
    caller.quality.home = StreamQuality::Standard;
    caller.quality.away = StreamQuality::Standard;
    let mut request = Request::builder()
        .uri(format!(
            "/rest/stream?id=outside-1&{}",
            auth("alex", "good", "Octo")
        ))
        .body(Body::empty())
        .expect("a request");
    request.extensions_mut().insert(caller);
    let response = stream(State(w.state.clone()), request).await.expect("answered");
    let body = response.into_body().collect().await.expect("read").to_bytes();
    assert_eq!(body.as_ref(), PREVIEW);
    assert!(Path::new(&w.folder.path().join(file_name("soulseek", "song1", "flac"))).is_file());
}
