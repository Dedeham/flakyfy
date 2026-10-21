//! The pre-cache's view of the rest of Octo: the lists it can read without a sign-in, the
//! acquisition queue, and the library actions that let a song go.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use octo_core::common::Clock;
use octo_core::json::dom::Node;
use octo_core::library::for_you_lists;
use octo_core::library::precache_pick::{Candidate, Uptake};
use octo_core::models::domain::Song;
use octo_core::settings::{LibraryAction, SettingsStore};

use super::library_action_executor::LibraryActionRequest;
use super::library_action_journal::LibraryActionState;
use super::library_review_sweep_worker::SweepReplacer;
use super::playlist_precache_worker::{ListInfo, PlaylistPrecacheWorker, PrecacheWorld};
use super::precache_store::{Cached, PrecacheStore};
use super::{ChartPlaylistService, GeneratedPlaylist, GeneratedPlaylistService, NavidromePlaylistApi};
use crate::services::common::{AcquisitionTarget, TrackAcquisitionQueue};
use crate::services::local::ILocalLibraryService;

/// What a chart's key starts with, so a chart and a Made for you list never share one.
pub const CHART_KEY_PREFIX: &str = "chart:";

/// Everything the pre-cache reads and does.
pub struct PrecacheParts {
    pub generated: Arc<GeneratedPlaylistService>,
    pub charts: Arc<ChartPlaylistService>,
    pub local: Arc<dyn ILocalLibraryService>,
    pub queue: Arc<TrackAcquisitionQueue>,
    pub replacer: Arc<dyn SweepReplacer>,
    pub marks: Arc<NavidromePlaylistApi>,
    /// Octo's admin sign-in name, once one is captured.
    pub admin_name: Arc<dyn Fn() -> Option<String> + Send + Sync>,
}

/// The store and the worker, the worker over [`AppPrecacheWorld`].
pub fn build_precache(
    settings: &Arc<SettingsStore>,
    config_dir: &Path,
    parts: PrecacheParts,
) -> (Arc<PrecacheStore>, Arc<PlaylistPrecacheWorker>) {
    let store = Arc::new(PrecacheStore::new(Some(config_dir.join("precache.json"))));
    let worker = Arc::new(PlaylistPrecacheWorker::new(
        store.clone(),
        settings.clone(),
        Arc::new(AppPrecacheWorld(parts)),
        Clock::system(),
    ));
    (store, worker)
}

pub struct AppPrecacheWorld(PrecacheParts);

impl AppPrecacheWorld {
    /// A song of a list as a candidate, when it is from outside the library.
    fn candidate(&self, id: &str, artist: &str, title: &str) -> Option<Candidate> {
        let (external, provider, external_id) = self.0.local.parse_song_id(id);
        if !external {
            return None;
        }
        Some(Candidate {
            provider: provider?,
            external_id: external_id?,
            artist: artist.to_string(),
            title: title.to_string(),
        })
    }

    fn candidate_of_node(&self, node: &Node) -> Option<Candidate> {
        let fields = node.as_object()?;
        if !matches!(fields.get("isExternal"), Some(Node::Bool(true))) {
            return None;
        }
        let text = |name: &str| fields.get(name).and_then(Node::as_str).unwrap_or_default();
        self.candidate(fields.get("id")?.as_str()?, text("artist"), text("title"))
    }

    fn candidate_of_song(&self, song: &Song) -> Option<Candidate> {
        (!song.is_local)
            .then(|| self.candidate(&song.id, &song.artist, &song.title))
            .flatten()
    }

    fn made_for_you(&self, owner: &str) -> Vec<GeneratedPlaylist> {
        self.0
            .generated
            .lists_for(owner)
            .into_iter()
            .filter(|mix| for_you_lists::is_for_you(&mix.kind))
            .collect()
    }

    /// The listener the charts are read as: charts are the same for everyone, but are asked for by
    /// name.
    fn chart_reader(&self) -> Option<String> {
        self.0
            .generated
            .listeners()
            .into_iter()
            .next()
            .or_else(|| (self.0.admin_name)().map(|name| name.trim().to_lowercase()))
            .filter(|name| !name.is_empty())
    }
}

#[async_trait]
impl PrecacheWorld for AppPrecacheWorld {
    fn lists(&self) -> Vec<ListInfo> {
        let mut lists = Vec::new();
        for owner in self.0.generated.listeners() {
            for mix in self.made_for_you(&owner) {
                let outside = self.outside_songs(&owner, &mix.key).len();
                lists.push(ListInfo {
                    owner: owner.clone(),
                    key: mix.key,
                    label: mix.label,
                    kind: "Made for you",
                    outside,
                });
            }
        }
        if let Some(owner) = self.chart_reader() {
            for chart in self.0.charts.lists_on() {
                if let Some(made) = self.0.charts.made_playlist(&owner, &chart) {
                    let key = format!("{CHART_KEY_PREFIX}{chart}");
                    let outside = self.outside_songs(&owner, &key).len();
                    lists.push(ListInfo {
                        owner: owner.clone(),
                        key,
                        label: made.name,
                        kind: "Chart",
                        outside,
                    });
                }
            }
        }
        lists
    }

    fn outside_songs(&self, owner: &str, key: &str) -> Vec<Candidate> {
        if let Some(chart) = key.strip_prefix(CHART_KEY_PREFIX) {
            return self
                .0
                .charts
                .outside_songs(owner, chart)
                .iter()
                .filter_map(|song| self.candidate_of_song(song))
                .collect();
        }
        let Some(mix) = self.made_for_you(owner).into_iter().find(|mix| mix.key == key) else {
            return Vec::new();
        };
        self.0
            .generated
            .drawn(owner, &mix)
            .unwrap_or_default()
            .iter()
            .filter_map(|node| self.candidate_of_node(node))
            .collect()
    }

    fn busy(&self) -> bool {
        !self.0.queue.is_idle()
    }

    async fn already_here(&self, song: &Candidate) -> bool {
        self.0.queue.is_in_flight(&song.provider, &song.external_id)
            || self
                .0
                .local
                .get_local_id_for_external_song(&song.provider, &song.external_id)
                .await
                .is_some()
    }

    async fn fetch(&self, song: &Candidate) -> Result<String, String> {
        // The play lane, behind every heart. Permanent, whatever the storage mode: the pre-cache
        // lets a song go itself. Nobody is named as the requester, so no listener is credited
        // with the file or gets it starred.
        self.0
            .queue
            .enqueue_for(
                &AcquisitionTarget::Shared,
                false,
                &song.provider,
                &song.external_id,
                false,
                false,
                true,
                None,
                false,
                None,
                false,
                None,
            )
            .wait()
            .await
            .map_err(|e| e.to_string())
    }

    fn file_exists(&self, path: &str) -> bool {
        Path::new(path).is_file()
    }

    async fn song_id(&self, cached: &Cached) -> Option<String> {
        self.0
            .replacer
            .song_id(&cached.artist, &cached.title, &cached.path)
            .await
    }

    async fn uptake(&self, song_id: &str) -> Uptake {
        // Stars and ratings of every listener that can be read; plays are the admin's own.
        Uptake {
            marked: self.0.replacer.marked(song_id).await,
            plays: self.0.marks.song_marks(song_id).await.map(|marks| marks.plays),
        }
    }

    async fn let_go(&self, song_id: &str) -> Result<(), String> {
        let admin = (self.0.admin_name)().ok_or("Octo has no admin sign-in to act with")?;
        // One copy, so Delete does not also forbid the song from being downloaded again.
        let request = LibraryActionRequest::new(LibraryAction::Delete, song_id, admin).only_a_copy(true);
        let outcome = self
            .0
            .replacer
            .replace(request)
            .await
            .map_err(|e| e.to_string())?;
        if outcome.state == LibraryActionState::Applied {
            Ok(())
        } else {
            Err(outcome
                .detail
                .unwrap_or_else(|| format!("the library actions answered {:?}", outcome.state)))
        }
    }
}
