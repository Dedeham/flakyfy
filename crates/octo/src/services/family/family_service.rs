//! What the Family page and the apps ask of Octo Family, in one place: members' lifecycle
//! (add, adopt, edit, suspend, remove, invite, repair), devices (add, pair, sign out),
//! turning Family on, and the answers the page and the `octoFamily` calls share.
//!
//! Work that belongs to other parts of Octo is reached through small traits gathered in
//! [`FamilyHooks`]: the members' folders and files, requests, storage use, the personal library,
//! saved songs, family playlists, live streams and the explicit marks. [`FamilyHooks::standard`]
//! puts each over the service that does the work ([`super::hooks`]); the state installs them
//! once it is built, and tests replace any of them with [`FamilyService::wire`].

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use octo_core::common::Clock;
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::family::abilities::RequestQuality;
use octo_core::family::abilities::{Abilities, Preset, Role};
use octo_core::family::codes::{app_password, device_secret, pair_code, token};
use octo_core::family::device::{DEVICE_NAME_LIMIT, Device, DeviceKind, NetworkPlace};
use octo_core::family::quality::DeviceQualityMode;
use octo_core::family::request::{FamilyRequest, RequestKind, RequestState};
use octo_core::json::datetime;
use octo_core::notifications::{NotificationEvent, NotificationEventType};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tracing::{info, warn};

use super::device_registry::NewDevice;
use super::family_store::{Member, PairOutcome, SharedLibraries};
use super::navidrome_admin::{AdminError, NavidromeAdmin, NdLibrary, NdUser};
use super::passwords;
use super::request_service::FamilyEvent;
use crate::app::AppState;
use crate::services::admin::SignInThrottle;
use crate::services::you_tube::ShimBusy;

// ------------------------------------------------------------------------------------------
// Errors
// ------------------------------------------------------------------------------------------

/// Why a family action did not happen, with the words the person reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FamilyError {
    /// Something in the request is missing or wrong.
    Invalid(String),
    /// Nothing by that name or id.
    NotFound(String),
    /// Not allowed for this person.
    Refused(String),
    /// Clashes with how things stand (a name taken, Navidrome cannot see a folder).
    Conflict(String),
    /// A part of Octo or Navidrome that is needed is not there right now.
    Unavailable(String),
    /// Too many wrong tries; wait this long.
    Throttled(String, TimeDelta),
}

impl FamilyError {
    pub fn message(&self) -> &str {
        match self {
            FamilyError::Invalid(m)
            | FamilyError::NotFound(m)
            | FamilyError::Refused(m)
            | FamilyError::Conflict(m)
            | FamilyError::Unavailable(m)
            | FamilyError::Throttled(m, _) => m,
        }
    }

    /// The Subsonic error code for this error: 10 missing or wrong, 50 not allowed, 70 not
    /// found, 0 for the rest.
    pub fn subsonic_code(&self) -> i32 {
        match self {
            FamilyError::Invalid(_) => 10,
            FamilyError::Refused(_) | FamilyError::Conflict(_) => 50,
            FamilyError::NotFound(_) => 70,
            FamilyError::Unavailable(_) | FamilyError::Throttled(..) => 0,
        }
    }
}

impl std::fmt::Display for FamilyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl From<AdminError> for FamilyError {
    fn from(e: AdminError) -> Self {
        match e {
            AdminError::NoIdentity | AdminError::NoServer => FamilyError::Unavailable(e.to_string()),
            other => FamilyError::Unavailable(format!("Navidrome could not do that: {other}")),
        }
    }
}

pub type FamilyResult<T> = Result<T, FamilyError>;

// ------------------------------------------------------------------------------------------
// Hooks: what other parts of Octo do for the family
// ------------------------------------------------------------------------------------------

/// Navidrome's accounts and libraries, as Octo's admin identity changes them.
#[async_trait]
pub trait MemberAccounts: Send + Sync {
    async fn users(&self) -> Result<Vec<NdUser>, AdminError>;
    /// Makes a user who is not an admin; answers their id.
    async fn create_user(&self, user_name: &str, name: &str, password: &str) -> Result<String, AdminError>;
    async fn delete_user(&self, id: &str) -> Result<(), AdminError>;
    async fn set_password(&self, id: &str, password: &str) -> Result<(), AdminError>;
    async fn libraries(&self) -> Result<Vec<NdLibrary>, AdminError>;
    /// Makes a library over a folder Navidrome can see; answers its id.
    async fn create_library(&self, name: &str, path: &str) -> Result<i64, AdminError>;
    async fn delete_library(&self, id: i64) -> Result<(), AdminError>;
    async fn set_user_libraries(&self, user_id: &str, library_ids: &[i64]) -> Result<(), AdminError>;
}

#[async_trait]
impl MemberAccounts for NavidromeAdmin {
    async fn users(&self) -> Result<Vec<NdUser>, AdminError> {
        self.list_users().await
    }

    async fn create_user(&self, user_name: &str, name: &str, password: &str) -> Result<String, AdminError> {
        NavidromeAdmin::create_user(self, user_name, name, password).await
    }

    async fn delete_user(&self, id: &str) -> Result<(), AdminError> {
        NavidromeAdmin::delete_user(self, id).await
    }

    async fn set_password(&self, id: &str, password: &str) -> Result<(), AdminError> {
        NavidromeAdmin::set_password(self, id, password).await
    }

    async fn libraries(&self) -> Result<Vec<NdLibrary>, AdminError> {
        self.list_libraries().await
    }

    async fn create_library(&self, name: &str, path: &str) -> Result<i64, AdminError> {
        NavidromeAdmin::create_library(self, name, path).await
    }

    async fn delete_library(&self, id: i64) -> Result<(), AdminError> {
        NavidromeAdmin::delete_library(self, id).await
    }

    async fn set_user_libraries(&self, user_id: &str, library_ids: &[i64]) -> Result<(), AdminError> {
        NavidromeAdmin::set_user_libraries(self, user_id, library_ids).await
    }
}

pub use super::pool::{FilesChoice, LinkMode};

/// Members' folders on disk.
#[async_trait]
pub trait PersonalFolders: Send + Sync {
    /// Makes the member's folder (readable by Navidrome at the same path) and answers its full
    /// path. Making one that is already there is fine.
    async fn create(&self, username: &str) -> Result<String, String>;
    /// Removes a member's folder when it holds nothing (after an add that did not finish).
    async fn remove_if_empty(&self, username: &str);
    /// Makes a link in the members folder, reads through it and removes it.
    async fn probe_links(&self) -> LinkMode;
    /// How members' libraries get files now, when it can change after the probe (Navidrome
    /// never showing a linked song switches to copies).
    fn mode(&self) -> Option<LinkMode> {
        None
    }
}

/// A removed member's files.
#[async_trait]
pub trait MemberFiles: Send + Sync {
    /// Deletes the member's links and handles the files only they had, by `choice`.
    async fn release(&self, username: &str, choice: FilesChoice) -> Result<(), FamilyError>;
}

/// What a member asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRequest {
    pub username: String,
    /// An outside song or album id, or a library song id for a better copy.
    pub target: String,
    pub quality: RequestQuality,
    /// None works it out from the id.
    pub kind: Option<RequestKind>,
}

/// Requests for a copy: making, canceling and deciding them, and asking again for an MP3 when a
/// FLAC could not be found. Listing reads the request store.
#[async_trait]
pub trait RequestService: Send + Sync {
    async fn create(&self, ask: NewRequest) -> Result<FamilyRequest, FamilyError>;
    /// The requester's own pending request, canceled.
    async fn cancel(&self, id: &str, by: &str) -> Result<FamilyRequest, FamilyError>;
    /// Approves or declines a request, with a note for the member.
    async fn decide(
        &self,
        id: &str,
        approve: bool,
        by: &str,
        note: &str,
    ) -> Result<FamilyRequest, FamilyError>;
    /// The requester's own failed FLAC request, asked for again as an MP3.
    async fn retry_as_mp3(&self, id: &str, by: &str) -> Result<FamilyRequest, FamilyError>;
}

/// How much a member's own library holds.
pub trait StorageUsage: Send + Sync {
    fn used_bytes(&self, username: &str) -> u64;
}

/// One song in a member's own library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MySong {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub size: u64,
    /// When it was added, if known.
    pub added: Option<String>,
}

/// A page of a member's own library.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MyLibraryPage {
    pub songs: Vec<MySong>,
    pub total: usize,
}

/// A member's own library: what is in it, and taking a song out.
#[async_trait]
pub trait RemoveFromMyLibrary: Send + Sync {
    fn list(&self, username: &str, offset: usize, size: usize) -> MyLibraryPage;
    /// Takes one of the member's own songs out of their library. Refused for shared songs.
    async fn remove(&self, username: &str, song_id: &str) -> Result<(), FamilyError>;
}

/// What a member saved: outside songs and albums.
#[async_trait]
pub trait SavedItems: Send + Sync {
    /// `{songs: [...], albums: [...]}`.
    fn list(&self, username: &str) -> Value;
    /// Forgets one saved song or album. Answers whether it was saved.
    async fn remove(&self, username: &str, id: &str) -> bool;
}

/// One family playlist as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyPlaylistSummary {
    pub id: String,
    pub name: String,
    pub song_count: usize,
    pub approve_additions: bool,
    pub created_by: String,
    pub created: String,
}

/// Playlists every member can add to. Each change is checked for the person making it.
#[async_trait]
pub trait FamilyPlaylists: Send + Sync {
    /// The playlists `viewer` sees, each with the count of songs they are shown.
    fn list(&self, viewer: &str) -> Vec<FamilyPlaylistSummary>;
    fn get(&self, id: &str, viewer: &str) -> Option<FamilyPlaylistSummary>;
    async fn create(
        &self,
        name: &str,
        by: &str,
        approve_additions: bool,
    ) -> Result<FamilyPlaylistSummary, FamilyError>;
    async fn update(
        &self,
        id: &str,
        by: &str,
        name: Option<&str>,
        approve_additions: Option<bool>,
    ) -> Result<FamilyPlaylistSummary, FamilyError>;
    async fn delete(&self, id: &str, by: &str) -> Result<(), FamilyError>;
}

/// What a device is playing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playing {
    pub song_id: String,
    pub title: String,
    pub artist: String,
}

/// One live stream on the load panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadStream {
    pub username: String,
    pub device: String,
    pub song_title: String,
    pub kbps: u32,
    pub place: NetworkPlace,
}

/// The outside-song helper's slots, from its health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ShimLoad {
    /// Outside songs streaming now.
    pub active: u32,
    /// How many yt-dlp lookups may run at once.
    pub limit: u32,
    /// yt-dlp lookups running now.
    pub ytdlp_running: u32,
}

impl From<ShimBusy> for ShimLoad {
    fn from(busy: ShimBusy) -> Self {
        ShimLoad {
            active: busy.active_streams,
            limit: busy.ytdlp_limit,
            ytdlp_running: busy.ytdlp_running,
        }
    }
}

/// Downloads waiting, per member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueLoad {
    pub username: String,
    pub waiting: u32,
}

/// The load panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoadReport {
    pub streams: Vec<LoadStream>,
    /// None when the helper did not answer.
    pub shim: Option<ShimLoad>,
    pub queue: Vec<QueueLoad>,
    pub upload_kbps: u32,
}

/// What is playing right now.
pub trait LiveStreams: Send + Sync {
    fn playing(&self, device_id: &str) -> Option<Playing>;
    /// Whether anything of this member's is playing.
    fn user_playing(&self, username: &str) -> bool;
    /// How many streams are live.
    fn count(&self) -> usize;
    fn load(&self) -> LoadReport;
}

/// Shared songs that carry no explicit mark, which kid profiles cannot judge.
#[async_trait]
pub trait UnratedCount: Send + Sync {
    /// How many shared songs have no explicit mark; None while it is not known.
    fn unrated(&self) -> Option<u64>;
    /// Starts marking them; answers what happens next, in plain words.
    async fn mark_now(&self, by: &str) -> Result<String, FamilyError>;
}

/// Every hook, as installed.
#[derive(Clone)]
pub struct FamilyHooks {
    pub accounts: Arc<dyn MemberAccounts>,
    pub folders: Arc<dyn PersonalFolders>,
    pub files: Arc<dyn MemberFiles>,
    pub requests: Arc<dyn RequestService>,
    pub storage: Arc<dyn StorageUsage>,
    pub my_library: Arc<dyn RemoveFromMyLibrary>,
    pub saved: Arc<dyn SavedItems>,
    pub playlists: Arc<dyn FamilyPlaylists>,
    pub streams: Arc<dyn LiveStreams>,
    pub unrated: Arc<dyn UnratedCount>,
}

impl FamilyHooks {
    /// The hooks Octo runs with: each over the service on `state` that does the work.
    pub fn standard(state: &AppState) -> Self {
        use super::hooks::{
            IndexUnrated, LiveRequests, PersonalSongs, PersonalStorage, PoolFiles, PoolFolders,
            RulePlaylists, TrackerStreams,
        };
        FamilyHooks {
            accounts: state.navidrome_admin.clone(),
            folders: Arc::new(PoolFolders {
                personal: state.personal_library.clone(),
            }),
            files: Arc::new(PoolFiles {
                pool: state.family_pool_service.clone(),
            }),
            requests: Arc::new(LiveRequests {
                service: state.family_request_service.clone(),
            }),
            storage: Arc::new(PersonalStorage {
                personal: state.personal_library.clone(),
            }),
            my_library: Arc::new(PersonalSongs {
                personal: state.personal_library.clone(),
            }),
            saved: Arc::new(StoreSaved {
                saved: state.family_saved.clone(),
            }),
            playlists: Arc::new(RulePlaylists {
                store: state.family_playlists.clone(),
                family: state.family_store.clone(),
            }),
            streams: Arc::new(TrackerStreams {
                tracker: state.family_audio.tracker().clone(),
                index: state.library_index.clone(),
                registry: state.external_id_registry.clone(),
                acquisitions: state.acquisition_tracker.clone(),
            }),
            unrated: Arc::new(IndexUnrated {
                index: state.library_index.clone(),
                scope: state.library_scope.clone(),
                backfill: state.explicit_backfill.clone(),
            }),
        }
    }
}

/// Makes a folder (and its parents) that Navidrome's user can read and the group can write.
fn make_folder(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o775))?;
    }
    Ok(())
}

/// Saved songs and albums, from `family-saved.json`.
pub struct StoreSaved {
    saved: Arc<super::SavedStore>,
}

#[async_trait]
impl SavedItems for StoreSaved {
    fn list(&self, username: &str) -> Value {
        let songs: Vec<Value> = self
            .saved
            .list(username)
            .iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "title": s.title,
                    "artist": s.artist,
                    "album": s.album,
                    "coverArt": s.cover_art,
                    "duration": s.duration,
                    "explicit": s.explicit == 1,
                    "saved": datetime::format_utc(&s.saved),
                })
            })
            .collect();
        let albums: Vec<Value> = self
            .saved
            .albums(username)
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "name": a.name,
                    "artist": a.artist,
                    "coverArt": a.cover_art,
                    "songCount": a.song_count,
                    "year": a.year,
                    "saved": datetime::format_utc(&a.saved),
                })
            })
            .collect();
        json!({ "songs": songs, "albums": albums })
    }

    async fn remove(&self, username: &str, id: &str) -> bool {
        self.saved.remove(username, id)
    }
}

// ------------------------------------------------------------------------------------------
// Throttles
// ------------------------------------------------------------------------------------------

/// Wrong pairing codes Octo takes in [`PAIR_CAP_WINDOW`] while any code is live, from everyone
/// together, before it stops taking codes at all.
pub const PAIR_CAP: usize = 30;
pub const PAIR_CAP_WINDOW: TimeDelta = TimeDelta::minutes(10);
/// Wrong pairing codes from one address in [`PAIR_ADDRESS_WINDOW`] before it waits.
pub const PAIR_ADDRESS_FAILURES: usize = 10;
pub const PAIR_ADDRESS_WINDOW: TimeDelta = TimeDelta::minutes(15);
/// Wrong pairing codes from one peer Octo can vouch for (whatever hops it names) in
/// [`PAIR_ADDRESS_WINDOW`] before it waits. A peer may stand for several people, so it is higher.
pub const PAIR_PEER_FAILURES: usize = 50;
/// Wrong pairing codes for one username, from every address together, in
/// [`PAIR_ADDRESS_WINDOW`] before it waits.
pub const PAIR_ACCOUNT_FAILURES: usize = 20;
/// Addresses whose wrong pairing codes are kept at most.
const PAIR_ADDRESSES: usize = 10_000;

/// The words for every refused pairing code, so a wrong, used, expired or locked code read
/// alike.
pub const PAIR_REFUSED: &str = "That code did not work. Ask for a new one.";

/// Counts wrong pairing codes per address, per peer, per username and for everyone together.
pub struct PairGuard {
    clock: Clock,
    by_address: Mutex<HashMap<String, VecDeque<DateTime<Utc>>>>,
    everyone: Mutex<VecDeque<DateTime<Utc>>>,
}

impl PairGuard {
    pub fn new(clock: Clock) -> Self {
        PairGuard {
            clock,
            by_address: Mutex::new(HashMap::new()),
            everyone: Mutex::new(VecDeque::new()),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Whether `address` has had too many wrong codes lately.
    pub fn address_blocked(&self, address: &str) -> bool {
        let now = self.now();
        let mut map = self.by_address.lock();
        let Some(list) = map.get_mut(address) else {
            return false;
        };
        while list.front().is_some_and(|at| now - *at >= PAIR_ADDRESS_WINDOW) {
            list.pop_front();
        }
        list.len() >= PAIR_ADDRESS_FAILURES
    }

    /// Whether a pairing from `address` behind `peer` for `username` has to wait: too many wrong
    /// codes from that address, that peer, or for that username lately.
    pub fn blocked(&self, address: &str, peer: &str, username: &str) -> bool {
        let now = self.now();
        let mut map = self.by_address.lock();
        pair_keys(address, peer, username)
            .into_iter()
            .any(|(key, ceiling)| {
                let Some(list) = map.get_mut(&key) else {
                    return false;
                };
                while list.front().is_some_and(|at| now - *at >= PAIR_ADDRESS_WINDOW) {
                    list.pop_front();
                }
                list.len() >= ceiling
            })
    }

    /// One more wrong code from `address` behind `peer` for `username`.
    pub fn failed_from(&self, address: &str, peer: &str, username: &str) {
        let keys = pair_keys(address, peer, username);
        let now = self.now();
        {
            let mut map = self.by_address.lock();
            for (key, _) in &keys[1..] {
                map.entry(key.clone()).or_default().push_back(now);
            }
        }
        self.failed(&keys[0].0);
    }

    /// Whether everyone together has had [`PAIR_CAP`] wrong codes in the window.
    pub fn capped(&self) -> bool {
        let now = self.now();
        let mut everyone = self.everyone.lock();
        while everyone.front().is_some_and(|at| now - *at >= PAIR_CAP_WINDOW) {
            everyone.pop_front();
        }
        everyone.len() >= PAIR_CAP
    }

    pub fn failed(&self, address: &str) {
        let now = self.now();
        {
            let mut map = self.by_address.lock();
            if map.len() > PAIR_ADDRESSES {
                map.retain(|_, list| list.back().is_some_and(|at| now - *at < PAIR_ADDRESS_WINDOW));
                crate::services::admin::sign_in_throttle::trim_oldest(&mut map, PAIR_ADDRESSES, |list| {
                    list.back().copied()
                });
            }
            map.entry(address.to_string()).or_default().push_back(now);
        }
        let mut everyone = self.everyone.lock();
        everyone.push_back(now);
        while everyone.len() > PAIR_CAP * 4 {
            everyone.pop_front();
        }
    }
}

/// The keys a pairing try counts under, each with its ceiling; the address comes first.
fn pair_keys(address: &str, peer: &str, username: &str) -> [(String, usize); 3] {
    [
        (address.to_string(), PAIR_ADDRESS_FAILURES),
        (format!("peer|{peer}"), PAIR_PEER_FAILURES),
        (
            format!("account|{}", username.trim().to_lowercase()),
            PAIR_ACCOUNT_FAILURES,
        ),
    ]
}

// ------------------------------------------------------------------------------------------
// The service
// ------------------------------------------------------------------------------------------

/// Who is acting: the person signed in, as the family sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub user: String,
    pub role: Role,
}

impl Actor {
    pub fn is_owner(&self) -> bool {
        self.role == Role::Owner
    }
}

/// A new member as the manager describes them.
#[derive(Debug, Clone, Default)]
pub struct AddMember {
    pub username: String,
    pub display_name: String,
    pub preset: Option<Preset>,
    pub shared_libraries: Option<SharedLibraries>,
    /// Bring an existing Navidrome account into the family (its old sign-ins stop).
    pub adopt: bool,
}

/// Changes to a member.
#[derive(Debug, Clone, Default)]
pub struct EditMember {
    pub display_name: Option<String>,
    pub preset: Option<Preset>,
    pub edits: Option<Map<String, Value>>,
    pub shared_libraries: Option<SharedLibraries>,
    pub suspended: Option<bool>,
}

/// A device added from the page or an app: shown once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAdded {
    pub device_id: Option<String>,
    pub kind: DeviceKind,
    pub app_password: Option<String>,
    pub pair_code: Option<String>,
    pub expires: Option<String>,
    pub server: String,
    pub username: String,
}

/// A paired Octo app (or another app given an app password from a code).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paired {
    pub username: String,
    pub secret: String,
    pub device_id: String,
    pub server: String,
}

/// Turning Family on or off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableReport {
    pub enabled: bool,
    pub members_path: String,
    pub links: LinkMode,
    pub message: String,
}

/// The words when Navidrome cannot see a member's folder.
pub fn cannot_see_members_folder(members_path: &str) -> String {
    format!(
        "Navidrome cannot see the members folder. Add the same {members_path} volume to the navidrome service."
    )
}

/// The words for copy mode.
pub fn copy_mode_message(members_path: &str) -> String {
    format!(
        "This disk cannot link files, so a song two members want takes space twice. To fix it, start the rclone mount for {members_path} with --vfs-links."
    )
}

/// The service behind the Family page and the `octoFamily` calls.
pub struct FamilyService {
    hooks: RwLock<Option<FamilyHooks>>,
    /// Wrong sign-ins on the Family page, per address.
    pub sign_in_throttle: SignInThrottle,
    /// Wrong invite links, per address.
    pub join_throttle: SignInThrottle,
    pub pair_guard: PairGuard,
    /// What the last link probe found.
    links: RwLock<Option<LinkMode>>,
    /// The library ids each member was last given, so a change is sent only once.
    applied_libraries: Mutex<HashMap<String, Vec<i64>>>,
}

impl Default for FamilyService {
    fn default() -> Self {
        Self::new(Clock::system())
    }
}

/// Usernames a member can have: 2 to 32 of `a-z`, `0-9`, `.`, `_` and `-`, starting with a
/// letter or a digit. The name is a member's own folder under the members folder, so it must be
/// one plain folder name: never `.` or `..`, and never a hidden name like the pool's `.pool`.
pub fn valid_username(name: &str) -> bool {
    (2..=32).contains(&name.len())
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

/// A device name as given, without surrounding spaces, cut to [`DEVICE_NAME_LIMIT`] characters.
pub fn clean_device_name(name: &str) -> String {
    name.trim().chars().take(DEVICE_NAME_LIMIT).collect()
}

/// A new device id.
pub fn new_device_id() -> String {
    format!("d_{}", &device_secret()[..16])
}

/// Edit keys as `family.json` keeps them: the ability's name starting with a capital.
fn stored_edit_key(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Whether a library sits in the members folder.
fn under_members(path: &str, members_path: &str) -> bool {
    let path = path.trim_end_matches('/');
    let root = members_path.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

impl FamilyService {
    pub fn new(clock: Clock) -> Self {
        FamilyService {
            hooks: RwLock::new(None),
            sign_in_throttle: SignInThrottle::new().with_clock(clock.clone()),
            join_throttle: SignInThrottle::new().with_clock(clock.clone()),
            pair_guard: PairGuard::new(clock),
            links: RwLock::new(None),
            applied_libraries: Mutex::new(HashMap::new()),
        }
    }

    /// The hooks in use: the ones installed, or the standard ones for `state`.
    pub fn hooks(&self, state: &AppState) -> FamilyHooks {
        if let Some(hooks) = self.hooks.read().as_ref() {
            return hooks.clone();
        }
        let mut slot = self.hooks.write();
        slot.get_or_insert_with(|| FamilyHooks::standard(state)).clone()
    }

    /// Installs hooks: `change` gets the hooks in use and replaces the ones it owns.
    pub fn wire(&self, state: &AppState, change: impl FnOnce(&mut FamilyHooks)) {
        let mut hooks = self.hooks(state);
        change(&mut hooks);
        *self.hooks.write() = Some(hooks);
    }

    /// How members' libraries get files, once a link probe has run: what the probe found, or
    /// copies when Navidrome has since shown it cannot follow links.
    pub fn links(&self, state: &AppState) -> Option<LinkMode> {
        let probed = (*self.links.read())?;
        Some(self.hooks(state).folders.mode().unwrap_or(probed))
    }

    /// The link mode with what the Family page and the dashboard say about it:
    /// `{links, message, fix}`, or None before the first probe.
    pub fn link_status(&self, state: &AppState) -> Option<super::LinkStatus> {
        let mut status = super::LinkStatus::of(self.links(state)?);
        if status.message.is_some() {
            let members_path = state.settings.current().family.effective_members_path();
            status.message = Some(copy_mode_message(&members_path));
        }
        Some(status)
    }

    /// Probes the members folder for links and keeps the answer.
    pub async fn probe_links(&self, state: &AppState) -> LinkMode {
        let mode = self.hooks(state).folders.probe_links().await;
        *self.links.write() = Some(mode);
        mode
    }

    /// The address members' apps and links use: `Server:PublicUrl`, else `origin`.
    pub fn public_base(state: &AppState, origin: &str) -> String {
        let configured = state.settings.current().server.public_url.trim().to_string();
        let base = if configured.is_empty() {
            origin.to_string()
        } else {
            configured
        };
        base.trim_end_matches('/').to_string()
    }

    // --- who is who ----------------------------------------------------------------------

    /// The role of a username: a member's preset, the owner, or an account outside the family.
    pub fn role_of(state: &AppState, user: &str) -> Role {
        match state.family_store.member(user) {
            Some(member) => member.preset.into(),
            None if state.family_store.is_owner(user) => Role::Owner,
            None => Role::Unmanaged,
        }
    }

    /// The abilities that apply to a username: a member's, or the owner's for anyone else.
    pub fn abilities_of(state: &AppState, user: &str) -> Abilities {
        state
            .family_store
            .member(user)
            .map(|m| m.abilities())
            .unwrap_or_else(|| Abilities::preset(Preset::Owner))
    }

    /// Whether `actor` may manage the family.
    pub fn may_manage(state: &AppState, actor: &Actor) -> bool {
        actor.is_owner()
            || state
                .family_store
                .member(&actor.user)
                .is_some_and(|m| !m.suspended && m.abilities().manage_family)
    }

    /// Whether `actor` may approve requests.
    pub fn may_approve(state: &AppState, actor: &Actor) -> bool {
        actor.is_owner()
            || state
                .family_store
                .member(&actor.user)
                .is_some_and(|m| !m.suspended && m.abilities().approve_requests)
    }

    // --- answers ---------------------------------------------------------------------------

    /// The `family.me` object for a username.
    pub fn me_json(
        &self,
        state: &AppState,
        user: &str,
        device_id: Option<&str>,
        place: NetworkPlace,
    ) -> Value {
        let hooks = self.hooks(state);
        let member = state.family_store.member(user);
        let role = Self::role_of(state, user);
        let abilities = Self::abilities_of(state, user);
        let preference = state.family_store.preference(user);
        let now = Utc::now();
        json!({
            "username": member.as_ref().map_or(user.to_string(), |m| m.username.clone()),
            "displayName": member.as_ref().map_or(user.to_string(), |m| m.shown_name().to_string()),
            "role": role,
            "managed": member.is_some(),
            "abilities": abilities,
            "requestsThisWeek": state.family_requests.quota_used(user, now),
            "storageUsedBytes": if member.is_some() { hooks.storage.used_bytes(user) } else { 0 },
            "place": place,
            "deviceId": if member.is_some() { device_id } else { None },
            "quality": quality_json(&preference, &abilities),
        })
    }

    /// The `family.manager` object, or None when `user` does not manage the family.
    pub fn manager_json(&self, state: &AppState, actor: &Actor) -> Option<Value> {
        if !Self::may_manage(state, actor) {
            return None;
        }
        let hooks = self.hooks(state);
        let pending = state.family_requests.pending();
        let members: Vec<Value> = state
            .family_store
            .members()
            .iter()
            .map(|m| self.member_summary(state, &hooks, m, &pending))
            .collect();
        Some(json!({
            "members": members,
            "pendingRequests": pending.len(),
            "liveStreams": hooks.streams.count(),
        }))
    }

    fn member_summary(
        &self,
        state: &AppState,
        hooks: &FamilyHooks,
        m: &Member,
        pending: &[FamilyRequest],
    ) -> Value {
        let abilities = m.abilities();
        json!({
            "username": m.username,
            "displayName": m.shown_name(),
            "role": Role::from(m.preset),
            "suspended": m.suspended,
            "devices": state.family_store.devices_of(&m.username).len(),
            "playingNow": hooks.streams.user_playing(&m.username),
            "pendingRequests": pending.iter().filter(|r| eq_ignore_case(&r.username, &m.username)).count(),
            "storageUsedBytes": hooks.storage.used_bytes(&m.username),
            "storageLimitGb": abilities.storage_limit_gb,
        })
    }

    /// A member as the manager's list shows them: the summary plus abilities, edits and shared
    /// libraries.
    pub fn member_full_json(&self, state: &AppState, m: &Member) -> Value {
        let hooks = self.hooks(state);
        let pending = state.family_requests.pending();
        let mut value = self.member_summary(state, &hooks, m, &pending);
        if let Value::Object(map) = &mut value {
            map.insert("preset".into(), json!(m.preset));
            map.insert("abilities".into(), json!(m.abilities()));
            map.insert("edits".into(), Value::Object(api_edits(&m.edits)));
            map.insert(
                "sharedLibraries".into(),
                match &m.shared_libraries {
                    SharedLibraries::All => json!("All"),
                    SharedLibraries::Ids(ids) => json!(ids),
                },
            );
            map.insert("personalLibraryId".into(), json!(m.personal_library_id));
            map.insert("joined".into(), json!(!m.account_password.is_empty()));
            map.insert("created".into(), json!(datetime::format_utc(&m.created)));
        }
        value
    }

    /// A device as the API shows it.
    pub fn device_json(&self, state: &AppState, device: &Device, current: Option<&str>) -> Value {
        let hooks = self.hooks(state);
        json!({
            "id": device.id,
            "username": device.username,
            "name": device.name,
            "kind": device.kind,
            "app": device.app,
            "created": datetime::format_utc(&device.created),
            "lastSeen": device.last_seen.as_ref().map(datetime::format_utc),
            "place": device.place,
            "playing": hooks.streams.playing(&device.id),
            "current": current.is_some_and(|id| id == device.id),
            "quality": device.quality,
        })
    }

    /// The devices of `user`: a member's signed-in devices, or the devices seen for anyone else.
    pub fn devices_of(state: &AppState, user: &str) -> Vec<Device> {
        if state.family_store.member(user).is_some() {
            state.family_store.devices_of(user)
        } else {
            state
                .family_store
                .detected()
                .into_iter()
                .filter(|d| eq_ignore_case(&d.username, user))
                .collect()
        }
    }

    /// Every device: members' and the ones seen for everyone else.
    pub fn all_devices(state: &AppState) -> Vec<Device> {
        let mut devices = state.family_store.devices();
        devices.extend(state.family_store.detected());
        devices
    }

    /// A request as the API shows it.
    pub fn request_json(state: &AppState, request: &FamilyRequest) -> Value {
        let display = state
            .family_store
            .member(&request.username)
            .map(|m| m.shown_name().to_string())
            .unwrap_or_else(|| request.username.clone());
        request.api_json(&display)
    }

    /// Requests, newest first: `user`'s own, or everyone's when `user` is None; optionally
    /// only those in `state_filter`.
    pub fn requests(
        state: &AppState,
        user: Option<&str>,
        state_filter: Option<RequestState>,
    ) -> Vec<FamilyRequest> {
        let mut list = match user {
            Some(user) => state.family_requests.for_user(user),
            None => state.family_requests.all(),
        };
        if let Some(wanted) = state_filter {
            list.retain(|r| r.state == wanted);
        }
        list.sort_by_key(|a| std::cmp::Reverse(a.created));
        list
    }

    /// Navidrome's libraries for the page: `[{id, name, path, personal, owner}]`.
    pub async fn libraries_json(&self, state: &AppState) -> FamilyResult<Value> {
        let libraries = self.hooks(state).accounts.libraries().await?;
        let members = state.family_store.members();
        let members_path = state.settings.current().family.effective_members_path();
        Ok(Value::Array(
            libraries
                .iter()
                .map(|library| {
                    let owner = members
                        .iter()
                        .find(|m| m.personal_library_id == Some(library.id))
                        .map(|m| m.username.clone());
                    json!({
                        "id": library.id,
                        "name": library.name,
                        "path": library.path,
                        "personal": owner.is_some() || under_members(&library.path, &members_path),
                        "owner": owner,
                    })
                })
                .collect(),
        ))
    }

    // --- members ---------------------------------------------------------------------------

    /// The shared (non-personal) libraries out of `libraries`.
    fn shared_library_ids(state: &AppState, libraries: &[NdLibrary]) -> Vec<i64> {
        let personal: Vec<i64> = state
            .family_store
            .members()
            .iter()
            .filter_map(|m| m.personal_library_id)
            .collect();
        let members_path = state.settings.current().family.effective_members_path();
        libraries
            .iter()
            .filter(|l| !personal.contains(&l.id) && !under_members(&l.path, &members_path))
            .map(|l| l.id)
            .collect()
    }

    /// The libraries a member sees: their shared choice plus their own.
    fn member_library_ids(shared: &[i64], member: &Member) -> Vec<i64> {
        let mut ids = member.shared_libraries.resolve(shared);
        if let Some(own) = member.personal_library_id
            && !ids.contains(&own)
        {
            ids.push(own);
        }
        ids
    }

    fn check_preset_change(actor: &Actor, preset: Preset) -> FamilyResult<()> {
        match preset {
            Preset::Owner => Err(FamilyError::Invalid("A member cannot be an owner.".into())),
            Preset::CoAdmin if !actor.is_owner() => Err(FamilyError::Refused(
                "Only the owner can make someone a co-admin.".into(),
            )),
            _ => Ok(()),
        }
    }

    /// Adds a member: their Navidrome account with a hidden password, their folder and their
    /// own library, and the libraries they see. Answers the member and their invite link.
    pub async fn add_member(
        &self,
        state: &AppState,
        actor: &Actor,
        add: AddMember,
        base: &str,
    ) -> FamilyResult<(Member, String)> {
        let username = add.username.trim().to_lowercase();
        if !valid_username(&username) {
            return Err(FamilyError::Invalid(
                "Use 2 to 32 lowercase letters, numbers, dots, dashes or underscores for the username."
                    .into(),
            ));
        }
        let preset = add.preset.unwrap_or(Preset::Listener);
        Self::check_preset_change(actor, preset)?;
        if state.family_store.member(&username).is_some() {
            return Err(FamilyError::Conflict(format!(
                "{username} is already in the family."
            )));
        }
        if state.family_store.is_owner(&username) {
            return Err(FamilyError::Conflict(format!("{username} is the family owner.")));
        }
        if !state.family_vault.has_key() {
            return Err(FamilyError::Unavailable(
                "family.key is missing, so Octo cannot keep members' secrets. Turn Family on again from the dashboard.".into(),
            ));
        }
        let hooks = self.hooks(state);
        let users = hooks.accounts.users().await?;
        let existing = users.iter().find(|u| eq_ignore_case(u.user_name(), &username));
        if let Some(user) = existing {
            if user.is_admin() {
                return Err(FamilyError::Conflict(format!(
                    "{username} is a Navidrome admin, and an admin cannot be a family member."
                )));
            }
            if !add.adopt {
                return Err(FamilyError::Conflict(format!(
                    "{username} already has a Navidrome account. Bring it into the family to manage it here; the apps already signed in to it will have to sign in again."
                )));
            }
        }
        let display_name = match add.display_name.trim() {
            "" => existing
                .map(|u| u.name().to_string())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| username.clone()),
            name => name.to_string(),
        };
        let hidden = device_secret();
        let sealed = state
            .family_vault
            .seal(&hidden)
            .ok_or_else(|| FamilyError::Unavailable("family.key is missing.".into()))?;

        // The account.
        let (user_id, created_user) = match existing {
            Some(user) => {
                hooks.accounts.set_password(user.id(), &hidden).await?;
                (user.id().to_string(), false)
            }
            None => (
                hooks
                    .accounts
                    .create_user(&username, &display_name, &hidden)
                    .await?,
                true,
            ),
        };
        let undo_user = |accounts: Arc<dyn MemberAccounts>, id: String| async move {
            if created_user && let Err(e) = accounts.delete_user(&id).await {
                warn!("FAMILY the half-made Navidrome user {id} could not be removed: {e}");
            }
        };

        // The folder and the library.
        let folder = match hooks.folders.create(&username).await {
            Ok(folder) => folder,
            Err(e) => {
                undo_user(hooks.accounts.clone(), user_id).await;
                return Err(FamilyError::Unavailable(e));
            }
        };
        let libraries = match hooks.accounts.libraries().await {
            Ok(libraries) => libraries,
            Err(e) => {
                undo_user(hooks.accounts.clone(), user_id).await;
                return Err(e.into());
            }
        };
        let members_path = state.settings.current().family.effective_members_path();
        let library_id = match libraries.iter().find(|l| l.path.trim_end_matches('/') == folder) {
            Some(library) => library.id,
            None => {
                let mut name = format!("{display_name}'s library");
                let mut n = 2;
                while libraries.iter().any(|l| eq_ignore_case(&l.name, &name)) {
                    name = format!("{display_name}'s library ({n})");
                    n += 1;
                }
                match hooks.accounts.create_library(&name, &folder).await {
                    Ok(id) => id,
                    Err(e) => {
                        undo_user(hooks.accounts.clone(), user_id).await;
                        hooks.folders.remove_if_empty(&username).await;
                        let words = e.body().unwrap_or("").to_ascii_lowercase();
                        return Err(match &e {
                            AdminError::Status { status, .. }
                                if (400..500).contains(status)
                                    && ["exist", "directory", "path", "not found", "no such"]
                                        .iter()
                                        .any(|w| words.contains(w)) =>
                            {
                                FamilyError::Conflict(cannot_see_members_folder(&members_path))
                            }
                            _ => e.into(),
                        });
                    }
                }
            }
        };

        let mut member = Member {
            username: existing.map_or(username.clone(), |u| u.user_name().to_string()),
            display_name,
            preset,
            edits: Map::new(),
            navidrome_user_id: user_id.clone(),
            navidrome_password: sealed,
            personal_library_id: Some(library_id),
            shared_libraries: add.shared_libraries.unwrap_or_default(),
            account_password: String::new(),
            created: Utc::now(),
            suspended: false,
            ntfy_topic: String::new(),
        };
        let shared = Self::shared_library_ids(state, &libraries)
            .into_iter()
            .filter(|id| *id != library_id)
            .collect::<Vec<_>>();
        let ids = Self::member_library_ids(&shared, &member);
        if let Err(e) = hooks.accounts.set_user_libraries(&user_id, &ids).await {
            let _ = hooks.accounts.delete_library(library_id).await;
            undo_user(hooks.accounts.clone(), user_id).await;
            hooks.folders.remove_if_empty(&username).await;
            return Err(e.into());
        }
        self.applied_libraries
            .lock()
            .insert(member.username.to_lowercase(), ids);
        if existing.is_some() {
            // The account's old sign-ins on the Family page stop too.
            state.browse_sessions.revoke_user(&member.username);
        }
        member.username = member.username.trim().to_string();
        state.family_store.upsert_member(member.clone());
        let link = self.invite(state, &member.username, base)?;
        info!(
            "FAMILY member {} {} ({:?}) by {}",
            if existing.is_some() { "adopted" } else { "added" },
            member.username,
            member.preset,
            actor.user
        );
        Ok((member, link))
    }

    /// Changes a member.
    pub async fn edit_member(
        &self,
        state: &AppState,
        actor: &Actor,
        name: &str,
        edit: EditMember,
    ) -> FamilyResult<Member> {
        let mut member = state
            .family_store
            .member(name)
            .ok_or_else(|| FamilyError::NotFound(format!("{name} is not in the family.")))?;
        if member.preset == Preset::CoAdmin && !actor.is_owner() {
            return Err(FamilyError::Refused(
                "Only the owner can change a co-admin.".into(),
            ));
        }
        if eq_ignore_case(&member.username, &actor.user)
            && (edit.preset.is_some() || edit.edits.is_some() || edit.suspended == Some(true))
        {
            return Err(FamilyError::Refused(
                "Ask the owner to change your own abilities.".into(),
            ));
        }
        if let Some(display) = edit.display_name.as_deref().map(str::trim) {
            if display.is_empty() {
                return Err(FamilyError::Invalid("A name cannot be empty.".into()));
            }
            member.display_name = display.chars().take(64).collect();
        }
        if let Some(preset) = edit.preset {
            Self::check_preset_change(actor, preset)?;
            member.preset = preset;
        }
        if let Some(edits) = edit.edits {
            let base = Abilities::preset(member.preset);
            let (applied, warnings) = base.clone().with_edits(&edits);
            if !warnings.is_empty() {
                return Err(FamilyError::Invalid(warnings.join("; ")));
            }
            if !actor.is_owner() && (applied.manage_family || applied.approve_requests) && !base.manage_family
            {
                return Err(FamilyError::Refused(
                    "Only the owner can let someone manage the family or approve requests.".into(),
                ));
            }
            member.edits = edits
                .into_iter()
                .map(|(key, value)| (stored_edit_key(&key), value))
                .collect();
        }
        let libraries_changed = edit.shared_libraries.is_some();
        if let Some(shared) = edit.shared_libraries {
            member.shared_libraries = shared;
        }
        if let Some(suspended) = edit.suspended {
            member.suspended = suspended;
        }
        if libraries_changed {
            let hooks = self.hooks(state);
            let libraries = hooks.accounts.libraries().await?;
            let shared = Self::shared_library_ids(state, &libraries);
            let ids = Self::member_library_ids(&shared, &member);
            if ids.is_empty() {
                return Err(FamilyError::Invalid(
                    "A member needs at least one library.".into(),
                ));
            }
            hooks
                .accounts
                .set_user_libraries(&member.navidrome_user_id, &ids)
                .await?;
            self.applied_libraries
                .lock()
                .insert(member.username.to_lowercase(), ids);
        }
        if member.suspended {
            state.browse_sessions.revoke_user(&member.username);
        }
        state.family_store.upsert_member(member.clone());
        info!("FAMILY member {} changed by {}", member.username, actor.user);
        Ok(member)
    }

    /// Removes a member: their files by `files`, their Navidrome library and account, their
    /// devices and sign-ins. `confirm` must be the member's username.
    pub async fn remove_member(
        &self,
        state: &AppState,
        actor: &Actor,
        name: &str,
        confirm: &str,
        files: FilesChoice,
    ) -> FamilyResult<()> {
        let member = state
            .family_store
            .member(name)
            .ok_or_else(|| FamilyError::NotFound(format!("{name} is not in the family.")))?;
        if !eq_ignore_case(confirm.trim(), &member.username) {
            return Err(FamilyError::Invalid(format!(
                "Type {} to confirm removing them.",
                member.username
            )));
        }
        if member.preset == Preset::CoAdmin && !actor.is_owner() {
            return Err(FamilyError::Refused(
                "Only the owner can remove a co-admin.".into(),
            ));
        }
        if eq_ignore_case(&member.username, &actor.user) {
            return Err(FamilyError::Refused(
                "Ask the owner to remove your account.".into(),
            ));
        }
        let hooks = self.hooks(state);
        // Navidrome must be reachable as an admin before anything is changed.
        hooks.accounts.users().await?;
        hooks.files.release(&member.username, files).await?;
        if let Some(library) = member.personal_library_id {
            match hooks.accounts.delete_library(library).await {
                Ok(()) | Err(AdminError::Status { status: 404, .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if !member.navidrome_user_id.trim().is_empty() {
            match hooks.accounts.delete_user(&member.navidrome_user_id).await {
                Ok(()) | Err(AdminError::Status { status: 404, .. }) => {}
                Err(e) => return Err(e.into()),
            }
        }
        state.family_store.remove_member(&member.username);
        state.family_request_service.close_member(&member.username);
        state.family_saved.remove_user(&member.username);
        state.browse_sessions.revoke_user(&member.username);
        self.applied_libraries
            .lock()
            .remove(&member.username.to_lowercase());
        info!(
            "FAMILY member {} removed by {} (files {files:?})",
            member.username, actor.user
        );
        Ok(())
    }

    /// A new sign-in link for a member: the join page, where they set their account password.
    pub fn invite(&self, state: &AppState, name: &str, base: &str) -> FamilyResult<String> {
        let member = state
            .family_store
            .member(name)
            .ok_or_else(|| FamilyError::NotFound(format!("{name} is not in the family.")))?;
        let invite = token();
        let days = i64::from(state.settings.current().family.invite_days.max(1));
        state
            .family_store
            .add_invite(&invite, &member.username, Utc::now() + TimeDelta::days(days));
        Ok(format!("{base}/family/join#invite={invite}"))
    }

    /// Sets a new hidden Navidrome password for a member and gives them their libraries again.
    pub async fn repair(&self, state: &AppState, actor: &Actor, name: &str) -> FamilyResult<()> {
        let mut member = state
            .family_store
            .member(name)
            .ok_or_else(|| FamilyError::NotFound(format!("{name} is not in the family.")))?;
        if member.navidrome_user_id.trim().is_empty() {
            return Err(FamilyError::Conflict(format!(
                "{} has no Navidrome account on record. Remove them and add them again.",
                member.username
            )));
        }
        let hooks = self.hooks(state);
        let hidden = device_secret();
        let sealed = state
            .family_vault
            .seal(&hidden)
            .ok_or_else(|| FamilyError::Unavailable("family.key is missing.".into()))?;
        hooks
            .accounts
            .set_password(&member.navidrome_user_id, &hidden)
            .await?;
        member.navidrome_password = sealed;
        state.family_store.upsert_member(member.clone());
        if let Ok(libraries) = hooks.accounts.libraries().await {
            let shared = Self::shared_library_ids(state, &libraries);
            let ids = Self::member_library_ids(&shared, &member);
            if !ids.is_empty() {
                hooks
                    .accounts
                    .set_user_libraries(&member.navidrome_user_id, &ids)
                    .await?;
                self.applied_libraries
                    .lock()
                    .insert(member.username.to_lowercase(), ids);
            }
        }
        info!("FAMILY member {} repaired by {}", member.username, actor.user);
        Ok(())
    }

    /// Gives every member the libraries they should see when the shared libraries changed
    /// (one added or removed in Navidrome) since they were last given them.
    pub async fn sync_libraries(&self, state: &AppState) -> FamilyResult<usize> {
        let hooks = self.hooks(state);
        let libraries = hooks.accounts.libraries().await?;
        let shared = Self::shared_library_ids(state, &libraries);
        let mut changed = 0;
        for member in state.family_store.members() {
            if member.navidrome_user_id.trim().is_empty() {
                continue;
            }
            let ids = Self::member_library_ids(&shared, &member);
            let key = member.username.to_lowercase();
            if ids.is_empty() || self.applied_libraries.lock().get(&key) == Some(&ids) {
                continue;
            }
            match hooks
                .accounts
                .set_user_libraries(&member.navidrome_user_id, &ids)
                .await
            {
                Ok(()) => {
                    self.applied_libraries.lock().insert(key, ids);
                    changed += 1;
                }
                Err(e) => warn!("FAMILY {}'s libraries could not be set: {e}", member.username),
            }
        }
        Ok(changed)
    }

    // --- joining and signing in ------------------------------------------------------------

    /// Uses an invite: sets the member's account password (and name). Answers the username.
    pub async fn join(
        &self,
        state: &AppState,
        address: &str,
        invite: &str,
        password: &str,
        display_name: Option<&str>,
    ) -> FamilyResult<String> {
        if let Some(wait) = self.join_throttle.retry_after(address) {
            return Err(FamilyError::Throttled(
                format!(
                    "Too many wrong links. Try again in {}.",
                    SignInThrottle::minutes(wait)
                ),
                wait,
            ));
        }
        passwords::acceptable(password).map_err(|e| FamilyError::Invalid(e.into()))?;
        let Some(user) = state.family_store.take_invite(invite.trim(), Utc::now()) else {
            self.join_throttle.failed(address);
            return Err(FamilyError::Refused(
                "This link has expired or was already used. Ask for a new one.".into(),
            ));
        };
        let mut member = state
            .family_store
            .member(&user)
            .ok_or_else(|| FamilyError::NotFound("This account is no longer in the family.".into()))?;
        if member.suspended {
            return Err(FamilyError::Refused(
                "This account is paused by the family owner.".into(),
            ));
        }
        self.join_throttle.succeeded(address);
        member.account_password = hashed(password).await?;
        if let Some(name) = display_name.map(str::trim).filter(|n| !n.is_empty()) {
            member.display_name = name.chars().take(64).collect();
        }
        state.family_store.upsert_member(member.clone());
        state.browse_sessions.revoke_user(&member.username);
        info!("FAMILY {} joined", member.username);
        Ok(member.username)
    }

    /// Checks a member's account password: whether it is right, or [`passwords::Busy`] when
    /// no hashing slot came free in time.
    pub async fn check_member_password(
        &self,
        member: &Member,
        password: &str,
    ) -> Result<bool, passwords::Busy> {
        if member.account_password.is_empty() {
            return Ok(false);
        }
        passwords::verify_async(password, &member.account_password).await
    }

    /// Changes a member's account password after checking the current one.
    pub async fn change_password(
        &self,
        state: &AppState,
        user: &str,
        current: &str,
        next: &str,
    ) -> FamilyResult<()> {
        let mut member = state
            .family_store
            .member(user)
            .ok_or_else(|| FamilyError::NotFound("This account is not in the family.".into()))?;
        match self.check_member_password(&member, current).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(FamilyError::Refused("Your current password is not right.".into()));
            }
            Err(passwords::Busy) => return Err(busy()),
        }
        passwords::acceptable(next).map_err(|e| FamilyError::Invalid(e.into()))?;
        member.account_password = hashed(next).await?;
        state.family_store.upsert_member(member);
        Ok(())
    }

    // --- devices ---------------------------------------------------------------------------

    /// Adds a device for a member: an app password (any Subsonic app, shown once) or a pairing
    /// code (an Octo app).
    pub fn add_device(
        &self,
        state: &AppState,
        user: &str,
        name: &str,
        kind: DeviceKind,
        base: &str,
    ) -> FamilyResult<DeviceAdded> {
        let member = state
            .family_store
            .member(user)
            .ok_or_else(|| FamilyError::Refused("Only family members add devices here.".into()))?;
        if member.suspended {
            return Err(FamilyError::Refused(
                "This account is paused by the family owner.".into(),
            ));
        }
        let name = clean_device_name(name);
        match kind {
            DeviceKind::SubsonicApp => {
                let password = app_password();
                let device =
                    self.new_device(state, &member, &name, DeviceKind::SubsonicApp, &password, "")?;
                info!(
                    "FAMILY app password made for {} ({:?})",
                    member.username, device.name
                );
                Ok(DeviceAdded {
                    device_id: Some(device.id),
                    kind,
                    app_password: Some(password),
                    pair_code: None,
                    expires: None,
                    server: base.to_string(),
                    username: member.username,
                })
            }
            DeviceKind::OctoApp => {
                let code = pair_code();
                let minutes = i64::from(state.settings.current().family.pair_code_minutes.max(1));
                let expires = Utc::now() + TimeDelta::minutes(minutes);
                state
                    .family_store
                    .add_pair_code(&code, &member.username, &name, expires);
                Ok(DeviceAdded {
                    device_id: None,
                    kind,
                    app_password: None,
                    pair_code: Some(code),
                    expires: Some(datetime::format_utc(&expires)),
                    server: base.to_string(),
                    username: member.username,
                })
            }
            DeviceKind::NavidromeWeb | DeviceKind::Detected => Err(FamilyError::Invalid(
                "Choose OctoApp for an Octo app or SubsonicApp for any other app.".into(),
            )),
        }
    }

    fn new_device(
        &self,
        state: &AppState,
        member: &Member,
        name: &str,
        kind: DeviceKind,
        secret: &str,
        app: &str,
    ) -> FamilyResult<Device> {
        let sealed = state
            .family_vault
            .seal(secret)
            .ok_or_else(|| FamilyError::Unavailable("family.key is missing.".into()))?;
        let device = Device {
            id: new_device_id(),
            username: member.username.clone(),
            name: name.to_string(),
            kind,
            secret: sealed,
            created: Utc::now(),
            last_seen: None,
            app: app.to_string(),
            place: NetworkPlace::Home,
            seen_apps: if app.is_empty() {
                Vec::new()
            } else {
                vec![app.to_string()]
            },
            navidrome_jwt: None,
            quality: DeviceQualityMode::Account,
        };
        state.family_store.add_device(device.clone());
        Ok(device)
    }

    /// Pairs an app with a member's code. `kind` SubsonicApp gives an app password in place of
    /// an Octo app's secret, for a person who opened the join link and uses another app.
    #[allow(clippy::too_many_arguments)]
    pub fn pair(
        &self,
        state: &AppState,
        address: &str,
        peer: &str,
        username: &str,
        code: &str,
        device_name: &str,
        platform: &str,
        kind: DeviceKind,
        base: &str,
    ) -> FamilyResult<Paired> {
        let refused = || FamilyError::Refused(PAIR_REFUSED.into());
        let now = Utc::now();
        if self.pair_guard.blocked(address, peer, username) {
            return Err(refused());
        }
        if self.pair_guard.capped() && state.family_store.any_live_pair_code(now) {
            if state.family_bridge.may_log("pair-cap") {
                warn!("FAMILY pairing paused: {PAIR_CAP} wrong codes in 10 minutes");
            }
            return Err(refused());
        }
        let (username, code_name) = match state
            .family_store
            .take_pair_code(username.trim(), code.trim(), now)
        {
            PairOutcome::Ok {
                username,
                device_name,
            } => (username, device_name),
            _ => {
                self.pair_guard.failed_from(address, peer, username);
                return Err(refused());
            }
        };
        let member = state.family_store.member(&username).ok_or_else(refused)?;
        if member.suspended {
            return Err(FamilyError::Refused(
                "This account is paused by the family owner.".into(),
            ));
        }
        let platform = platform.trim();
        let name = match clean_device_name(device_name) {
            name if !name.is_empty() => name,
            _ if !code_name.trim().is_empty() => code_name,
            _ if !platform.is_empty() => format!("Octo on {platform}"),
            _ => "Octo app".to_string(),
        };
        let (secret, app) = match kind {
            DeviceKind::SubsonicApp => (app_password(), String::new()),
            _ => (
                device_secret(),
                if platform.is_empty() {
                    "Octo".to_string()
                } else {
                    format!("Octo ({platform})")
                },
            ),
        };
        let kind = if kind == DeviceKind::SubsonicApp {
            kind
        } else {
            DeviceKind::OctoApp
        };
        let device = self.new_device(state, &member, &name, kind, &secret, &app)?;
        info!(
            "FAMILY device paired {} {:?} ({})",
            member.username, device.name, device.app
        );
        Ok(Paired {
            username: member.username,
            secret,
            device_id: device.id,
            server: base.to_string(),
        })
    }

    /// Signs a device out. Members sign out their own; managers anyone's but the devices Octo
    /// only saw (the owner's and accounts outside the family).
    pub fn sign_out_device(
        &self,
        state: &AppState,
        actor: &Actor,
        id: &str,
        as_manager: bool,
    ) -> FamilyResult<()> {
        let Some(device) = state.family_store.device(id) else {
            if state.family_store.detected().iter().any(|d| d.id == id) {
                return Err(FamilyError::Refused(
                    "Octo did not sign this device in, so it cannot sign it out. Change the account's password in Navidrome instead.".into(),
                ));
            }
            return Err(FamilyError::NotFound("There is no such device.".into()));
        };
        let own = eq_ignore_case(&device.username, &actor.user);
        if !own && !as_manager {
            return Err(FamilyError::NotFound("There is no such device.".into()));
        }
        state.family_store.remove_device(&device.id);
        info!(
            "FAMILY device signed out {} {:?} by {}",
            device.username, device.name, actor.user
        );
        Ok(())
    }

    /// Changes how one of `user`'s devices picks its quality, and its name.
    pub fn set_device(
        &self,
        state: &AppState,
        user: &str,
        id: &str,
        mode: Option<DeviceQualityMode>,
        name: Option<&str>,
    ) -> FamilyResult<Device> {
        let mut device = Self::devices_of(state, user)
            .into_iter()
            .find(|d| d.id == id)
            .ok_or_else(|| FamilyError::NotFound("There is no such device.".into()))?;
        if let Some(name) = name.map(clean_device_name).filter(|n| !n.is_empty()) {
            if device.kind == DeviceKind::Detected {
                return Err(FamilyError::Invalid(
                    "Octo names this device from what its app sends.".into(),
                ));
            }
            device.name = name;
            state.family_store.add_device(device.clone());
        }
        if let Some(mode) = mode {
            state.family_store.set_device_quality(id, mode);
            device.quality = mode;
        }
        Ok(device)
    }

    // --- turning Family on -----------------------------------------------------------------

    /// Turns Family on or off for `by` (a Navidrome admin). On: checks Octo can write the
    /// members folder, probes links, makes `family.key`, makes `by` the owner when there is
    /// none, saves `Family:Enabled` and starts Family's work.
    pub async fn enable(&self, state: &AppState, by: &str, on: bool) -> FamilyResult<EnableReport> {
        let owner = state.family_store.owner();
        if !owner.is_empty() && !eq_ignore_case(&owner, by) {
            return Err(FamilyError::Refused(format!(
                "Only {owner} can turn Family on or off."
            )));
        }
        let members_path = state.settings.current().family.effective_members_path();
        if !on {
            save_enabled(state, false)?;
            info!("FAMILY turned off by {by}");
            return Ok(EnableReport {
                enabled: false,
                members_path,
                links: self.links(state).unwrap_or(LinkMode::Symlink),
                message: "Family is off. Members cannot sign in until it is on again.".into(),
            });
        }
        let root = PathBuf::from(&members_path);
        let probe_root = root.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            make_folder(&probe_root)?;
            let probe = probe_root.join(format!(".octo-write-probe-{}", hex::encode(rand::random::<[u8; 6]>())));
            std::fs::write(&probe, b"octo")?;
            std::fs::remove_file(&probe)
        })
        .await
        .map_err(|e| FamilyError::Unavailable(e.to_string()))?
        .map_err(|e| {
            FamilyError::Conflict(format!(
                "Octo cannot write to {members_path} ({e}). Give Octo a {members_path} folder it can write to: the members volume in docker-compose.yml."
            ))
        })?;
        let links = self.probe_links(state).await;
        match state.family_vault.create_key_if_missing() {
            Ok(true) => {}
            Ok(false) => {
                return Err(FamilyError::Conflict(
                    "family.key is in the config folder but cannot be read. Put the right key file back, or move it aside to start over.".into(),
                ));
            }
            Err(e) => {
                return Err(FamilyError::Unavailable(format!(
                    "family.key could not be made: {e}"
                )));
            }
        }
        if owner.is_empty() {
            state.family_store.set_owner(by);
        }
        save_enabled(state, true)?;
        state.start_family();
        info!("FAMILY turned on by {by} ({links:?})");
        let message = match links {
            LinkMode::Symlink => "Family is on. A song two members want is kept on disk once.".to_string(),
            LinkMode::Copy => copy_mode_message(&members_path),
        };
        Ok(EnableReport {
            enabled: true,
            members_path,
            links,
            message,
        })
    }

    // --- notifications ---------------------------------------------------------------------

    fn request_event(
        state: &AppState,
        request: &FamilyRequest,
        kind: NotificationEventType,
    ) -> NotificationEvent {
        let member = state
            .family_store
            .member(&request.username)
            .map(|m| m.shown_name().to_string())
            .unwrap_or_else(|| request.username.clone());
        NotificationEvent {
            title: Some(request.title.clone()).filter(|t| !t.is_empty()),
            artist: Some(request.artist.clone()).filter(|a| !a.is_empty()),
            album: Some(request.album.clone()).filter(|a| !a.is_empty()),
            format: Some(match request.quality {
                RequestQuality::Best => "best quality".to_string(),
                other => other.label().to_string(),
            }),
            detail: request.failure.clone(),
            family_member: Some(member),
            family_note: Some(request.note.clone()).filter(|n| !n.trim().is_empty()),
            ..NotificationEvent::new(kind)
        }
    }

    fn push_member(state: &AppState, request: &FamilyRequest, kind: NotificationEventType) {
        let Some(member) = state.family_store.member(&request.username) else {
            return;
        };
        if member.ntfy_topic.trim().is_empty() {
            return;
        }
        state
            .notifications
            .notify_member(&member.ntfy_topic, Self::request_event(state, request, kind));
    }

    /// Tells people about one change to a request, whatever made it (the Family page, an app,
    /// a heart, an action playlist, an import, a Find songs pick or a playlist addition). Each
    /// change is raised once on [`super::FamilyEvents`], so each is told once:
    ///
    /// - a request that waits: the owner's channels;
    /// - approved (by an approver or at once by the member's abilities) or declined: the member;
    /// - in the member's library: the member;
    /// - failed: the owner's channels and the member.
    pub fn announce(state: &AppState, event: &FamilyEvent) {
        match event {
            FamilyEvent::RequestCreated(request) => state.notifications.notify(Self::request_event(
                state,
                request,
                NotificationEventType::FamilyRequestNew,
            )),
            FamilyEvent::RequestDecided(request) => match request.state {
                RequestState::Approved => {
                    Self::push_member(state, request, NotificationEventType::FamilyRequestApproved)
                }
                RequestState::Declined => {
                    Self::push_member(state, request, NotificationEventType::FamilyRequestDeclined)
                }
                _ => {}
            },
            FamilyEvent::RequestDone(request) => {
                Self::push_member(state, request, NotificationEventType::FamilyRequestAdded)
            }
            FamilyEvent::RequestFailed(request) => {
                state.notifications.notify(Self::request_event(
                    state,
                    request,
                    NotificationEventType::FamilyRequestFailed,
                ));
                Self::push_member(state, request, NotificationEventType::FamilyRequestFailed);
            }
        }
    }

    /// A device was seen for the first time: the owner's channels hear of it.
    pub fn device_seen(state: &AppState, device: &NewDevice) {
        let who = state
            .family_store
            .member(&device.username)
            .map(|m| m.shown_name().to_string())
            .unwrap_or_else(|| device.username.clone());
        let app = match (device.app.trim(), device.name.trim()) {
            ("", "") => None,
            (app, "") => Some(app.to_string()),
            ("", name) => Some(name.to_string()),
            (app, name) => Some(format!("{app} on {name}")),
        };
        state.notifications.notify(NotificationEvent {
            family_member: Some(who),
            source: app,
            ..NotificationEvent::new(NotificationEventType::FamilyDeviceNew)
        });
    }
}

/// How often members' libraries are checked against Navidrome's list of libraries.
pub const LIBRARY_SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// Family's own background work while Family is on: probes the members folder for links,
/// starts again approved requests a restart left unfinished, tells people about requests
/// ([`FamilyService::announce`]) and the owner's channels about new devices, and keeps
/// members' libraries in step with the shared libraries Navidrome has.
/// Holds the state weakly, so a dropped state ends it.
pub async fn run_family_work(
    state: std::sync::Weak<crate::app::AppInner>,
    token: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    let Some(first) = state.upgrade().map(AppState::from_inner) else {
        return Ok(());
    };
    let mut devices = first.family_devices.subscribe();
    let mut requests = first.family_events.subscribe();
    first.family.probe_links(&first).await;
    // Approved requests a restart left unfinished start again, now that the pool knows how
    // members' libraries get files.
    first.family_request_service.resume_approved();
    drop(first);
    let mut sync = tokio::time::interval(LIBRARY_SYNC_EVERY);
    loop {
        tokio::select! {
            () = token.cancelled() => return Ok(()),
            changed = requests.recv() => match changed {
                Ok(event) => {
                    let Some(state) = state.upgrade().map(AppState::from_inner) else { return Ok(()) };
                    if state.settings.current().family.enabled {
                        FamilyService::announce(&state, &event);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!("FAMILY {missed} request notices were not sent: too many at once");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            },
            seen = devices.recv() => match seen {
                Ok(device) => {
                    let Some(state) = state.upgrade().map(AppState::from_inner) else { return Ok(()) };
                    if state.settings.current().family.enabled {
                        FamilyService::device_seen(&state, &device);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
            },
            _ = sync.tick() => {
                let Some(state) = state.upgrade().map(AppState::from_inner) else { return Ok(()) };
                if state.settings.current().family.enabled
                    && state.navidrome_identity.has_admin_identity()
                    && let Err(e) = state.family.sync_libraries(&state).await
                {
                    tracing::debug!("FAMILY members' libraries were not checked: {e}");
                }
            }
        }
    }
}

/// The `quality` object: the account's choice and the family limits.
pub fn quality_json(
    preference: &octo_core::family::quality::QualityPreference,
    abilities: &Abilities,
) -> Value {
    json!({
        "home": preference.home.label(),
        "away": preference.away.label(),
        "familyLimitKbps": abilities.stream_cap.kbps().unwrap_or(0),
        "familyAwayLimitKbps": abilities.away_cap,
    })
}

/// Edits as the API writes them: camelCase keys.
fn api_edits(edits: &Map<String, Value>) -> Map<String, Value> {
    edits
        .iter()
        .map(|(key, value)| {
            let mut chars = key.chars();
            let key = match chars.next() {
                Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
                None => String::new(),
            };
            (key, value.clone())
        })
        .collect()
}

/// Every hashing slot stayed busy: the person tries again shortly.
pub fn busy() -> FamilyError {
    FamilyError::Throttled(passwords::BUSY.into(), TimeDelta::seconds(5))
}

/// An account password's hash, made when a hashing slot is free.
async fn hashed(password: &str) -> FamilyResult<String> {
    let hash = passwords::hash_async(password).await.map_err(|_| busy())?;
    if hash.is_empty() {
        return Err(FamilyError::Unavailable(
            "Octo could not save the password. Try again.".into(),
        ));
    }
    Ok(hash)
}

/// Saves `Family:Enabled` in settings.json and makes it the running value.
fn save_enabled(state: &AppState, on: bool) -> FamilyResult<()> {
    use indexmap::IndexMap;
    use octo_core::json::dom::Node;
    if state.settings.settings_path().is_some() {
        let mut section = IndexMap::new();
        section.insert("Enabled".to_string(), Node::Bool(on));
        let mut patch = IndexMap::new();
        patch.insert("Family".to_string(), Node::Object(section));
        state
            .settings_writer
            .merge(&patch, &[])
            .map_err(|e| FamilyError::Unavailable(format!("Octo could not save the setting: {e}")))?;
        if let Err(e) = state.settings.reload_now() {
            warn!("FAMILY settings could not be read again: {e}");
        }
    }
    if state.settings.current().family.enabled != on {
        // A test store, or a value the environment sets over the file.
        if state.settings.settings_path().is_some() && state.settings.raw("Family:Enabled").is_some() {
            let current = state.settings.current();
            if current.family.enabled != on {
                return Err(FamilyError::Conflict(format!(
                    "Family:Enabled is set to {} in Octo's environment (FAMILY_ENABLED), which wins over the dashboard. Change it there.",
                    current.family.enabled
                )));
            }
        }
        let mut settings = (*state.settings.current()).clone();
        settings.family.enabled = on;
        state.settings.set(settings);
    }
    Ok(())
}
