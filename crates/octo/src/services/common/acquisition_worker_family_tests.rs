//! The worker with the family pool: a member's request is placed in the pool, everyone who
//! joined while it ran gets a link, and a request nobody in the family made goes as always.

use std::sync::Arc;

use async_trait::async_trait;
use octo_core::models::download::DownloadInfo;
use octo_core::settings::DownloadSource;
use parking_lot::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::services::common::{AcquisitionPlace, AcquisitionTarget};
use crate::services::family::test_support::{World, write_file};
use crate::services::i_download_service::{AudioStream, DirectStreamInfo};
use crate::services::library::ReplacementHandoff;

/// Downloads that wait for a release, then land a file in the pool (for a place with members)
/// or answer a shared path, remembering which way they were asked.
struct PlacingDownloads {
    pool_root: String,
    places: Mutex<Vec<Option<AcquisitionPlace>>>,
    release: watch::Receiver<bool>,
}

#[async_trait]
impl IDownloadService for PlacingDownloads {
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
        self.places.lock().push(None);
        Ok("/music/Artist/Album/1.flac".into())
    }

    async fn execute_acquisition_to(
        &self,
        _: &str,
        external_id: &str,
        _: bool,
        _: bool,
        _: Option<DownloadSource>,
        _: &CancellationToken,
        _: Option<Vec<String>>,
        _: bool,
        _: Option<Arc<ReplacementHandoff>>,
        place: AcquisitionPlace,
    ) -> anyhow::Result<String> {
        self.places.lock().push(Some(place));
        let mut release = self.release.clone();
        let _ = release.wait_for(|released| *released).await;
        let path = format!("{}/Artist/Album/{external_id}.flac", self.pool_root);
        write_file(&path, 10);
        Ok(path)
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

struct Rig {
    world: World,
    queue: Arc<TrackAcquisitionQueue>,
    downloads: Arc<PlacingDownloads>,
    release: watch::Sender<bool>,
    stopping: CancellationToken,
}

fn rig() -> Rig {
    let world = World::new(None);
    let queue = Arc::new(TrackAcquisitionQueue::new());
    let (release, released) = watch::channel(false);
    let downloads = Arc::new(PlacingDownloads {
        pool_root: world.pool.pool_root_path(),
        places: Mutex::new(Vec::new()),
        release: released,
    });
    let worker = Arc::new(
        AcquisitionWorker::new(
            Arc::clone(&queue),
            Arc::clone(&downloads) as Arc<dyn IDownloadService>,
            Arc::new(ExternalIdRegistry::in_memory()),
            Arc::new(NotificationService::new(Vec::new(), world.settings.clone(), None)),
            None,
        )
        .with_family(world.pool.clone()),
    );
    let stopping = CancellationToken::new();
    tokio::spawn(worker.run(stopping.clone()));
    Rig {
        world,
        queue,
        downloads,
        release,
        stopping,
    }
}

fn for_member(rig: &Rig, user: &str) -> crate::services::common::Completion {
    rig.queue.enqueue_for(
        &AcquisitionTarget::Member(user.into()),
        false,
        "test",
        "1",
        true,
        false,
        true,
        None,
        true,
        Some(user),
        false,
        None,
    )
}

#[tokio::test]
async fn a_member_who_joins_while_it_runs_gets_a_link_too() {
    let rig = rig();
    let alex = for_member(&rig, "alex");
    crate::services::common::test_fakes::until(|| !rig.downloads.places.lock().is_empty()).await;
    let sam = for_member(&rig, "sam");
    rig.release.send(true).expect("released");
    let path = alex.wait().await.expect("landed");
    assert_eq!(sam.wait().await.expect("landed"), path, "one download");
    assert_eq!(
        rig.downloads.places.lock().as_slice(),
        [Some(AcquisitionPlace {
            pool_members: Some(vec!["alex".into()]),
            flac_only: false,
        })]
    );
    for user in ["alex", "sam"] {
        let link = rig.world.pool.link_path(&path, user).expect("a link path");
        assert!(std::fs::symlink_metadata(&link).is_ok(), "{user} has it");
    }
    rig.stopping.cancel();
}

#[tokio::test]
async fn a_request_nobody_in_the_family_made_goes_as_always() {
    let rig = rig();
    let path = rig
        .queue
        .enqueue(
            "test",
            "2",
            true,
            false,
            true,
            None,
            true,
            Some("winters"),
            false,
            None,
        )
        .wait()
        .await
        .expect("landed");
    assert_eq!(path, "/music/Artist/Album/1.flac");
    assert_eq!(rig.downloads.places.lock().as_slice(), [None]);
    rig.stopping.cancel();
}
