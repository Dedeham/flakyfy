//! Members' requests for a copy: made, decided, fetched and finished.
//!
//! A new request for a song is met without a download whenever it can be, in this order:
//!
//! 1. The song is in the shared library: done at once ("Already in the family library").
//! 2. Another member has it in the family pool: a link is added to the member's library and
//!    it is done at once ("Added from the family library"). Not for a kid, or anyone whose
//!    `InstantFromFamily` is off, or whose library is full: their request waits like any other.
//! 3. Someone is downloading it right now: the member joins that download and gets a link when
//!    it lands (same rules as 2).
//! 4. Otherwise it waits for an approver, or is approved at once when the member may (see
//!    [`policy::request_decision`]).
//!
//! An approved song request downloads into the family pool, linked for the member. It is done
//! only once the library index finds the song in the member's own library, which can take a
//! scan or two; Octo looks for up to ten minutes. A FLAC request takes FLAC files only, and a
//! FLAC made from lossy audio does not count; without a real one it fails with "No real FLAC
//! copy found", and the member can ask again for an MP3. A better copy of a shared song goes
//! through the upgrade queue as the owner; an import list's missing songs go to the imports
//! trickle as the member's.
//!
//! Requests met without a download never count toward the weekly limit. Every change is
//! raised on [`FamilyEvents`] so pushes and the apps can tell people.

use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::family::abilities::{AddToLibrary, Preset, RequestQuality};
use octo_core::family::policy::{self, RequestDecision, storage_full};
use octo_core::family::request::{FamilyRequest, RequestKind, RequestOutcome, RequestState};
use octo_core::models::domain::{Album, Song};
use octo_core::settings::{DownloadSource, SettingsStore};
use octo_core::soulseek::soulseek_metadata_service::RoutingKind;
use tokio::sync::broadcast;
use tracing::{info, warn};

use super::family_store::{FamilyStore, Member};
use super::personal_library::PersonalLibrary;
use super::pool::{FamilyPool, LinkMode, MemberStars};
use super::request_store::RequestStore;
use crate::services::common::acquisition_tracker::key_of;
use crate::services::common::base_download_service::NO_REAL_FLAC;
use crate::services::common::heart_acquisition_coordinator::TrackOutcome;
use crate::services::common::{
    AcquisitionTarget, AcquisitionTracker, HeartAcquisitionCoordinator, TrackAcquisitionQueue,
};
use crate::services::i_music_metadata_service::IMusicMetadataService;
use crate::services::imports::ImportService;
use crate::services::library::{LibraryOwnership, OwnedCopy, UpgradeAsk, UpgradeQueue};
use crate::services::local::ILocalLibraryService;
use crate::services::soulseek::{ExternalIdRegistry, SoulseekMetadataService};

/// One change to a request, for whoever tells people about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FamilyEvent {
    /// A request waits for an approver.
    RequestCreated(FamilyRequest),
    /// An approver approved or declined it.
    RequestDecided(FamilyRequest),
    /// The song is in the member's library.
    RequestDone(FamilyRequest),
    /// It could not be met, with the reason in `failure`.
    RequestFailed(FamilyRequest),
}

/// The request events, for every listener (pushes, the apps' notices).
pub struct FamilyEvents {
    sender: broadcast::Sender<FamilyEvent>,
}

impl Default for FamilyEvents {
    fn default() -> Self {
        FamilyEvents {
            sender: broadcast::channel(256).0,
        }
    }
}

impl FamilyEvents {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every event from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<FamilyEvent> {
        self.sender.subscribe()
    }

    pub fn send(&self, event: FamilyEvent) {
        // Nobody listening is fine: the request itself is the record.
        let _ = self.sender.send(event);
    }
}

/// Why a request could not be made or changed, with the Subsonic error code the app gets (10
/// missing, 50 not allowed, 70 not found) and the words the member reads.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RequestError {
    pub code: i32,
    pub message: String,
}

impl RequestError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        RequestError {
            code,
            message: message.into(),
        }
    }
}

/// What a member reads when their account cannot ask for copies.
pub const NO_REQUESTS_MESSAGE: &str = "Asking for copies is off for this account.";

/// What a member reads when the song never showed up in their library.
pub const NOT_SHOWN_MESSAGE: &str = "The song did not show up in your library.";

/// An outside song or album an id names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideTarget {
    pub kind: RequestKind,
    pub provider: String,
    pub external_id: String,
}

/// Titles for a request Octo cannot look up itself (an import list, a playlist addition).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestFacts {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_art: String,
}

/// What requests look up.
#[async_trait]
pub trait RequestCatalog: Send + Sync {
    /// The outside song or album an id names.
    fn outside(&self, id: &str) -> Option<OutsideTarget>;
    async fn song(&self, provider: &str, external_id: &str) -> Option<Song>;
    async fn album(&self, provider: &str, external_id: &str) -> Option<Album>;
    /// The shared library's copy of a song.
    async fn shared_copy(&self, song: &Song) -> Option<OwnedCopy>;
}

/// How a song request's download ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// The file is here (a pool file, or the shared library's when someone wanted it shared).
    Landed(String),
    /// The member's library or the shared one had it already.
    AlreadyThere,
    /// It could not be got, with the reason.
    Failed(String),
}

/// What requests start.
#[async_trait]
pub trait RequestFetcher: Send + Sync {
    /// Whether this song is downloading for someone right now.
    fn in_flight(&self, provider: &str, external_id: &str) -> bool;
    /// One song into `user`'s own library at this quality.
    async fn song(
        &self,
        user: &str,
        provider: &str,
        external_id: &str,
        quality: RequestQuality,
        song: Option<&Song>,
    ) -> Fetched;
    /// Every song of an album into `user`'s own library; where each that landed is.
    async fn album(&self, user: &str, provider: &str, external_id: &str) -> Vec<String>;
    /// Queues a better copy of shared song `navidrome_id` as the owner. False when it could not
    /// be queued.
    fn upgrade(&self, owner: &str, navidrome_id: &str, facts: &RequestFacts) -> bool;
    /// How the better copy of `navidrome_id` ended, once it has: Ok, or the reason.
    fn upgrade_result(&self, navidrome_id: &str) -> Option<Result<(), String>>;
    /// Hands an import list's missing songs to the trickle for `user`. The words to show.
    async fn import(&self, user: &str, list_id: &str) -> Result<String, String>;
}

/// How patiently requests wait.
#[derive(Debug, Clone, Copy)]
pub struct RequestTiming {
    /// How often a better copy is asked after.
    pub poll: Duration,
    /// How long a better copy is waited for.
    pub upgrade_wait: Duration,
}

impl Default for RequestTiming {
    fn default() -> Self {
        RequestTiming {
            poll: Duration::from_secs(30),
            upgrade_wait: Duration::from_secs(24 * 60 * 60),
        }
    }
}

/// The services a [`RequestService`] works with.
pub struct RequestParts {
    pub settings: Arc<SettingsStore>,
    pub family: Arc<FamilyStore>,
    pub requests: Arc<RequestStore>,
    pub personal: Arc<PersonalLibrary>,
    pub catalog: Arc<dyn RequestCatalog>,
    pub fetcher: Arc<dyn RequestFetcher>,
    pub stars: Arc<dyn MemberStars>,
    pub events: Arc<FamilyEvents>,
}

/// Members' requests. See the module comment.
pub struct RequestService {
    me: Weak<RequestService>,
    settings: Arc<SettingsStore>,
    family: Arc<FamilyStore>,
    requests: Arc<RequestStore>,
    personal: Arc<PersonalLibrary>,
    catalog: Arc<dyn RequestCatalog>,
    fetcher: Arc<dyn RequestFetcher>,
    stars: Arc<dyn MemberStars>,
    events: Arc<FamilyEvents>,
    timing: RequestTiming,
}

impl RequestService {
    pub fn new(parts: RequestParts) -> Arc<Self> {
        Self::with_timing(parts, RequestTiming::default())
    }

    pub fn with_timing(parts: RequestParts, timing: RequestTiming) -> Arc<Self> {
        Arc::new_cyclic(|me| RequestService {
            me: me.clone(),
            settings: parts.settings,
            family: parts.family,
            requests: parts.requests,
            personal: parts.personal,
            catalog: parts.catalog,
            fetcher: parts.fetcher,
            stars: parts.stars,
            events: parts.events,
            timing,
        })
    }

    pub fn events(&self) -> &Arc<FamilyEvents> {
        &self.events
    }

    pub fn store(&self) -> &Arc<RequestStore> {
        &self.requests
    }

    fn pool(&self) -> &Arc<FamilyPool> {
        self.personal.pool()
    }

    pub fn get(&self, id: &str) -> Option<FamilyRequest> {
        self.requests.get(id)
    }

    /// One member's requests, oldest first.
    pub fn requests_of(&self, user: &str) -> Vec<FamilyRequest> {
        self.requests.for_user(user)
    }

    /// Every request waiting for an approver, oldest first.
    pub fn pending(&self) -> Vec<FamilyRequest> {
        self.requests.pending()
    }

    /// The member's weekly slots in use now.
    pub fn quota_used(&self, user: &str) -> u32 {
        self.requests.quota_used(user, Utc::now())
    }

    fn member(&self, user: &str) -> Result<Member, RequestError> {
        if !self.settings.current().family.enabled {
            return Err(RequestError::new(50, "Family is off on this server."));
        }
        let member = self
            .family
            .member(user.trim())
            .ok_or_else(|| RequestError::new(50, "Only family members ask for copies."))?;
        if member.suspended {
            return Err(RequestError::new(
                50,
                "This account is paused by the family owner.",
            ));
        }
        Ok(member)
    }

    /// Whether `user` may take a song another member already has, without asking.
    fn instant_allowed(&self, member: &Member) -> bool {
        let abilities = member.abilities();
        abilities.instant_from_family
            && member.preset != Preset::Kid
            && storage_full(&abilities, self.personal.storage_used(&member.username)).is_none()
    }

    /// A request for `target` (an outside song or album id, or a library song id for a better
    /// copy) at `quality`. The kind is read from the id unless given.
    pub async fn create(
        &self,
        user: &str,
        target: &str,
        quality: RequestQuality,
        kind: Option<RequestKind>,
    ) -> Result<FamilyRequest, RequestError> {
        self.create_with(user, kind, target, quality, None).await
    }

    /// [`Self::create`] with titles for a request Octo cannot look up (an import list).
    pub async fn create_with(
        &self,
        user: &str,
        kind: Option<RequestKind>,
        target: &str,
        quality: RequestQuality,
        facts: Option<RequestFacts>,
    ) -> Result<FamilyRequest, RequestError> {
        let target = target.trim();
        if target.is_empty() {
            return Err(RequestError::new(10, "Required parameter is missing: id"));
        }
        let member = self.member(user)?;
        let abilities = member.abilities();
        if abilities.add_to_library == AddToLibrary::SaveOnly {
            return Err(RequestError::new(50, NO_REQUESTS_MESSAGE));
        }
        if quality.rank() > abilities.request_quality.rank() {
            return Err(RequestError::new(
                50,
                format!(
                    "This account can ask for {} copies at most.",
                    abilities.request_quality.label()
                ),
            ));
        }
        let outside = self.catalog.outside(target);
        let kind = match (kind, &outside) {
            (Some(kind), _) => kind,
            (None, Some(outside)) => outside.kind,
            (None, None) if self.pool().index().song(target).is_some() => RequestKind::Upgrade,
            (None, None) => return Err(RequestError::new(70, "That song was not found.")),
        };
        // Asking again for something still waiting answers the request already made.
        if let Some(existing) = self.requests.for_user(&member.username).into_iter().find(|r| {
            r.target == target
                && r.kind == kind
                && r.quality == quality
                && matches!(r.state, RequestState::Pending | RequestState::Approved)
        }) {
            return Ok(existing);
        }
        match kind {
            RequestKind::Song => {
                let outside = outside
                    .filter(|o| o.kind == RequestKind::Song)
                    .ok_or_else(|| RequestError::new(70, "That song was not found."))?;
                self.create_song(&member, target, &outside, quality, facts).await
            }
            RequestKind::Album => {
                let outside = outside
                    .filter(|o| o.kind == RequestKind::Album)
                    .ok_or_else(|| RequestError::new(70, "That album was not found."))?;
                self.create_album(&member, target, &outside, quality, facts).await
            }
            RequestKind::Upgrade => self.create_upgrade(&member, target, quality, facts),
            RequestKind::Import | RequestKind::PlaylistAdd => self.decide_new(
                &member,
                self.new_request(&member, kind, target, quality, facts.unwrap_or_default()),
            ),
        }
    }

    /// One request for an import list's missing songs (`list_id`), titled from the list.
    pub fn create_import(
        &self,
        user: &str,
        list_id: &str,
        facts: RequestFacts,
    ) -> Result<FamilyRequest, RequestError> {
        let member = self.member(user)?;
        if member.abilities().add_to_library == AddToLibrary::SaveOnly {
            return Err(RequestError::new(50, NO_REQUESTS_MESSAGE));
        }
        if let Some(existing) = self.requests.for_user(&member.username).into_iter().find(|r| {
            r.kind == RequestKind::Import
                && r.target == list_id
                && matches!(r.state, RequestState::Pending | RequestState::Approved)
        }) {
            return Ok(existing);
        }
        // The list fetches at whatever quality the trickle finds, so it asks at the member's most.
        let quality = member.abilities().request_quality;
        let request = self.new_request(&member, RequestKind::Import, list_id, quality, facts);
        self.decide_new(&member, request)
    }

    fn new_request(
        &self,
        member: &Member,
        kind: RequestKind,
        target: &str,
        quality: RequestQuality,
        facts: RequestFacts,
    ) -> FamilyRequest {
        FamilyRequest {
            id: String::new(),
            username: member.username.clone(),
            kind,
            target: target.to_string(),
            title: facts.title,
            artist: facts.artist,
            album: facts.album,
            cover_art: facts.cover_art,
            quality,
            state: RequestState::Pending,
            created: Utc::now(),
            decided_by: None,
            decided: None,
            note: String::new(),
            library_song_id: None,
            failure: None,
            outcome: None,
        }
    }

    async fn create_song(
        &self,
        member: &Member,
        target: &str,
        outside: &OutsideTarget,
        quality: RequestQuality,
        facts: Option<RequestFacts>,
    ) -> Result<FamilyRequest, RequestError> {
        let song = self.catalog.song(&outside.provider, &outside.external_id).await;
        let facts = facts.unwrap_or_else(|| song_facts(song.as_ref(), target));
        let mut request = self.new_request(member, RequestKind::Song, target, quality, facts.clone());

        // 1. Already shared.
        if let Some(song) = &song
            && let Some(copy) = self.catalog.shared_copy(song).await
        {
            if quality != RequestQuality::Flac || copy.lossless {
                request.state = RequestState::Done;
                request.outcome = Some(RequestOutcome::AlreadyShared);
                request.library_song_id = copy.navidrome_id.clone();
                let made = self.requests.create(request, Utc::now());
                if let Some(id) = &copy.navidrome_id {
                    self.stars.star(&member.username, id).await;
                }
                info!(
                    "FAMILY request {} {} {}: already shared",
                    made.id, made.username, made.title
                );
                self.events.send(FamilyEvent::RequestDone(made.clone()));
                return Ok(made);
            }
            // A FLAC asked for a song the shared library has only lossy: a better copy of it.
            if let Some(id) = &copy.navidrome_id {
                return self.create_upgrade(member, id, quality, Some(facts));
            }
        }

        // 2. Another member has it.
        if let Some(file) = self.pooled(outside, song.as_ref(), quality)
            && (file.refs.iter().any(|r| eq_ignore_case(r, &member.username)) || self.instant_allowed(member))
        {
            return match self.pool().link(&file.path, &member.username).await {
                Ok(_) => {
                    request.state = RequestState::Done;
                    request.outcome = Some(RequestOutcome::AddedFromFamily);
                    let made = self.requests.create(request, Utc::now());
                    info!(
                        "FAMILY request {} {}: added from the family library",
                        made.id, made.username
                    );
                    self.events.send(FamilyEvent::RequestDone(made.clone()));
                    Ok(made)
                }
                Err(e) => Err(RequestError::new(0, e.to_string())),
            };
        }

        // 3. Downloading for someone now.
        if quality != RequestQuality::Flac
            && self.fetcher.in_flight(&outside.provider, &outside.external_id)
            && self.instant_allowed(member)
        {
            request.state = RequestState::Approved;
            let made = self.requests.create(request, Utc::now());
            info!(
                "FAMILY request {} {}: joins a download in flight",
                made.id, made.username
            );
            self.start(made.clone(), true);
            return Ok(made);
        }

        // 4. Ask.
        self.decide_new(member, request)
    }

    /// A pool file holding the song at a quality that will do.
    fn pooled(
        &self,
        outside: &OutsideTarget,
        song: Option<&Song>,
        quality: RequestQuality,
    ) -> Option<super::family_pool_store::PoolFile> {
        let key = key_of(&outside.provider, &outside.external_id);
        let (isrc, artist, title) = song
            .map(|s| (s.isrc.as_deref(), s.artist.as_str(), s.title.as_str()))
            .unwrap_or((None, "", ""));
        self.pool()
            .find(Some(&key), isrc, artist, title)
            .filter(|file| quality != RequestQuality::Flac || file.format == "flac")
    }

    async fn create_album(
        &self,
        member: &Member,
        target: &str,
        outside: &OutsideTarget,
        quality: RequestQuality,
        facts: Option<RequestFacts>,
    ) -> Result<FamilyRequest, RequestError> {
        let album = self.catalog.album(&outside.provider, &outside.external_id).await;
        let facts = facts.unwrap_or_else(|| RequestFacts {
            title: album.as_ref().map(|a| a.title.clone()).unwrap_or_default(),
            artist: album.as_ref().map(|a| a.artist.clone()).unwrap_or_default(),
            album: album.as_ref().map(|a| a.title.clone()).unwrap_or_default(),
            cover_art: target.to_string(),
        });
        let mut request = self.new_request(member, RequestKind::Album, target, quality, facts);
        // Every song already shared: nothing to fetch.
        if let Some(album) = &album
            && !album.songs.is_empty()
        {
            let mut all_shared = true;
            for song in &album.songs {
                if self.catalog.shared_copy(song).await.is_none() {
                    all_shared = false;
                    break;
                }
            }
            if all_shared {
                request.state = RequestState::Done;
                request.outcome = Some(RequestOutcome::AlreadyShared);
                let made = self.requests.create(request, Utc::now());
                self.events.send(FamilyEvent::RequestDone(made.clone()));
                return Ok(made);
            }
        }
        self.decide_new(member, request)
    }

    fn create_upgrade(
        &self,
        member: &Member,
        navidrome_id: &str,
        quality: RequestQuality,
        facts: Option<RequestFacts>,
    ) -> Result<FamilyRequest, RequestError> {
        let index = self.pool().index();
        if let Some(library) = index.library_of(navidrome_id)
            && self
                .family
                .members()
                .iter()
                .any(|m| m.personal_library_id == Some(library))
        {
            return Err(RequestError::new(
                50,
                "Ask for a better copy of a shared song only.",
            ));
        }
        let request = self.new_request(
            member,
            RequestKind::Upgrade,
            navidrome_id,
            quality,
            facts.unwrap_or_default(),
        );
        self.decide_new(member, request)
    }

    /// Pending, approved at once, or refused, by the member's abilities, their weekly limit
    /// and their storage.
    fn decide_new(&self, member: &Member, mut request: FamilyRequest) -> Result<FamilyRequest, RequestError> {
        let abilities = member.abilities();
        let used = self.requests.quota_used(&member.username, Utc::now());
        let storage = self.personal.storage_used(&member.username);
        // A playlist addition always waits for whoever decides on that playlist.
        let decision = if request.kind == RequestKind::PlaylistAdd {
            RequestDecision::Pending
        } else {
            policy::request_decision(&abilities, request.quality, used, storage)
        };
        match decision {
            RequestDecision::Refuse(message) => Err(RequestError::new(50, message)),
            RequestDecision::AutoApprove => {
                request.state = RequestState::Approved;
                let made = self.requests.create(request, Utc::now());
                info!(
                    "FAMILY request {} {} {:?} \"{}\" {:?} -> AutoApproved",
                    made.id, made.username, made.kind, made.title, made.quality
                );
                self.start(made.clone(), false);
                Ok(made)
            }
            RequestDecision::Pending => {
                if let Some(full) = storage_full(&abilities, storage) {
                    request.note = full;
                }
                let made = self.requests.create(request, Utc::now());
                info!(
                    "FAMILY request {} {} {:?} \"{}\" {:?} -> Pending",
                    made.id, made.username, made.kind, made.title, made.quality
                );
                self.events.send(FamilyEvent::RequestCreated(made.clone()));
                Ok(made)
            }
        }
    }

    /// Whether `by` may decide requests: the owner, or a member with `ApproveRequests`.
    fn may_decide(&self, by: &str) -> bool {
        self.family.is_owner(by)
            || self
                .family
                .member(by)
                .is_some_and(|m| !m.suspended && m.abilities().approve_requests)
    }

    /// Approves a waiting request; it is fetched now. An approver may approve past the member's
    /// storage limit or weekly limit.
    pub fn approve(&self, id: &str, by: &str, note: Option<&str>) -> Result<FamilyRequest, RequestError> {
        if !self.may_decide(by) {
            return Err(RequestError::new(50, "Only approvers decide requests."));
        }
        let request = self
            .requests
            .get(id)
            .ok_or_else(|| RequestError::new(70, "There is no such request."))?;
        if request.state != RequestState::Pending {
            return Err(RequestError::new(50, "This request was already decided."));
        }
        let decided = self
            .requests
            .update(id, |r| {
                r.state = RequestState::Approved;
                r.decided_by = Some(by.to_string());
                r.decided = Some(Utc::now());
                if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
                    r.note = note.trim().to_string();
                }
            })
            .ok_or_else(|| RequestError::new(70, "There is no such request."))?;
        info!("FAMILY request {id} approved by {by}");
        self.events.send(FamilyEvent::RequestDecided(decided.clone()));
        self.start(decided.clone(), false);
        Ok(decided)
    }

    /// Declines a waiting request, with an optional note for the member. It does not count
    /// toward their weekly limit.
    pub fn decline(&self, id: &str, by: &str, note: Option<&str>) -> Result<FamilyRequest, RequestError> {
        if !self.may_decide(by) {
            return Err(RequestError::new(50, "Only approvers decide requests."));
        }
        let request = self
            .requests
            .get(id)
            .ok_or_else(|| RequestError::new(70, "There is no such request."))?;
        if request.state != RequestState::Pending {
            return Err(RequestError::new(50, "This request was already decided."));
        }
        let decided = self
            .requests
            .update(id, |r| {
                r.state = RequestState::Declined;
                r.decided_by = Some(by.to_string());
                r.decided = Some(Utc::now());
                r.note = note.map(str::trim).unwrap_or_default().to_string();
            })
            .ok_or_else(|| RequestError::new(70, "There is no such request."))?;
        info!("FAMILY request {id} declined by {by}");
        self.events.send(FamilyEvent::RequestDecided(decided.clone()));
        Ok(decided)
    }

    /// Takes back one of the member's own waiting requests.
    pub fn cancel(&self, id: &str, by_member: &str) -> Result<FamilyRequest, RequestError> {
        let request = self
            .requests
            .get(id)
            .filter(|r| eq_ignore_case(&r.username, by_member))
            .ok_or_else(|| RequestError::new(70, "There is no such request."))?;
        if request.state != RequestState::Pending {
            return Err(RequestError::new(50, "Only a waiting request can be taken back."));
        }
        self.requests
            .update(id, |r| r.state = RequestState::Cancelled)
            .ok_or_else(|| RequestError::new(70, "There is no such request."))
    }

    /// Asks again for an MP3 of a song whose FLAC request failed.
    pub async fn retry_as_mp3(&self, id: &str, by_member: &str) -> Result<FamilyRequest, RequestError> {
        let request = self
            .requests
            .get(id)
            .filter(|r| eq_ignore_case(&r.username, by_member))
            .ok_or_else(|| RequestError::new(70, "There is no such request."))?;
        if request.state != RequestState::Failed || request.quality != RequestQuality::Flac {
            return Err(RequestError::new(
                50,
                "Only a FLAC request that failed can be tried as MP3.",
            ));
        }
        let facts = RequestFacts {
            title: request.title.clone(),
            artist: request.artist.clone(),
            album: request.album.clone(),
            cover_art: request.cover_art.clone(),
        };
        self.create_with(
            by_member,
            Some(request.kind),
            &request.target,
            RequestQuality::Mp3,
            Some(facts),
        )
        .await
    }

    /// Fetches an approved request on a task of its own.
    fn start(&self, request: FamilyRequest, joined: bool) {
        let Some(me) = self.me.upgrade() else { return };
        tokio::spawn(async move { me.run(request, joined).await });
    }

    async fn run(&self, request: FamilyRequest, joined: bool) {
        match request.kind {
            RequestKind::Song => self.run_song(&request, joined).await,
            RequestKind::Album => self.run_album(&request).await,
            RequestKind::Upgrade => self.run_upgrade(&request).await,
            RequestKind::Import => match self.fetcher.import(&request.username, &request.target).await {
                Ok(_) => self.done(&request, Some(RequestOutcome::Downloaded), None),
                Err(reason) => self.failed(&request, &reason),
            },
            RequestKind::PlaylistAdd => self.done(&request, None, None),
        }
    }

    async fn run_song(&self, request: &FamilyRequest, joined: bool) {
        let Some(outside) = self.catalog.outside(&request.target) else {
            self.failed(request, "That song was not found.");
            return;
        };
        let user = request.username.clone();
        let song = self.catalog.song(&outside.provider, &outside.external_id).await;
        // Someone may have got it since it was asked for.
        if let Some(file) = self.pooled(&outside, song.as_ref(), request.quality) {
            match self.pool().link(&file.path, &user).await {
                Ok(link) => {
                    self.wait_done(request, &link, RequestOutcome::AddedFromFamily)
                        .await;
                    return;
                }
                Err(e) => warn!("FAMILY request {} could not link {}: {e}", request.id, file.path),
            }
        }
        let fetched = self
            .fetcher
            .song(
                &user,
                &outside.provider,
                &outside.external_id,
                request.quality,
                song.as_ref(),
            )
            .await;
        let outcome = if joined {
            RequestOutcome::AddedFromFamily
        } else {
            RequestOutcome::Downloaded
        };
        match fetched {
            Fetched::Landed(path) => match self.pool().link_path(&path, &user) {
                Some(_) => match self.pool().link(&path, &user).await {
                    Ok(link) => self.wait_done(request, &link, outcome).await,
                    Err(e) => self.failed(request, &e.to_string()),
                },
                // Someone wanted it shared, so it is in the shared library for everyone.
                None => self.wait_done(request, &path, outcome).await,
            },
            Fetched::AlreadyThere => self.done(request, Some(RequestOutcome::AlreadyShared), None),
            Fetched::Failed(reason) => {
                let reason = if request.quality == RequestQuality::Flac {
                    NO_REAL_FLAC.to_string()
                } else {
                    reason
                };
                self.failed(request, &reason);
            }
        }
    }

    async fn run_album(&self, request: &FamilyRequest) {
        let Some(outside) = self.catalog.outside(&request.target) else {
            self.failed(request, "That album was not found.");
            return;
        };
        let landed = self
            .fetcher
            .album(&request.username, &outside.provider, &outside.external_id)
            .await;
        let Some(first) = landed.first() else {
            self.failed(request, "None of the album's songs could be found.");
            return;
        };
        let shown = self
            .pool()
            .link_path(first, &request.username)
            .unwrap_or_else(|| first.clone());
        self.wait_done(request, &shown, RequestOutcome::Downloaded).await;
    }

    async fn run_upgrade(&self, request: &FamilyRequest) {
        let facts = RequestFacts {
            title: request.title.clone(),
            artist: request.artist.clone(),
            album: request.album.clone(),
            cover_art: request.cover_art.clone(),
        };
        let owner = self.family.owner();
        if !self.fetcher.upgrade(&owner, &request.target, &facts) {
            self.failed(
                request,
                "The better copy could not be asked for now. Try again later.",
            );
            return;
        }
        let started = tokio::time::Instant::now();
        loop {
            if let Some(result) = self.fetcher.upgrade_result(&request.target) {
                match result {
                    Ok(()) => self.done(request, Some(RequestOutcome::Downloaded), None),
                    Err(reason) => self.failed(request, &reason),
                }
                return;
            }
            if started.elapsed() >= self.timing.upgrade_wait {
                self.failed(request, "No better copy came in time.");
                return;
            }
            tokio::time::sleep(self.timing.poll).await;
        }
    }

    /// Done once the library index finds `path` in the member's own library (anywhere, for a
    /// shared path). A link Navidrome never shows means links do not work there: the member
    /// gets a copy instead and Octo looks again.
    async fn wait_done(&self, request: &FamilyRequest, path: &str, outcome: RequestOutcome) {
        let pool = self.pool();
        let library = pool
            .pool_path_of_link(&request.username, path)
            .and_then(|_| self.personal.library_id(&request.username));
        if let Some(song) = pool.wait_for_path(path, library).await {
            self.stars.star(&request.username, &song.id).await;
            self.done(request, Some(outcome), Some(song.id));
            return;
        }
        let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        if is_link && pool.mode() == LinkMode::Symlink && !pool.links_seen() {
            pool.fall_back_to_copies();
            if let Some(library) = library {
                pool.index().note_placed(
                    library,
                    path,
                    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                );
            }
            if let Some(song) = pool.wait_for_path(path, library).await {
                self.stars.star(&request.username, &song.id).await;
                self.done(request, Some(outcome), Some(song.id));
                return;
            }
        }
        self.failed(request, NOT_SHOWN_MESSAGE);
    }

    fn done(
        &self,
        request: &FamilyRequest,
        outcome: Option<RequestOutcome>,
        library_song_id: Option<String>,
    ) {
        let Some(done) = self.requests.update(&request.id, |r| {
            r.state = RequestState::Done;
            r.outcome = outcome;
            if library_song_id.is_some() {
                r.library_song_id = library_song_id.clone();
            }
        }) else {
            return;
        };
        info!("FAMILY request {} {} done ({outcome:?})", done.id, done.username);
        self.events.send(FamilyEvent::RequestDone(done));
    }

    fn failed(&self, request: &FamilyRequest, reason: &str) {
        let Some(failed) = self.requests.update(&request.id, |r| {
            r.state = RequestState::Failed;
            r.failure = Some(reason.to_string());
        }) else {
            return;
        };
        warn!(
            "FAMILY request {} {} failed: {reason}",
            failed.id, failed.username
        );
        self.events.send(FamilyEvent::RequestFailed(failed));
    }
}

/// A request's titles from the song it asks for.
fn song_facts(song: Option<&Song>, target: &str) -> RequestFacts {
    RequestFacts {
        title: song.map(|s| s.title.clone()).unwrap_or_default(),
        artist: song.map(|s| s.artist.clone()).unwrap_or_default(),
        album: song.map(|s| s.album.clone()).unwrap_or_default(),
        cover_art: target.to_string(),
    }
}

/// [`RequestCatalog`] over the catalog, the shared library's ownership check and Octo's ids.
pub struct LiveRequestCatalog {
    pub metadata: Arc<dyn IMusicMetadataService>,
    pub ownership: Arc<LibraryOwnership>,
    pub library: Arc<dyn ILocalLibraryService>,
    pub registry: Arc<ExternalIdRegistry>,
}

#[async_trait]
impl RequestCatalog for LiveRequestCatalog {
    fn outside(&self, id: &str) -> Option<OutsideTarget> {
        let (is_external, provider, external_id) = self.library.parse_song_id(id);
        let album = self
            .registry
            .lookup(id)
            .is_some_and(|routing| routing.snapshot().kind == RoutingKind::Album);
        if album {
            return Some(OutsideTarget {
                kind: RequestKind::Album,
                provider: provider.unwrap_or_else(|| SoulseekMetadataService::PROVIDER_NAME.to_string()),
                external_id: id.to_string(),
            });
        }
        is_external.then(|| OutsideTarget {
            kind: RequestKind::Song,
            provider: provider.unwrap_or_default(),
            external_id: external_id.unwrap_or_default(),
        })
    }

    async fn song(&self, provider: &str, external_id: &str) -> Option<Song> {
        self.metadata.get_song(provider, external_id).await
    }

    async fn album(&self, provider: &str, external_id: &str) -> Option<Album> {
        self.metadata.get_album(provider, external_id).await
    }

    async fn shared_copy(&self, song: &Song) -> Option<OwnedCopy> {
        self.ownership
            .find_in(
                &super::LibraryScope::Shared,
                Some(&song.artist),
                Some(&song.title),
                song.duration,
                Some(&song.album),
            )
            .await
    }
}

/// [`RequestFetcher`] over the acquisition queue, the heart chain, the upgrade queue and the
/// imports.
pub struct LiveRequestFetcher {
    pub queue: Arc<TrackAcquisitionQueue>,
    pub coordinator: Arc<HeartAcquisitionCoordinator>,
    pub tracker: Arc<AcquisitionTracker>,
    pub upgrades: Arc<UpgradeQueue>,
    imports: OnceLock<Weak<ImportService>>,
}

impl LiveRequestFetcher {
    pub fn new(
        queue: Arc<TrackAcquisitionQueue>,
        coordinator: Arc<HeartAcquisitionCoordinator>,
        tracker: Arc<AcquisitionTracker>,
        upgrades: Arc<UpgradeQueue>,
    ) -> Self {
        LiveRequestFetcher {
            queue,
            coordinator,
            tracker,
            upgrades,
            imports: OnceLock::new(),
        }
    }

    /// The imports an approved import request hands its list to.
    pub fn use_imports(&self, imports: &Arc<ImportService>) {
        let _ = self.imports.set(Arc::downgrade(imports));
    }
}

#[async_trait]
impl RequestFetcher for LiveRequestFetcher {
    fn in_flight(&self, provider: &str, external_id: &str) -> bool {
        self.queue.is_in_flight(provider, external_id)
    }

    async fn song(
        &self,
        user: &str,
        provider: &str,
        external_id: &str,
        quality: RequestQuality,
        song: Option<&Song>,
    ) -> Fetched {
        self.tracker.begin(
            provider,
            external_id,
            None,
            Some(user),
            song.map(|s| s.artist.as_str()),
            song.map(|s| s.title.as_str()),
            song.map(|s| s.album.as_str()),
        );
        let target = AcquisitionTarget::Member(user.to_string());
        let source = match quality {
            RequestQuality::Best => {
                return match self
                    .coordinator
                    .acquire_track_outcome(provider, external_id, Some(user), None, &target)
                    .await
                {
                    Ok(TrackOutcome::Landed(path)) => Fetched::Landed(path),
                    Ok(TrackOutcome::AlreadyThere) => Fetched::AlreadyThere,
                    Ok(TrackOutcome::HandedToLidarr) => {
                        Fetched::Failed("Lidarr took the song for the shared library.".into())
                    }
                    Ok(TrackOutcome::Failed(reason)) => Fetched::Failed(reason),
                    Err(e) => Fetched::Failed(e.to_string()),
                };
            }
            RequestQuality::Flac => DownloadSource::Soulseek,
            RequestQuality::Mp3 => DownloadSource::YouTube,
        };
        let completion = self.queue.enqueue_for(
            &target,
            quality == RequestQuality::Flac,
            provider,
            external_id,
            true,
            false,
            true,
            Some(source),
            true,
            Some(user),
            false,
            None,
        );
        match completion.wait().await {
            Ok(path) => Fetched::Landed(path),
            Err(e) => {
                let reason = e.to_string();
                self.tracker.fail(provider, external_id, Some(&reason));
                Fetched::Failed(reason)
            }
        }
    }

    async fn album(&self, user: &str, provider: &str, external_id: &str) -> Vec<String> {
        self.tracker.begin_album(provider, external_id, Some(user));
        self.coordinator
            .acquire_album_for_member(provider, external_id, Some(user), user, false)
            .await
    }

    fn upgrade(&self, owner: &str, navidrome_id: &str, facts: &RequestFacts) -> bool {
        let (jobs, _) = self.upgrades.add(
            vec![UpgradeAsk {
                navidrome_id: navidrome_id.to_string(),
                title: Some(facts.title.clone()).filter(|t| !t.is_empty()),
                artist: Some(facts.artist.clone()).filter(|a| !a.is_empty()),
                album: Some(facts.album.clone()).filter(|a| !a.is_empty()),
                suffix: None,
                attempt_key: None,
                pick: None,
            }],
            owner,
            "request",
        );
        !jobs.is_empty()
    }

    fn upgrade_result(&self, navidrome_id: &str) -> Option<Result<(), String>> {
        use crate::services::library::upgrade_queue::UpgradeStates;
        let job = self
            .upgrades
            .snapshot()
            .into_iter()
            .find(|job| job.navidrome_id == navidrome_id)?;
        if UpgradeStates::open(&job.state) {
            return None;
        }
        Some(if job.state == UpgradeStates::UPGRADED {
            Ok(())
        } else {
            Err(job
                .detail
                .unwrap_or_else(|| "No better copy was found.".to_string()))
        })
    }

    async fn import(&self, user: &str, list_id: &str) -> Result<String, String> {
        let Some(imports) = self.imports.get().and_then(Weak::upgrade) else {
            return Err("Imports are not running.".to_string());
        };
        let result = imports.approve_family_list(user, list_id);
        if result.ok {
            Ok(result.message)
        } else {
            Err(result.message)
        }
    }
}

#[cfg(test)]
#[path = "request_service_tests.rs"]
mod tests;
