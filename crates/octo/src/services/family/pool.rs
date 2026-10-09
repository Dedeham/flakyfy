//! The family pool: one real file for every song a member downloaded, under
//! `<members>/.pool/<layout>`, and a relative link to it in the library of each member who has
//! it (`<members>/<user>/<layout>`). No Navidrome library points at the pool, so nobody browses
//! it; each member sees the song through their own library only.
//!
//! `family-pool.json` ([`FamilyPoolStore`]) is the record of who links what. When the last
//! member lets a file go it goes to Octo's trash (the library actions trash under the members
//! folder, kept for the same number of days). A better copy of a song replaces the old one for
//! everyone who had it, and a song the shared library wants moves there, leaving no links
//! behind. Stars and family playlist entries follow a song whose id changes with its file.
//!
//! Where links do not work (an rclone mount without `--vfs-links`, some network disks), each
//! member gets a real copy instead and [`FamilyPool::link_status`] says so in plain words.
//! What Octo learns about links (Navidrome showed one, or showed a copy where it never showed
//! the link) is kept in `family-pool.json`, so a restart does not forget it.
//!
//! Every change to the pool (a link, an unlink, a better copy, a move into the shared library,
//! a member let go, a sweep) holds one lock for its whole run, so no change reads the record
//! while another is halfway through it.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::common::{PathHelper, SongIdentity};
use octo_core::models::domain::Song;
use octo_core::settings::{LibraryAction, SettingsStore};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::family_playlist_store::FamilyPlaylistStore;
use super::family_pool_store::{FamilyPoolStore, PoolFile};
use super::family_service::valid_username;
use super::family_store::{FamilyStore, Member, SharedLibraries};
use super::library_index::{IndexedSong, LibraryIndex};
use super::member_credentials::MemberCredentials;
use crate::services::common::acquisition_tracker::key_of;
use crate::services::library::{LibraryActionQuarantine, PathSource, ResolvedSongFile};
use crate::services::local::ILocalLibraryService;

/// The pool's folder under the members folder. The dot keeps it out of every member's
/// library and out of Navidrome's scans.
pub const POOL_FOLDER: &str = ".pool";

/// Where a removed member's files are kept aside, under the members folder. The dot keeps it
/// out of every library.
pub const KEPT_FOLDER: &str = ".kept";

/// What becomes of the files only a removed member had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesChoice {
    /// Kept aside for the owner, under `<members>/.kept/<user>/`.
    Keep,
    /// Moved into the shared library.
    Promote,
    /// Sent to Octo's trash.
    Delete,
}

impl FilesChoice {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "keep" => Some(FilesChoice::Keep),
            "promote" => Some(FilesChoice::Promote),
            "delete" => Some(FilesChoice::Delete),
            _ => None,
        }
    }
}

/// What became of a removed member's pool files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Released {
    pub kept: usize,
    pub promoted: usize,
    pub trashed: usize,
    /// Files another member still has, left where they are.
    pub shared: usize,
}

/// What the Family page says when the members folder cannot link files.
pub const COPY_MODE_MESSAGE: &str =
    "This disk cannot link files, so a song two members want takes space twice.";

/// How to make links work again, for the same page.
pub const COPY_MODE_FIX: &str = "Start the rclone mount of the members folder with --vfs-links, or keep the members folder on a local disk.";

/// Cover files a member's album folder gets beside its songs.
const COVER_FILES: [&str; 6] = [
    "cover.jpg",
    "cover.png",
    "folder.jpg",
    "folder.png",
    "front.jpg",
    "front.png",
];

/// How a member's library gets a pool song.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkMode {
    /// A relative link to the one pool file.
    Symlink,
    /// A real copy of the file (the disk cannot link).
    Copy,
}

/// The pool's link mode as the Family page shows it (`links`, `message`, `fix`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkStatus {
    pub links: LinkMode,
    /// What the owner reads when links do not work; None when they do.
    pub message: Option<String>,
    /// How to make them work; None when they do.
    pub fix: Option<String>,
}

impl LinkStatus {
    pub fn of(mode: LinkMode) -> Self {
        match mode {
            LinkMode::Symlink => LinkStatus {
                links: mode,
                message: None,
                fix: None,
            },
            LinkMode::Copy => LinkStatus {
                links: mode,
                message: Some(COPY_MODE_MESSAGE.to_string()),
                fix: Some(COPY_MODE_FIX.to_string()),
            },
        }
    }
}

/// Why a pool change could not be made.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("{0} is not a family member")]
    NotMember(String),
    #[error("{0} is not a file in the family pool")]
    NotInPool(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl PoolError {
    /// What a member reads about it: plain words, never a path inside the server.
    pub fn for_member(&self) -> &'static str {
        match self {
            PoolError::NotMember(_) => "Only family members have their own library.",
            PoolError::NotInPool(_) => "That copy is no longer in the family library.",
            PoolError::Io(_) => "Octo could not put the song in your library. Try again later.",
        }
    }
}

/// How many times a link Navidrome did not show had to become a copy that it then showed,
/// with no link ever shown, before every member gets copies.
pub const UNSHOWN_LINKS_FOR_COPIES: u32 = 2;

/// How patiently a moved or new file is waited for in the library index.
#[derive(Debug, Clone, Copy)]
pub struct PoolTiming {
    /// Ask again this often (and read the index again when it is older than this).
    pub poll: Duration,
    /// Give up after this long.
    pub max_wait: Duration,
}

impl Default for PoolTiming {
    fn default() -> Self {
        PoolTiming {
            poll: Duration::from_secs(15),
            max_wait: Duration::from_secs(10 * 60),
        }
    }
}

/// Stars as a member sees them, for a song whose id changes when its file moves.
#[async_trait]
pub trait MemberStars: Send + Sync {
    /// Whether `user` has starred library song `song_id`.
    async fn is_starred(&self, user: &str, song_id: &str) -> bool;
    /// Stars library song `song_id` as `user`. True when Navidrome said ok.
    async fn star(&self, user: &str, song_id: &str) -> bool;
}

/// [`MemberStars`] through Navidrome, signed in as the member with a sign-in made at the
/// moment of use from their current hidden password.
pub struct NavidromeMemberStars {
    settings: Arc<SettingsStore>,
    http: reqwest::Client,
    credentials: Arc<MemberCredentials>,
}

impl NavidromeMemberStars {
    pub fn new(
        settings: Arc<SettingsStore>,
        http: reqwest::Client,
        credentials: Arc<MemberCredentials>,
    ) -> Self {
        NavidromeMemberStars {
            settings,
            http,
            credentials,
        }
    }

    async fn call(&self, user: &str, endpoint: &str, song_id: &str) -> Option<serde_json::Value> {
        let base = self.settings.current().subsonic.url.clone().unwrap_or_default();
        if base.trim().is_empty() {
            return None;
        }
        let credential = self.credentials.fresh(user)?;
        let parameters = credential.parameters(&[("f", "json"), ("id", song_id)]);
        let url = format!("{}/rest/{endpoint}", base.trim_end_matches('/'));
        let response = self.http.get(&url).query(&parameters).send().await.ok()?;
        let body: serde_json::Value = response.json().await.ok()?;
        let envelope = body.get("subsonic-response")?.clone();
        (envelope.get("status").and_then(|s| s.as_str()) == Some("ok")).then_some(envelope)
    }
}

#[async_trait]
impl MemberStars for NavidromeMemberStars {
    async fn is_starred(&self, user: &str, song_id: &str) -> bool {
        self.call(user, "getSong", song_id)
            .await
            .is_some_and(|envelope| envelope.pointer("/song/starred").is_some())
    }

    async fn star(&self, user: &str, song_id: &str) -> bool {
        self.call(user, "star", song_id).await.is_some()
    }
}

/// What the download service asks of the family pool while it places a file. With Family off
/// [`FamilyPlacement::pool_root`] is None and the download goes exactly as it always has.
#[async_trait]
pub trait FamilyPlacement: Send + Sync {
    /// Where member downloads are placed (the pool folder); None while Family is off.
    fn pool_root(&self) -> Option<String>;
    /// Whether a path is a file in the pool.
    fn is_pool_path(&self, path: &str) -> bool;
    /// A file in the pool holding this song: by its outside id, then its ISRC, then its artist
    /// and title.
    fn find_song(&self, provider: &str, external_id: &str, song: &Song) -> Option<PoolFile>;
    /// Whether a pool file is a FLAC the spectrum check found was made from lossy audio.
    fn made_from_lossy(&self, path: &str) -> bool;
    /// The path the first member's link has, for a file about to be linked.
    fn link_path_for(&self, pool_path: &str, user: &str) -> Option<String>;
    /// Records a file just placed in the pool and links it for `members`. A copy of the same
    /// song already in the pool that this one betters is replaced for everyone. Answers the
    /// first member's link.
    async fn placed(
        &self,
        path: &str,
        song: &Song,
        provider: &str,
        external_id: &str,
        members: &[String],
    ) -> Option<String>;
    /// Links a finished download for every member who asked, and moves it into the shared
    /// library when someone wants it there. Answers where the file now is.
    async fn settle(&self, path: &str, members: &[String], wants_shared: bool) -> String;
}

/// A member's star or playlist entry to carry over to the song's new id once the index finds
/// it.
#[derive(Debug, Clone)]
struct Remap {
    user: String,
    old_id: Option<String>,
    starred: bool,
    new_path: String,
    scope: String,
}

/// The family pool. See the module comment.
pub struct FamilyPool {
    me: Weak<FamilyPool>,
    settings: Arc<SettingsStore>,
    store: Arc<FamilyPoolStore>,
    family: Arc<FamilyStore>,
    index: Arc<LibraryIndex>,
    library: Arc<dyn ILocalLibraryService>,
    playlists: Arc<FamilyPlaylistStore>,
    stars: Arc<dyn MemberStars>,
    quarantine: LibraryActionQuarantine,
    /// The shared library's folder, read at use (it follows Navidrome's detected folder).
    shared_root: Box<dyn Fn() -> String + Send + Sync>,
    mode: RwLock<LinkMode>,
    /// Held for the whole of every change to the pool.
    changes: tokio::sync::Mutex<()>,
    /// Pool files the last sweep found missing; a file goes only when the next sweep finds it
    /// missing again.
    missing_once: Mutex<HashSet<String>>,
    timing: PoolTiming,
}

/// The services a [`FamilyPool`] works with.
pub struct PoolParts {
    pub settings: Arc<SettingsStore>,
    pub store: Arc<FamilyPoolStore>,
    pub family: Arc<FamilyStore>,
    pub index: Arc<LibraryIndex>,
    pub library: Arc<dyn ILocalLibraryService>,
    pub playlists: Arc<FamilyPlaylistStore>,
    pub stars: Arc<dyn MemberStars>,
    pub shared_root: Box<dyn Fn() -> String + Send + Sync>,
}

impl FamilyPool {
    pub fn new(parts: PoolParts) -> Arc<Self> {
        Self::with_timing(parts, PoolTiming::default())
    }

    /// The same pool, waiting for the index on another schedule (tests).
    pub fn with_timing(parts: PoolParts, timing: PoolTiming) -> Arc<Self> {
        let quarantine = LibraryActionQuarantine::new(Arc::clone(&parts.settings));
        let mode = if parts.store.link_state().copies {
            LinkMode::Copy
        } else {
            LinkMode::Symlink
        };
        Arc::new_cyclic(|me| FamilyPool {
            me: me.clone(),
            settings: parts.settings,
            store: parts.store,
            family: parts.family,
            index: parts.index,
            library: parts.library,
            playlists: parts.playlists,
            stars: parts.stars,
            quarantine,
            shared_root: parts.shared_root,
            mode: RwLock::new(mode),
            changes: tokio::sync::Mutex::new(()),
            missing_once: Mutex::new(HashSet::new()),
            timing,
        })
    }

    fn arc(&self) -> Option<Arc<FamilyPool>> {
        self.me.upgrade()
    }

    /// Whether Family is on.
    pub fn active(&self) -> bool {
        self.settings.current().family.enabled
    }

    pub fn store(&self) -> &Arc<FamilyPoolStore> {
        &self.store
    }

    pub fn index(&self) -> &Arc<LibraryIndex> {
        &self.index
    }

    pub fn timing(&self) -> PoolTiming {
        self.timing
    }

    /// The members folder, without a trailing slash.
    pub fn members_root(&self) -> String {
        self.settings.current().family.effective_members_path()
    }

    /// The pool folder, `<members>/.pool`.
    pub fn pool_root_path(&self) -> String {
        format!("{}/{POOL_FOLDER}", self.members_root())
    }

    /// A member's own folder, `<members>/<user>` (their name as Octo keeps it).
    pub fn member_root(&self, user: &str) -> String {
        let name = self
            .family
            .member(user)
            .map(|m| m.username)
            .unwrap_or_else(|| user.trim().to_string());
        format!("{}/{name}", self.members_root())
    }

    /// A pool file's path inside the pool (`Artist/Album/01 Song.flac`).
    pub fn relative_in_pool(&self, path: &str) -> Option<String> {
        let rest = path.strip_prefix(&self.pool_root_path())?.strip_prefix('/')?;
        (!rest.is_empty() && !rest.split('/').any(|part| part == "..")).then(|| rest.to_string())
    }

    /// Where `user`'s link to a pool file is.
    pub fn link_path(&self, pool_path: &str, user: &str) -> Option<String> {
        let relative = self.relative_in_pool(pool_path)?;
        Some(format!("{}/{relative}", self.member_root(user)))
    }

    /// The pool file a member's link (or copy) stands for.
    pub fn pool_path_of_link(&self, user: &str, link: &str) -> Option<String> {
        let root = self.member_root(user);
        let relative = link.strip_prefix(&root)?.strip_prefix('/')?;
        (!relative.is_empty()).then(|| format!("{}/{relative}", self.pool_root_path()))
    }

    /// How members' libraries get pool songs now.
    pub fn mode(&self) -> LinkMode {
        *self.mode.read()
    }

    /// The link mode with the words the Family page shows for it.
    pub fn link_status(&self) -> LinkStatus {
        LinkStatus::of(self.mode())
    }

    /// Tries a link in the members folder: makes a file, links it, reads through the link and
    /// removes both. Links when that works, copies when it does not, and copies whenever
    /// Octo has learned that Navidrome does not show links here.
    pub fn probe_links(&self) -> LinkStatus {
        let mode = if !self.store.link_state().copies && probe(Path::new(&self.members_root())) {
            LinkMode::Symlink
        } else {
            LinkMode::Copy
        };
        *self.mode.write() = mode;
        info!("FAMILY members folder links: {mode:?}");
        self.link_status()
    }

    /// Whether Navidrome has shown a linked song in a member's library yet.
    pub fn links_seen(&self) -> bool {
        self.mode() == LinkMode::Copy || self.store.link_state().seen
    }

    /// Navidrome showed a linked song in a member's library. Kept, so links are never given up
    /// on after that.
    pub fn confirm_links_seen(&self) {
        self.store.set_link_state(|links| links.seen = true);
    }

    /// Navidrome did not show `user`'s link at `link` in time. Only that one link becomes a
    /// real copy (written beside it first, then put in its place), and their library is scanned
    /// for it. Nothing changes once a link has ever been shown, or for anything but a link.
    /// Answers whether the link became a copy.
    pub async fn copy_unshown_link(&self, user: &str, link: &str) -> bool {
        if self.links_seen() || !is_symlink(link) {
            return false;
        }
        let _changing = self.changes.lock().await;
        let path = PathBuf::from(link);
        let copied = tokio::task::spawn_blocking(move || replace_link_with_copy(&path))
            .await
            .unwrap_or_else(|e| Err(io::Error::other(e.to_string())));
        if let Err(e) = copied {
            warn!("FAMILY could not make a copy of {link} for {user}: {e}");
            return false;
        }
        if let Some(library_id) = self.family.member(user).and_then(|m| m.personal_library_id) {
            let size = std::fs::metadata(link).map(|m| m.len()).unwrap_or(0);
            self.index.note_placed(library_id, link, size);
            self.library.trigger_library_scan_of(library_id).await;
        }
        info!("FAMILY Navidrome did not show {user}'s link {link}; it is a copy now");
        true
    }

    /// Navidrome showed a copy where it never showed the link before it. Once that has
    /// happened [`UNSHOWN_LINKS_FOR_COPIES`] times with no link ever shown, every member gets
    /// copies from now on ([`Self::fall_back_to_copies`]). Answers whether that happened now.
    pub async fn copy_shown_where_link_was_not(&self) -> bool {
        if self.links_seen() {
            return false;
        }
        self.store
            .set_link_state(|links| links.unshown = links.unshown.saturating_add(1));
        if self.store.link_state().unshown < UNSHOWN_LINKS_FOR_COPIES {
            return false;
        }
        self.fall_back_to_copies().await;
        true
    }

    /// Links do not work for Navidrome here: every member gets real copies from now on (kept
    /// across restarts), and every link made so far becomes a copy, written beside the link
    /// before it takes the link's place. Answers how many links were replaced.
    pub async fn fall_back_to_copies(&self) -> usize {
        let _changing = self.changes.lock().await;
        *self.mode.write() = LinkMode::Copy;
        self.store.set_link_state(|links| links.copies = true);
        warn!("FAMILY Navidrome did not show linked songs; members get copies from now on");
        let mut links = Vec::new();
        for file in self.store.list() {
            for user in &file.refs {
                if let Some(link) = self.link_path(&file.path, user) {
                    links.push(PathBuf::from(link));
                }
            }
        }
        tokio::task::spawn_blocking(move || {
            links
                .iter()
                .filter(|link| is_symlink(&link.to_string_lossy()))
                .filter(|link| match replace_link_with_copy(link) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("FAMILY could not make a copy of {}: {e}", link.display());
                        false
                    }
                })
                .count()
        })
        .await
        .unwrap_or(0)
    }

    /// A pool file holding this song: its outside id, then its ISRC, then its artist and title
    /// (one version: a live take is its own song). Only files still on disk count.
    pub fn find(
        &self,
        outside_key: Option<&str>,
        isrc: Option<&str>,
        artist: &str,
        title: &str,
    ) -> Option<PoolFile> {
        let on_disk = |file: &PoolFile| Path::new(&file.path).is_file();
        if let Some(found) = outside_key
            .and_then(|key| self.store.find_by_outside_id(key))
            .filter(on_disk)
        {
            return Some(found);
        }
        if let Some(found) = isrc
            .and_then(|isrc| self.store.find_by_isrc(isrc))
            .filter(on_disk)
        {
            return Some(found);
        }
        let wanted = SongIdentity::match_key(artist, title);
        if wanted.is_empty() {
            return None;
        }
        self.store
            .list()
            .into_iter()
            .filter(on_disk)
            .find(|file| SongIdentity::match_key(&file.artist, &file.title) == wanted)
    }

    /// Records a file in the pool (or updates its record).
    pub fn record(&self, file: PoolFile) {
        self.store.upsert(file);
    }

    /// The member named `user`, when their name is one a member's own folder can have. A name
    /// that starts with a dot, or is not one plain folder name, would reach the pool or the
    /// members folder itself, so it never gets a library here.
    fn member_named(&self, user: &str) -> Result<Member, PoolError> {
        self.family
            .member(user)
            .filter(|member| valid_username(&member.username))
            .ok_or_else(|| PoolError::NotMember(user.to_string()))
    }

    /// Puts a pool file in `user`'s library: a relative link (or a copy in copy mode), plus the
    /// song's lyrics files and the folder's cover. Records the member's reference, tells the
    /// index the file is there, and starts a scan of their library. Doing it twice changes
    /// nothing. Answers the link's path.
    pub async fn link(&self, pool_path: &str, user: &str) -> Result<String, PoolError> {
        let _changing = self.changes.lock().await;
        self.link_held(pool_path, user).await
    }

    /// [`Self::link`] for a change that holds the lock already.
    async fn link_held(&self, pool_path: &str, user: &str) -> Result<String, PoolError> {
        let member = self.member_named(user)?;
        let relative = self
            .relative_in_pool(pool_path)
            .filter(|_| Path::new(pool_path).is_file())
            .ok_or_else(|| PoolError::NotInPool(pool_path.to_string()))?;
        let link = format!("{}/{relative}", self.member_root(&member.username));
        self.make_link(pool_path, &link, &relative)?;
        for companion in companions(pool_path) {
            let Some(companion_relative) = self.relative_in_pool(&companion) else {
                continue;
            };
            let companion_link = format!("{}/{companion_relative}", self.member_root(&member.username));
            if let Err(e) = self.make_link(&companion, &companion_link, &companion_relative) {
                warn!("FAMILY could not give {} {companion}: {e}", member.username);
            }
        }
        let size = std::fs::metadata(pool_path).map(|m| m.len()).unwrap_or(0);
        if self.store.get(pool_path).is_none() {
            self.store.upsert(unrecorded_file(pool_path, size, Vec::new()));
        }
        self.store.add_ref(pool_path, &member.username);
        if let Some(library_id) = member.personal_library_id {
            self.index.note_placed(library_id, &link, size);
            self.library.trigger_library_scan_of(library_id).await;
        }
        Ok(link)
    }

    /// Takes a pool file out of `user`'s library and drops their reference; the file goes to
    /// the trash once nobody has it.
    pub async fn unlink(&self, pool_path: &str, user: &str) -> Result<(), PoolError> {
        let _changing = self.changes.lock().await;
        let member = self.member_named(user)?;
        self.remove_link(pool_path, &member.username, &companions(pool_path));
        let left = self.store.remove_ref(pool_path, &member.username);
        if left.is_none_or(|left| left == 0) {
            self.gc_held(pool_path).await;
        }
        if let Some(library_id) = member.personal_library_id {
            self.library.trigger_library_scan_of(library_id).await;
        }
        Ok(())
    }

    /// Forgets pool files that are no longer on disk (someone deleted one by hand): each
    /// member's link to it goes, and their library is scanned. A file goes only when two sweeps
    /// in a row find it missing, and nothing goes while the pool folder cannot be read or is
    /// empty (a members disk that is not mounted looks like every file gone). Answers how many
    /// files went.
    pub async fn sweep_dangling(&self) -> usize {
        let _changing = self.changes.lock().await;
        self.sweep_held().await
    }

    /// [`Self::sweep_dangling`] for a change that holds the lock already.
    async fn sweep_held(&self) -> usize {
        let recorded = self.store.list();
        if recorded.is_empty() {
            self.missing_once.lock().clear();
            return 0;
        }
        let root = self.pool_root_path();
        let readable = std::fs::read_dir(&root).is_ok_and(|mut entries| entries.next().is_some());
        if !readable {
            warn!(
                "FAMILY the pool folder {root} cannot be read or is empty; its records are kept as they are"
            );
            self.missing_once.lock().clear();
            return 0;
        }
        let missing: Vec<PoolFile> = recorded
            .into_iter()
            .filter(|file| !Path::new(&file.path).exists())
            .collect();
        let gone: Vec<PoolFile> = {
            let mut once = self.missing_once.lock();
            let now: HashSet<String> = missing.iter().map(|file| file.path.clone()).collect();
            let gone = missing
                .into_iter()
                .filter(|file| once.contains(&file.path))
                .collect();
            *once = now;
            gone
        };
        for file in &gone {
            for user in &file.refs {
                self.remove_link(&file.path, user, &[]);
                if let Some(library_id) = self.family.member(user).and_then(|m| m.personal_library_id) {
                    self.library.trigger_library_scan_of(library_id).await;
                }
            }
            self.store.remove(&file.path);
            self.missing_once.lock().remove(&file.path);
            if let Err(e) = self.library.forget_mapping(&file.path).await {
                warn!("FAMILY could not forget {}'s mapping: {e}", file.path);
            }
            warn!(
                "FAMILY {} is gone from the pool; removed the links of {}",
                file.path,
                file.refs.join(", ")
            );
        }
        gone.len()
    }

    /// Sends a pool file nobody links any more to the trash (kept as long as library actions
    /// keep removed songs), with its lyrics files. A file a member's library still links on
    /// disk is never trashed, whatever the record says: the record learns of them again.
    /// True when it went.
    pub async fn gc(&self, pool_path: &str) -> bool {
        let _changing = self.changes.lock().await;
        self.gc_held(pool_path).await
    }

    /// [`Self::gc`] for a change that holds the lock already.
    async fn gc_held(&self, pool_path: &str) -> bool {
        if self
            .store
            .get(pool_path)
            .is_some_and(|file| !file.refs.is_empty())
        {
            return false;
        }
        let holders = self.linked_on_disk(pool_path);
        if !holders.is_empty() {
            self.restore_refs(pool_path, &holders);
            warn!(
                "FAMILY {pool_path} had no record of who has it, but {} still do; it stays",
                holders.join(", ")
            );
            return false;
        }
        let members_root = self.members_root();
        let mut gone = !Path::new(pool_path).exists();
        if !gone {
            let size = std::fs::metadata(pool_path).map(|m| m.len() as i64).unwrap_or(0);
            let file = ResolvedSongFile {
                navidrome_id: String::new(),
                absolute_path: pool_path.to_string(),
                size_bytes: size,
                title: String::new(),
                artist: String::new(),
                album: String::new(),
                suffix: extension_of(pool_path),
                duration_seconds: None,
                source: PathSource::LocalMappings,
                album_artist: None,
            };
            let moved = self.quarantine.move_file_with_sidecars(
                &file,
                &members_root,
                LibraryAction::Delete,
                "family",
                true,
            );
            if moved.moved {
                info!("FAMILY nobody has {pool_path} any more; it went to the trash");
                gone = true;
            } else {
                warn!(
                    "FAMILY could not move {pool_path} to the trash: {}",
                    moved.error.unwrap_or_default()
                );
            }
        }
        if !gone {
            return false;
        }
        if let Some(folder) = Path::new(pool_path).parent() {
            remove_lonely_covers(folder);
            prune_empty(folder, Path::new(&self.pool_root_path()));
        }
        self.store.remove(pool_path);
        if let Err(e) = self.library.forget_mapping(pool_path).await {
            warn!("FAMILY could not forget {pool_path}'s mapping: {e}");
        }
        self.quarantine.sweep(&members_root);
        true
    }

    /// The members whose own library has something (a link, or a copy) at this pool file's
    /// place.
    fn linked_on_disk(&self, pool_path: &str) -> Vec<String> {
        let Some(relative) = self.relative_in_pool(pool_path) else {
            return Vec::new();
        };
        let root = self.members_root();
        self.family
            .members()
            .into_iter()
            .map(|member| member.username)
            .filter(|name| valid_username(name))
            .filter(|name| std::fs::symlink_metadata(format!("{root}/{name}/{relative}")).is_ok())
            .collect()
    }

    /// Records `holders` as linking a pool file, recording the file first when it is not.
    fn restore_refs(&self, pool_path: &str, holders: &[String]) {
        if self.store.get(pool_path).is_none() {
            let size = std::fs::metadata(pool_path).map(|m| m.len()).unwrap_or(0);
            self.store
                .upsert(unrecorded_file(pool_path, size, holders.to_vec()));
            return;
        }
        for holder in holders {
            self.store.add_ref(pool_path, holder);
        }
    }

    /// A better copy of a song replaces `old_path` for everyone who has it: each member's link
    /// moves to `new_path`, the old file goes to the trash, and their stars and family playlist
    /// entries follow the song once the index finds it again. Answers the members moved.
    pub async fn repoint(&self, old_path: &str, new_path: &str) -> Vec<String> {
        let _changing = self.changes.lock().await;
        self.repoint_held(old_path, new_path).await
    }

    /// [`Self::repoint`] for a change that holds the lock already.
    async fn repoint_held(&self, old_path: &str, new_path: &str) -> Vec<String> {
        if old_path == new_path {
            return Vec::new();
        }
        let Some(old) = self.store.get(old_path) else {
            return Vec::new();
        };
        let mut remaps = Vec::new();
        let mut moved = Vec::new();
        for user in &old.refs {
            let (Some(old_link), Some(new_link)) =
                (self.link_path(old_path, user), self.link_path(new_path, user))
            else {
                continue;
            };
            let old_id = self.index.find_by_path(&old_link).map(|s| s.id);
            let starred = match &old_id {
                Some(id) => self.stars.is_starred(user, id).await,
                None => false,
            };
            match self.link_held(new_path, user).await {
                Ok(_) => {
                    self.remove_link(old_path, user, &companions(old_path));
                    self.store.remove_ref(old_path, user);
                    remaps.push(Remap {
                        user: user.clone(),
                        old_id,
                        starred,
                        new_path: new_link,
                        scope: format!("Personal:{user}"),
                    });
                    moved.push(user.clone());
                }
                Err(e) => warn!("FAMILY could not move {user} to the better copy {new_path}: {e}"),
            }
        }
        // Whoever bettered the copy is recorded too, so the store follows the outside ids.
        if let Some(mut better) = self.store.get(new_path) {
            for id in old.outside_ids {
                if !better.outside_ids.contains(&id) {
                    better.outside_ids.push(id);
                }
            }
            if better.isrc.is_none() {
                better.isrc = old.isrc;
            }
            self.store.upsert(better);
        }
        self.gc_held(old_path).await;
        info!(
            "FAMILY {} moved from {old_path} to the better copy {new_path}",
            moved.join(", ")
        );
        self.carry_over(remaps);
        moved
    }

    /// The shared library wants a pool song: the file moves into the shared library's layout,
    /// every member's link goes (they see it there now), and their stars and family playlist
    /// entries follow the song. Still one file. A member who cannot see that shared library
    /// keeps their link, and the pool keeps the file for them: then the shared library gets a
    /// copy. Answers its new path.
    pub async fn promote(&self, pool_path: &str) -> Result<String, PoolError> {
        let _changing = self.changes.lock().await;
        self.promote_held(pool_path, None).await
    }

    /// [`Self::promote`] for a change that holds the lock already. `leaving` (a member being let
    /// go) never keeps the file in the pool, whatever they can see.
    async fn promote_held(&self, pool_path: &str, leaving: Option<&str>) -> Result<String, PoolError> {
        let relative = self
            .relative_in_pool(pool_path)
            .filter(|_| Path::new(pool_path).is_file())
            .ok_or_else(|| PoolError::NotInPool(pool_path.to_string()))?;
        let shared_root = (self.shared_root)();
        let shared_root = shared_root.trim_end_matches('/');
        let mut target = format!("{shared_root}/{relative}");
        if Path::new(&target).exists() {
            target = PathHelper::resolve_unique_path(&target);
        }
        let refs = self.store.get(pool_path).map(|f| f.refs).unwrap_or_default();
        let shared_library = self.index.library_of_folder(shared_root);
        let (seers, keepers): (Vec<String>, Vec<String>) = refs.into_iter().partition(|user| {
            leaving.is_some_and(|leaving| eq_ignore_case(leaving, user))
                || self
                    .family
                    .member(user)
                    .is_none_or(|member| sees_shared_library(&member, shared_library))
        });
        let mut remaps = Vec::new();
        for user in &seers {
            let Some(link) = self.link_path(pool_path, user) else {
                continue;
            };
            let old_id = self.index.find_by_path(&link).map(|s| s.id);
            let starred = match &old_id {
                Some(id) => self.stars.is_starred(user, id).await,
                None => false,
            };
            remaps.push(Remap {
                user: user.clone(),
                old_id,
                starred,
                new_path: target.clone(),
                scope: "Shared".to_string(),
            });
        }

        let stays_pooled = !keepers.is_empty();
        let companions = companions(pool_path);
        if stays_pooled {
            copy_file(Path::new(pool_path), Path::new(&target))?;
        } else {
            move_file(Path::new(pool_path), Path::new(&target))?;
        }
        for companion in &companions {
            let Some(name) = Path::new(companion).file_name() else {
                continue;
            };
            let destination = Path::new(&target).with_file_name(name);
            if destination.exists() {
                continue;
            }
            let is_cover = COVER_FILES
                .iter()
                .any(|cover| eq_ignore_case(cover, &name.to_string_lossy()));
            let result = if is_cover || stays_pooled {
                std::fs::copy(companion, &destination).map(|_| ())
            } else {
                move_file(Path::new(companion), &destination)
            };
            if let Err(e) = result {
                warn!("FAMILY could not move {companion} with its song: {e}");
            }
        }
        for user in &seers {
            self.remove_link(pool_path, user, &companions);
        }
        if stays_pooled {
            for user in &seers {
                self.store.remove_ref(pool_path, user);
            }
        } else {
            self.store.remove(pool_path);
            if let Some(folder) = Path::new(pool_path).parent() {
                remove_lonely_covers(folder);
                prune_empty(folder, Path::new(&self.pool_root_path()));
            }
        }
        match self.library.forget_mapping(pool_path).await {
            Ok(mappings) => {
                if let Err(e) = self.library.restore_mappings(&mappings, &target).await {
                    warn!("FAMILY could not point {target}'s mappings at it: {e}");
                }
            }
            Err(e) => warn!("FAMILY could not forget {pool_path}'s mapping: {e}"),
        }
        self.library.trigger_library_scan(true).await;
        for user in &seers {
            if let Some(library_id) = self.family.member(user).and_then(|m| m.personal_library_id) {
                self.library.trigger_library_scan_of(library_id).await;
            }
        }
        if stays_pooled {
            info!(
                "FAMILY {pool_path} was copied into the shared library as {target}; it stays in the pool for {}, who cannot see that library",
                keepers.join(", ")
            );
        } else {
            info!("FAMILY {pool_path} moved into the shared library as {target}");
        }
        self.carry_over(remaps);
        Ok(target)
    }

    /// Whether `path` is a file of the shared library: neither in the pool nor in a member's
    /// own folder.
    pub fn is_shared_file(&self, path: &str) -> bool {
        let members = format!("{}/", self.members_root());
        !path.trim().is_empty() && !path.starts_with(&members)
    }

    /// The shared library a shared file is in: the index's answer for the file, else the
    /// library of the nearest folder above it that the index holds songs in, up to the shared
    /// folder. None when the index cannot tell yet.
    pub fn shared_library_of(&self, path: &str) -> Option<i64> {
        if let Some(library) = self.index.find_by_path(path).and_then(|song| song.library_id) {
            return Some(library);
        }
        let shared_root = (self.shared_root)();
        let shared_root = shared_root.trim_end_matches('/');
        if shared_root.is_empty() {
            return None;
        }
        let inside = format!("{shared_root}/");
        let mut folder = Path::new(path).parent();
        while let Some(dir) = folder {
            let text = dir.to_string_lossy();
            if text != shared_root && !text.starts_with(&inside) {
                return None;
            }
            if let Some(library) = self.index.library_of_folder(&text) {
                return Some(library);
            }
            if text == shared_root {
                return None;
            }
            folder = dir.parent();
        }
        None
    }

    /// Whether `user` sees the shared library a shared file is in. Anyone who is not a member
    /// (the owner) sees every library.
    pub fn sees_shared_file(&self, user: &str, path: &str) -> bool {
        self.family
            .member(user)
            .is_none_or(|member| sees_shared_library(&member, self.shared_library_of(path)))
    }

    /// A shared file's place in the pool: its path under the shared folder, or its last three
    /// parts (`Artist/Album/file`) when it is somewhere else.
    fn pool_place_of_shared(&self, shared_path: &str) -> Option<String> {
        let shared_root = (self.shared_root)();
        let shared_root = shared_root.trim_end_matches('/');
        let under_root = (!shared_root.is_empty())
            .then(|| shared_path.strip_prefix(shared_root)?.strip_prefix('/'))
            .flatten()
            .map(str::to_string);
        let relative = under_root.unwrap_or_else(|| {
            let mut parts: Vec<&str> = shared_path.rsplit('/').take(3).collect();
            parts.reverse();
            parts.join("/")
        });
        let plain = !relative.is_empty()
            && relative
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        plain.then(|| format!("{}/{relative}", self.pool_root_path()))
    }

    /// Gives `user`, who cannot see the shared library a shared file is in, a copy they can
    /// play through the pool: the file, with its lyrics and its folder's cover, is copied once
    /// into the pool at its place in the shared layout, recorded as a copy of `song`, and
    /// linked into their library like any pool song. Another member who needs it gets a link
    /// to that same pool file. The copy is written on a blocking thread while the pool lock is
    /// held. Answers their link.
    pub async fn copy_shared_for(
        &self,
        shared_path: &str,
        user: &str,
        outside_key: Option<&str>,
        song: Option<&Song>,
    ) -> Result<String, PoolError> {
        let _changing = self.changes.lock().await;
        let member = self.member_named(user)?;
        if !Path::new(shared_path).is_file() {
            return Err(PoolError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{shared_path} is not there"),
            )));
        }
        let target = self
            .pool_place_of_shared(shared_path)
            .ok_or_else(|| PoolError::NotInPool(shared_path.to_string()))?;
        if !Path::new(&target).is_file() {
            let (from, to) = (PathBuf::from(shared_path), PathBuf::from(&target));
            tokio::task::spawn_blocking(move || copy_with_companions(&from, &to))
                .await
                .unwrap_or_else(|e| Err(io::Error::other(e.to_string())))?;
            info!(
                "FAMILY {shared_path} was copied into the pool as {target} for {}, who cannot see its shared library",
                member.username
            );
        }
        self.record_shared_copy(&target, outside_key, song);
        self.link_held(&target, &member.username).await
    }

    /// Records a pool copy of a shared file as a copy of `song`, or adds the outside id to the
    /// record it already has.
    fn record_shared_copy(&self, pool_path: &str, outside_key: Option<&str>, song: Option<&Song>) {
        if let Some(mut file) = self.store.get(pool_path) {
            if let Some(key) = outside_key
                && !file.outside_ids.iter().any(|id| id == key)
            {
                file.outside_ids.push(key.to_string());
                self.store.upsert(file);
            }
            return;
        }
        let format = extension_of(pool_path);
        self.store.upsert(PoolFile {
            path: pool_path.to_string(),
            size: std::fs::metadata(pool_path).map(|m| m.len()).unwrap_or(0),
            transcoded_from: song
                .and_then(|s| s.transcoded_from.clone())
                .filter(|from| format == "flac" && !from.trim().is_empty()),
            format,
            outside_ids: outside_key.map(str::to_string).into_iter().collect(),
            isrc: song
                .and_then(|s| s.isrc.clone())
                .filter(|isrc| !isrc.trim().is_empty()),
            title: song.map(|s| s.title.clone()).unwrap_or_default(),
            artist: song.map(|s| s.artist.clone()).unwrap_or_default(),
            album: song.map(|s| s.album.clone()).unwrap_or_default(),
            refs: Vec::new(),
        });
    }

    /// Lets go of everything a member who is leaving the family has: each of their links goes,
    /// and each pool file only they had follows `choice` (kept aside under
    /// `<members>/.kept/<user>/`, moved into the shared library, or sent to the trash). Files
    /// another member still has stay where they are. A file that cannot be kept aside or moved
    /// stays exactly as it was, linked and recorded for the member, and the first such failure
    /// is the answer once every other file is done, so nothing is left behind without a record.
    /// Answers what became of the files.
    pub async fn release_member(&self, user: &str, choice: FilesChoice) -> Result<Released, PoolError> {
        let _changing = self.changes.lock().await;
        let member = self.member_named(user)?;
        let name = member.username.clone();
        let mut released = Released::default();
        let mut failure: Option<PoolError> = None;
        for file in self.store.refs_of(&name) {
            let companions = companions(&file.path);
            let only_theirs = file.refs.iter().all(|r| eq_ignore_case(r, &name));
            if !only_theirs {
                self.remove_link(&file.path, &name, &companions);
                self.store.remove_ref(&file.path, &name);
                released.shared += 1;
                continue;
            }
            if !Path::new(&file.path).is_file() {
                self.remove_link(&file.path, &name, &companions);
                self.store.remove(&file.path);
                continue;
            }
            let outcome = match choice {
                FilesChoice::Keep => self.keep_aside(&file.path, &name, &companions).await.map(|()| {
                    self.remove_link(&file.path, &name, &companions);
                    released.kept += 1;
                }),
                FilesChoice::Promote => self
                    .promote_held(&file.path, Some(&name))
                    .await
                    .map(|_| released.promoted += 1),
                FilesChoice::Delete => {
                    self.remove_link(&file.path, &name, &companions);
                    self.store.remove_ref(&file.path, &name);
                    if self.gc_held(&file.path).await {
                        released.trashed += 1;
                    } else if Path::new(&file.path).exists() {
                        released.shared += 1;
                    }
                    Ok(())
                }
            };
            if let Err(e) = outcome {
                warn!("FAMILY {}'s file {} stays as it was: {e}", name, file.path);
                failure.get_or_insert(e);
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        // Whatever is left of their folder is empty folders and covers.
        let root = PathBuf::from(self.member_root(&name));
        prune_folder_tree(&root);
        info!(
            "FAMILY released {name}'s files ({choice:?}): {} kept, {} promoted, {} trashed, {} still with others",
            released.kept, released.promoted, released.trashed, released.shared
        );
        Ok(released)
    }

    /// Moves a pool file only `user` had to `<members>/.kept/<user>/<layout>`, with its lyrics
    /// and cover, out of every library. Nothing changes when the song itself cannot be moved.
    async fn keep_aside(&self, pool_path: &str, user: &str, companions: &[String]) -> Result<(), PoolError> {
        let relative = self
            .relative_in_pool(pool_path)
            .ok_or_else(|| PoolError::NotInPool(pool_path.to_string()))?;
        let mut target = format!("{}/{KEPT_FOLDER}/{user}/{relative}", self.members_root());
        if Path::new(&target).exists() {
            target = PathHelper::resolve_unique_path(&target);
        }
        move_file(Path::new(pool_path), Path::new(&target))?;
        for companion in companions {
            let Some(name) = Path::new(companion).file_name() else {
                continue;
            };
            let destination = Path::new(&target).with_file_name(name);
            if destination.exists() {
                continue;
            }
            let is_cover = COVER_FILES
                .iter()
                .any(|cover| eq_ignore_case(cover, &name.to_string_lossy()));
            let result = if is_cover {
                std::fs::copy(companion, &destination).map(|_| ())
            } else {
                move_file(Path::new(companion), &destination)
            };
            if let Err(e) = result {
                warn!("FAMILY could not keep {companion} with its song: {e}");
            }
        }
        self.store.remove(pool_path);
        if let Some(folder) = Path::new(pool_path).parent() {
            remove_lonely_covers(folder);
            prune_empty(folder, Path::new(&self.pool_root_path()));
        }
        if let Err(e) = self.library.forget_mapping(pool_path).await {
            warn!("FAMILY could not forget {pool_path}'s mapping: {e}");
        }
        Ok(())
    }

    /// Waits until the library index finds a file at `path` (in `library_id` when given),
    /// reading the index again when it is older than the poll interval. None after
    /// [`PoolTiming::max_wait`].
    pub async fn wait_for_path(&self, path: &str, library_id: Option<i64>) -> Option<IndexedSong> {
        let started = tokio::time::Instant::now();
        loop {
            if let Some(song) = self
                .index
                .find_by_path(path)
                .filter(|song| library_id.is_none() || song.library_id == library_id)
            {
                if library_id.is_some()
                    && std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
                {
                    self.confirm_links_seen();
                }
                return Some(song);
            }
            if started.elapsed() >= self.timing.max_wait {
                return None;
            }
            let poll = TimeDelta::from_std(self.timing.poll).unwrap_or(TimeDelta::seconds(15));
            let stale = self.index.refreshed_at().is_none_or(|at| at < Utc::now() - poll);
            if stale && self.index.refresh().await.is_ok() {
                continue;
            }
            tokio::time::sleep(self.timing.poll).await;
        }
    }

    /// Stars `user`'s copy at `path` once the index finds it, without holding anything up.
    pub fn star_when_indexed(&self, user: &str, path: &str) {
        let Some(pool) = self.arc() else { return };
        let (user, path) = (user.to_string(), path.to_string());
        tokio::spawn(async move {
            // A link is looked for in their own library; a shared song anywhere.
            let library = pool
                .pool_path_of_link(&user, &path)
                .and_then(|_| pool.family.member(&user))
                .and_then(|m| m.personal_library_id);
            if let Some(song) = pool.wait_for_path(&path, library).await {
                pool.stars.star(&user, &song.id).await;
            }
        });
    }

    /// Carries stars and family playlist entries over to the new ids, once the index finds
    /// the files where they are now.
    fn carry_over(&self, remaps: Vec<Remap>) {
        if remaps.is_empty() {
            return;
        }
        let Some(pool) = self.arc() else { return };
        tokio::spawn(async move { pool.finish_remaps(remaps).await });
    }

    async fn finish_remaps(&self, remaps: Vec<Remap>) {
        let paths: Vec<&str> = remaps.iter().map(|r| r.new_path.as_str()).collect();
        let mut done: HashSet<&str> = HashSet::new();
        for path in paths {
            if !done.insert(path) {
                continue;
            }
            let Some(song) = self.wait_for_path(path, None).await else {
                warn!("FAMILY {path} did not show up in the library; stars on its old id stay there");
                continue;
            };
            for remap in remaps.iter().filter(|r| r.new_path == path) {
                if remap.starred && !self.stars.star(&remap.user, &song.id).await {
                    warn!("FAMILY could not star {path} again for {}", remap.user);
                }
                if let Some(old) = &remap.old_id {
                    self.playlists.remap_library_id(old, &song.id, &remap.scope);
                }
            }
        }
    }

    /// Makes `link` stand for `pool_path`: a relative link, or a copy in copy mode (and when the
    /// disk refuses the link). Nothing changes when something is there already.
    fn make_link(&self, pool_path: &str, link: &str, relative: &str) -> io::Result<()> {
        let link_path = Path::new(link);
        if std::fs::symlink_metadata(link_path).is_ok() {
            return Ok(());
        }
        if let Some(parent) = link_path.parent() {
            make_dirs(parent)?;
        }
        if self.mode() == LinkMode::Symlink {
            match symlink(&relative_target(relative), link_path) {
                Ok(()) => return Ok(()),
                Err(e) => warn!("FAMILY could not link {link}: {e}; copying it instead"),
            }
        }
        std::fs::copy(pool_path, link_path).map(|_| ())
    }

    /// Removes `user`'s link to a pool file, the links to its `companions` (read before the
    /// pool file moved), and their album folder's cover link and folders once nothing else is
    /// in them.
    fn remove_link(&self, pool_path: &str, user: &str, companions: &[String]) {
        let Some(link) = self.link_path(pool_path, user) else {
            return;
        };
        let link_path = Path::new(&link);
        if std::fs::symlink_metadata(link_path).is_ok()
            && let Err(e) = std::fs::remove_file(link_path)
        {
            warn!("FAMILY could not remove {link}: {e}");
        }
        for companion in companions {
            if COVER_FILES
                .iter()
                .any(|cover| companion.to_lowercase().ends_with(&format!("/{cover}")))
            {
                continue;
            }
            if let Some(companion_link) = self.link_path(companion, user) {
                let _ = std::fs::remove_file(companion_link);
            }
        }
        if let Some(folder) = link_path.parent() {
            remove_lonely_covers(folder);
            prune_empty(folder, Path::new(&self.member_root(user)));
        }
    }
}

#[async_trait]
impl FamilyPlacement for FamilyPool {
    fn pool_root(&self) -> Option<String> {
        self.active().then(|| self.pool_root_path())
    }

    fn is_pool_path(&self, path: &str) -> bool {
        self.active() && self.relative_in_pool(path).is_some()
    }

    fn find_song(&self, provider: &str, external_id: &str, song: &Song) -> Option<PoolFile> {
        if !self.active() {
            return None;
        }
        let key = key_of(provider, external_id);
        self.find(Some(&key), song.isrc.as_deref(), &song.artist, &song.title)
    }

    fn made_from_lossy(&self, path: &str) -> bool {
        self.store
            .get(path)
            .is_some_and(|file| file.transcoded_from.is_some())
    }

    fn link_path_for(&self, pool_path: &str, user: &str) -> Option<String> {
        self.link_path(pool_path, user)
    }

    async fn placed(
        &self,
        path: &str,
        song: &Song,
        provider: &str,
        external_id: &str,
        members: &[String],
    ) -> Option<String> {
        let _changing = self.changes.lock().await;
        // Each new pool file is a moment to let go of any that went missing by hand.
        self.sweep_held().await;
        let key = key_of(provider, external_id);
        let older = self
            .find(Some(&key), song.isrc.as_deref(), &song.artist, &song.title)
            .filter(|file| file.path != path);
        let format = extension_of(path);
        let placed = PoolFile {
            path: path.to_string(),
            size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            transcoded_from: song
                .transcoded_from
                .clone()
                .filter(|from| format == "flac" && !from.trim().is_empty()),
            format,
            outside_ids: vec![key],
            isrc: song.isrc.clone().filter(|i| !i.trim().is_empty()),
            title: song.title.clone(),
            artist: song.artist.clone(),
            album: song.album.clone(),
            refs: Vec::new(),
        };
        let bettered = older.filter(|older| better(&placed, older));
        self.record(placed);
        let mut first = None;
        for member in members {
            match self.link_held(path, member).await {
                Ok(link) => {
                    first.get_or_insert(link);
                }
                Err(e) => warn!("FAMILY could not give {member} {path}: {e}"),
            }
        }
        if let Some(older) = bettered {
            self.repoint_held(&older.path, path).await;
        }
        first
    }

    async fn settle(&self, path: &str, members: &[String], wants_shared: bool) -> String {
        if !self.is_pool_path(path) {
            return path.to_string();
        }
        let _changing = self.changes.lock().await;
        if wants_shared {
            return match self.promote_held(path, None).await {
                Ok(moved) => moved,
                Err(e) => {
                    warn!("FAMILY could not move {path} into the shared library: {e}");
                    path.to_string()
                }
            };
        }
        for member in members {
            if let Err(e) = self.link_held(path, member).await {
                warn!("FAMILY could not give {member} {path}: {e}");
            }
        }
        path.to_string()
    }
}

/// Whether `new` is a better copy than `old`: a real FLAC for a lossy file, or for a FLAC made
/// from lossy audio.
fn better(new: &PoolFile, old: &PoolFile) -> bool {
    new.is_real_flac() && !old.is_real_flac()
}

/// Whether `member` sees the shared library `library`. One Octo cannot name is seen only by a
/// member who sees every shared library.
pub fn sees_shared_library(member: &Member, library: Option<i64>) -> bool {
    match &member.shared_libraries {
        SharedLibraries::All => true,
        SharedLibraries::Ids(ids) => library.is_some_and(|id| ids.contains(&id)),
    }
}

/// The record of a pool file Octo knows nothing more about than where it is.
fn unrecorded_file(pool_path: &str, size: u64, refs: Vec<String>) -> PoolFile {
    PoolFile {
        path: pool_path.to_string(),
        size,
        format: extension_of(pool_path),
        outside_ids: Vec::new(),
        isrc: None,
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        refs,
        transcoded_from: None,
    }
}

/// Whether `path` is a link (not following it).
fn is_symlink(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Puts a real copy of what the link at `link` points to in its place: the copy is written
/// beside the link first, then renamed over it, so the member never has neither.
fn replace_link_with_copy(link: &Path) -> io::Result<()> {
    let mut name = link.as_os_str().to_owned();
    name.push(".octo-copy");
    let copy = PathBuf::from(name);
    copy_file(link, &copy)?;
    if let Err(e) = std::fs::rename(&copy, link) {
        let _ = std::fs::remove_file(&copy);
        return Err(e);
    }
    Ok(())
}

/// Copies a song to `to`, written beside it first and then put in its place so a half-written
/// file never stands there, then copies its lyrics files and its folder's cover beside it where
/// nothing is there yet.
fn copy_with_companions(from: &Path, to: &Path) -> io::Result<()> {
    let mut name = to.as_os_str().to_owned();
    name.push(".octo-copy");
    let staged = PathBuf::from(name);
    copy_file(from, &staged)?;
    if let Err(e) = std::fs::rename(&staged, to) {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    for companion in companions(&from.to_string_lossy()) {
        let Some(file_name) = Path::new(&companion).file_name() else {
            continue;
        };
        let destination = to.with_file_name(file_name);
        if destination.exists() {
            continue;
        }
        if let Err(e) = std::fs::copy(&companion, &destination) {
            warn!("FAMILY could not copy {companion} with its song: {e}");
        }
    }
    Ok(())
}

/// The lower-case extension without its dot.
fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// The link text that reaches `<members>/.pool/<relative>` from `<members>/<user>/<relative>`.
pub fn relative_target(relative: &str) -> String {
    let depth = relative.split('/').filter(|part| !part.is_empty()).count();
    format!("{}{POOL_FOLDER}/{relative}", "../".repeat(depth))
}

/// Files that go with a song: its own lyrics files and its folder's cover.
fn companions(song_path: &str) -> Vec<String> {
    let mut found = LibraryActionQuarantine::sidecars_of(song_path);
    if let Some(folder) = Path::new(song_path).parent()
        && let Ok(entries) = std::fs::read_dir(folder)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if COVER_FILES.iter().any(|cover| eq_ignore_case(cover, &name)) {
                found.push(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    found
}

/// Removes cover files (or links) from a folder that holds nothing else.
fn remove_lonely_covers(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let names: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    let only_covers = names.iter().all(|path| {
        path.file_name().is_some_and(|name| {
            COVER_FILES
                .iter()
                .any(|cover| eq_ignore_case(cover, &name.to_string_lossy()))
        })
    });
    if only_covers {
        for path in names {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Removes empty folders from `folder` up to (not including) `stop`.
fn prune_empty(folder: &Path, stop: &Path) {
    let mut current = Some(folder.to_path_buf());
    while let Some(dir) = current {
        if dir == stop || !dir.starts_with(stop) {
            return;
        }
        if std::fs::remove_dir(&dir).is_err() {
            return;
        }
        current = dir.parent().map(Path::to_path_buf);
    }
}

/// Removes every empty folder under `root` (and `root` itself when it ends up empty), and covers
/// left alone in a folder. Files are never touched otherwise.
fn prune_folder_tree(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            prune_folder_tree(&path);
        }
    }
    remove_lonely_covers(root);
    let _ = std::fs::remove_dir(root);
}

/// Creates a folder and its parents, readable and writable by the group (Navidrome often runs
/// as another user in the same group).
fn make_dirs(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o775);
    }
    builder.create(path)
}

/// Moves a file, copying it across disks.
fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        make_dirs(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_file(from, to)?;
    std::fs::remove_file(from)
}

/// Copies a file (through a link at `from`, when it is one), checking the copy's size.
fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        make_dirs(parent)?;
    }
    let length = std::fs::metadata(from)?.len();
    std::fs::copy(from, to)?;
    if std::fs::metadata(to)?.len() != length {
        let _ = std::fs::remove_file(to);
        return Err(io::Error::other("the copy did not match the original's size"));
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &str, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "links need a unix host",
    ))
}

/// Makes a file in `root`, links to it, reads through the link, and removes both.
fn probe(root: &Path) -> bool {
    if make_dirs(root).is_err() {
        return false;
    }
    let name = format!(".octo-link-probe-{}", hex::encode(rand::random::<[u8; 6]>()));
    let file = root.join(&name);
    let link = root.join(format!("{name}.link"));
    let works = std::fs::write(&file, b"octo").is_ok()
        && symlink(&name, &link).is_ok()
        && std::fs::read(&link).is_ok_and(|read| read == b"octo");
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_file(&file);
    works
}

#[cfg(test)]
#[path = "pool_tests.rs"]
mod tests;
