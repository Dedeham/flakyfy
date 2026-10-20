//! The prefetch service over a handler-test state: what a now-playing queues, who it never
//! queues for, what DownloadOnPlay does instead, and the worker's download into the folder.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use octo_core::common::Clock;
use octo_core::family::abilities::{Abilities, Preset};
use octo_core::models::domain::Song;
use octo_core::models::download::DownloadInfo;
use octo_core::settings::{AppSettings, DownloadSource, PrefetchScope, PrefetchSettings};
use octo_core::soulseek::soulseek_metadata_service::{RoutingKind, SoulseekRouting};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::app::{AppState, TestServices};
use crate::services::family::FamilyCaller;
use crate::services::family::test_support::member;
use crate::services::i_download_service::{Ahead, AudioStream, DirectStreamInfo, IDownloadService};
use crate::services::library::ReplacementHandoff;

/// A download service whose downloads ahead land a small file and its facts in the folder.
#[derive(Default)]
struct AheadDownloads {
    asked: Mutex<Vec<String>>,
}

#[async_trait]
impl IDownloadService for AheadDownloads {
    async fn download_song(&self, _: &str, _: &str, _: &CancellationToken) -> anyhow::Result<String> {
        anyhow::bail!("not set up")
    }

    async fn download_and_stream(
        &self,
        _: &str,
        _: &str,
        _: &CancellationToken,
    ) -> anyhow::Result<AudioStream> {
        anyhow::bail!("not set up")
    }

    fn download_remaining_album_tracks_in_background(&self, _: &str, _: &str, _: &str) {}

    async fn execute_acquisition(
        &self,
        _: &str,
        _: &str,
        _: bool,
        _: bool,
        _: Option<DownloadSource>,
        _: &CancellationToken,
        _: Option<Vec<String>>,
        _: bool,
        _: Option<Arc<ReplacementHandoff>>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("not set up")
    }

    async fn download_album_with_source(
        &self,
        _: &str,
        _: &str,
        _: DownloadSource,
        _: bool,
        _: &CancellationToken,
        _: Option<Vec<String>>,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn download_ahead(
        &self,
        provider: &str,
        external_id: &str,
        folder: &Path,
        _: &CancellationToken,
    ) -> anyhow::Result<Ahead> {
        self.asked.lock().push(external_id.to_string());
        std::fs::create_dir_all(folder)?;
        let path = folder.join(file_name(provider, external_id, "flac"));
        std::fs::write(&path, b"fLaC")?;
        let song = Song {
            artist: "Artist".into(),
            title: external_id.into(),
            ..Default::default()
        };
        write_sidecar(
            &path,
            &AheadSidecar::of(provider, external_id, &path, &song, None, chrono::Utc::now()),
        )?;
        Ok(Ahead::Kept { path })
    }

    fn get_download_status(&self, _: &str) -> Option<DownloadInfo> {
        None
    }

    async fn get_local_path_if_exists(&self, _: &str, _: &str) -> Option<String> {
        None
    }

    async fn is_available(&self) -> bool {
        true
    }

    async fn get_direct_stream(
        &self,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: &CancellationToken,
    ) -> anyhow::Result<Option<DirectStreamInfo>> {
        Ok(None)
    }
}

struct World {
    state: AppState,
    service: Arc<PrefetchService>,
    downloads: Arc<AheadDownloads>,
    folder: tempfile::TempDir,
    ids: Vec<String>,
}

fn world(change: impl FnOnce(&mut AppSettings)) -> World {
    let folder = tempfile::tempdir().expect("a temp dir");
    let mut settings = AppSettings {
        prefetch: PrefetchSettings {
            enabled: true,
            scope: PrefetchScope::All,
            min_free_gb: 1,
            folder: folder.path().to_string_lossy().into_owned(),
            ..Default::default()
        },
        ..Default::default()
    };
    change(&mut settings);
    let downloads = Arc::new(AheadDownloads::default());
    let state = AppState::for_tests_with(
        settings,
        TestServices {
            download_service: Some(downloads.clone() as Arc<dyn IDownloadService>),
            ..Default::default()
        },
    );
    let ids: Vec<String> = ["One", "Two", "Three", "Four", "Five"]
        .iter()
        .map(|title| {
            state.external_id_registry.register(SoulseekRouting {
                kind: RoutingKind::Song,
                artist: Some("Artist".into()),
                title: Some(title.to_string()),
                duration: Some(200),
                ..Default::default()
            })
        })
        .collect();
    // The service over the state's own parts, without a Soulseek link to wait for.
    let service = PrefetchService::new(PrefetchParts {
        settings: state.settings.clone(),
        store: state.prefetch.store().clone(),
        downloads: downloads.clone(),
        queue: state.track_acquisition_queue.clone(),
        coordinator: state.heart_acquisition_coordinator.clone(),
        radio_queues: state.radio_queues.clone(),
        write_gate: state.family_write_gate.clone(),
        library: state.local_library.clone(),
        names: Some(state.external_id_registry.clone()),
        soulseek: None,
        clock: Clock::system(),
    });
    World {
        state,
        service,
        downloads,
        folder,
        ids,
    }
}

impl World {
    fn save_queue(&self, user: &str, current: usize) {
        self.service.saved_queue(
            user,
            self.ids.clone(),
            Some(self.ids[current].clone()),
            Some(current),
        );
    }

    fn asked(&self) -> Vec<String> {
        self.downloads.asked.lock().clone()
    }

    async fn run_jobs(&self) {
        self.service.step().await;
        crate::services::common::test_fakes::until(|| self.service.state_running() == 0).await;
    }
}

#[tokio::test]
async fn a_now_playing_fetches_the_next_songs_of_the_saved_queue_into_the_folder() {
    let w = world(|s| s.prefetch.count = 2);
    w.save_queue("alice", 0);
    w.service.on_playing("alice", &w.ids[0], None).await;
    w.run_jobs().await;
    w.service.step().await;
    w.run_jobs().await;
    assert_eq!(w.asked(), vec![w.ids[1].clone(), w.ids[2].clone()]);
    for id in &w.ids[1..3] {
        let ready = w.service.store().ready("soulseek", id).expect("ready");
        assert!(ready.starts_with(w.folder.path()));
    }
    assert!(w.service.store().ready("soulseek", &w.ids[3]).is_none());
    // Nothing went into the library or its logs.
    assert!(w.state.download_history.get_recent(10).is_empty());
}

#[tokio::test]
async fn songs_that_left_the_window_are_dropped_before_they_start() {
    let w = world(|s| {
        s.prefetch.count = 2;
        s.prefetch.at_once = 1;
    });
    w.save_queue("alice", 0);
    w.service.on_playing("alice", &w.ids[0], None).await;
    // A skip to the fourth song before anything ran: the second and third are no longer next.
    w.service.on_playing("alice", &w.ids[3], None).await;
    for _ in 0..3 {
        w.run_jobs().await;
    }
    assert_eq!(w.asked(), vec![w.ids[4].clone()]);
}

#[tokio::test]
async fn nothing_is_fetched_while_prefetch_is_off() {
    let w = world(|s| s.prefetch.enabled = false);
    w.save_queue("alice", 0);
    w.service.on_playing("alice", &w.ids[0], None).await;
    w.run_jobs().await;
    assert!(w.asked().is_empty());
}

#[tokio::test]
async fn a_member_whose_plays_keep_no_copies_gets_nothing_fetched() {
    let members = tempfile::tempdir().expect("a temp dir");
    let path = members.path().to_string_lossy().into_owned();
    let w = world(move |s| {
        s.family.enabled = true;
        s.family.members_path = path;
    });
    w.state.family_store.set_owner("winters");
    w.state
        .family_store
        .upsert_member(member("sam", Preset::Listener, 4));
    w.state
        .family_store
        .upsert_member(member("alex", Preset::Member, 3));
    let sam = FamilyCaller::member("sam", Preset::Listener, Abilities::preset(Preset::Listener));
    assert!(!w.service.may_keep_copies("sam", Some(&sam)));
    w.save_queue("sam", 0);
    w.service.on_playing("sam", &w.ids[0], Some(&sam)).await;
    w.run_jobs().await;
    assert!(w.asked().is_empty(), "a listener's plays keep no copies");

    // A member who adds songs at once does get the next songs ahead.
    let alex = FamilyCaller::member("alex", Preset::Member, Abilities::preset(Preset::Member));
    assert!(w.service.may_keep_copies("alex", Some(&alex)));
    w.save_queue("alex", 0);
    w.service.on_playing("alex", &w.ids[0], Some(&alex)).await;
    w.run_jobs().await;
    assert!(!w.asked().is_empty());
}

#[tokio::test]
async fn with_download_on_play_the_next_songs_are_queued_as_plays_instead() {
    let w = world(|s| {
        s.subsonic.download_on_play = true;
        // YouTube first, so the play is queued at once rather than after a look at Soulseek.
        s.subsonic.heart_download_sources = vec![octo_core::settings::HeartDownloadStep {
            source: octo_core::settings::HeartDownloadSource::YouTube,
            song_enabled: Some(true),
            album_enabled: Some(true),
            ..Default::default()
        }];
        s.prefetch.count = 2;
    });
    w.save_queue("alice", 0);
    w.service.on_playing("alice", &w.ids[0], None).await;
    let queue = &w.state.track_acquisition_queue;
    // The play lane keeps one waiting play, as a play would.
    assert!(queue.is_in_flight("soulseek", &w.ids[1]));
    w.run_jobs().await;
    assert!(w.asked().is_empty(), "nothing goes into the prefetch folder");
}

#[tokio::test]
async fn a_radio_list_counts_only_in_order_and_radio_only_keeps_radio_songs() {
    let w = world(|s| s.prefetch.scope = PrefetchScope::RadioOnly);
    w.state.radio_queues.register(w.ids.clone());
    w.service.note_radio(&w.ids[..3]);
    w.service.on_playing("alice", &w.ids[0], None).await;
    w.service.on_playing("alice", &w.ids[1], None).await;
    for _ in 0..3 {
        w.run_jobs().await;
    }
    // The fourth song is next in the list too, but no radio offered it.
    assert_eq!(w.asked(), vec![w.ids[2].clone()]);
}

#[tokio::test]
async fn a_song_already_ready_is_not_fetched_again() {
    let w = world(|s| s.prefetch.count = 1);
    w.save_queue("alice", 0);
    w.service.on_playing("alice", &w.ids[0], None).await;
    w.run_jobs().await;
    w.service.on_playing("alice", &w.ids[4], None).await;
    w.service.on_playing("alice", &w.ids[0], None).await;
    w.run_jobs().await;
    assert_eq!(w.asked(), vec![w.ids[1].clone()]);
}

#[tokio::test]
async fn the_status_reads_the_folder_and_clear_empties_it() {
    let w = world(|s| s.prefetch.count = 1);
    w.save_queue("alice", 0);
    w.service.on_playing("alice", &w.ids[0], None).await;
    w.run_jobs().await;
    let status = w.service.status().await;
    assert!(status.enabled);
    assert_eq!(status.ready, 1);
    assert_eq!(status.used_bytes, 4);
    assert_eq!(status.cap_bytes, 20 * 1024 * 1024 * 1024);
    // Played just now, so it stays.
    w.service.store().touch("soulseek", &w.ids[1]);
    assert_eq!(w.service.clear(), 0);
}
