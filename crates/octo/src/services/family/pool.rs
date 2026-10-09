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

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::common::{PathHelper, SongIdentity};
use octo_core::models::domain::Song;
use octo_core::settings::{LibraryAction, SettingsStore};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::family_playlist_store::FamilyPlaylistStore;
use super::family_pool_store::{FamilyPoolStore, PoolFile};
use super::family_store::FamilyStore;
use super::library_index::{IndexedSong, LibraryIndex};
use super::member_credentials::MemberCredentials;
use crate::services::common::acquisition_tracker::key_of;
use crate::services::library::{LibraryActionQuarantine, PathSource, ResolvedSongFile};
use crate::services::local::ILocalLibraryService;

/// The pool's folder under the members folder. The dot keeps it out of every member's
/// library and out of Navidrome's scans.
pub const POOL_FOLDER: &str = ".pool";

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
    fn of(mode: LinkMode) -> Self {
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
    /// Navidrome has shown a linked song in a member's library at least once.
    links_seen: AtomicBool,
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
            mode: RwLock::new(LinkMode::Symlink),
            links_seen: AtomicBool::new(false),
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
    /// removes both. Links when that works, copies when it does not.
    pub fn probe_links(&self) -> LinkStatus {
        let mode = if probe(Path::new(&self.members_root())) {
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
        self.mode() == LinkMode::Copy || self.links_seen.load(Ordering::SeqCst)
    }

    /// Navidrome showed a linked song in a member's library.
    pub fn confirm_links_seen(&self) {
        self.links_seen.store(true, Ordering::SeqCst);
    }

    /// Navidrome never showed a linked song: every member gets real copies from now on, and
    /// every link made so far is replaced by a copy. Answers how many links were replaced.
    pub fn fall_back_to_copies(&self) -> usize {
        *self.mode.write() = LinkMode::Copy;
        warn!("FAMILY Navidrome did not show a linked song; members get copies from now on");
        let mut replaced = 0;
        for file in self.store.list() {
            for user in &file.refs {
                let Some(link) = self.link_path(&file.path, user) else {
                    continue;
                };
                let is_link = std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink());
                if is_link && std::fs::remove_file(&link).is_ok() && std::fs::copy(&file.path, &link).is_ok()
                {
                    replaced += 1;
                }
            }
        }
        replaced
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

    /// Puts a pool file in `user`'s library: a relative link (or a copy in copy mode), plus the
    /// song's lyrics files and the folder's cover. Records the member's reference, tells the
    /// index the file is there, and starts a scan of their library. Doing it twice changes
    /// nothing. Answers the link's path.
    pub async fn link(&self, pool_path: &str, user: &str) -> Result<String, PoolError> {
        let member = self
            .family
            .member(user)
            .ok_or_else(|| PoolError::NotMember(user.to_string()))?;
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
            self.store.upsert(PoolFile {
                path: pool_path.to_string(),
                size,
                format: extension_of(pool_path),
                outside_ids: Vec::new(),
                isrc: None,
                title: String::new(),
                artist: String::new(),
                album: String::new(),
                refs: Vec::new(),
            });
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
        let member = self
            .family
            .member(user)
            .ok_or_else(|| PoolError::NotMember(user.to_string()))?;
        self.remove_link(pool_path, &member.username, &companions(pool_path));
        let left = self.store.remove_ref(pool_path, &member.username);
        if left.is_none_or(|left| left == 0) {
            self.gc(pool_path).await;
        }
        if let Some(library_id) = member.personal_library_id {
            self.library.trigger_library_scan_of(library_id).await;
        }
        Ok(())
    }

    /// Sends a pool file nobody links any more to the trash (kept as long as library actions
    /// keep removed songs), with its lyrics files. True when it went.
    pub async fn gc(&self, pool_path: &str) -> bool {
        if self
            .store
            .get(pool_path)
            .is_some_and(|file| !file.refs.is_empty())
        {
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

    /// A better copy of a song replaces `old_path` for everyone who has it: each member's link
    /// moves to `new_path`, the old file goes to the trash, and their stars and family playlist
    /// entries follow the song once the index finds it again. Answers the members moved.
    pub async fn repoint(&self, old_path: &str, new_path: &str) -> Vec<String> {
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
            match self.link(new_path, user).await {
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
        self.gc(old_path).await;
        info!(
            "FAMILY {} moved from {old_path} to the better copy {new_path}",
            moved.join(", ")
        );
        self.carry_over(remaps);
        moved
    }

    /// The shared library wants a pool song: the file moves into the shared library's layout,
    /// every member's link goes (they see it there now), and their stars and family playlist
    /// entries follow the song. Still one file. Answers its new path.
    pub async fn promote(&self, pool_path: &str) -> Result<String, PoolError> {
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
        let mut remaps = Vec::new();
        for user in &refs {
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

        let companions = companions(pool_path);
        move_file(Path::new(pool_path), Path::new(&target))?;
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
            let result = if is_cover {
                std::fs::copy(companion, &destination).map(|_| ())
            } else {
                move_file(Path::new(companion), &destination)
            };
            if let Err(e) = result {
                warn!("FAMILY could not move {companion} with its song: {e}");
            }
        }
        for user in &refs {
            self.remove_link(pool_path, user, &companions);
        }
        self.store.remove(pool_path);
        if let Some(folder) = Path::new(pool_path).parent() {
            remove_lonely_covers(folder);
            prune_empty(folder, Path::new(&self.pool_root_path()));
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
        for user in &refs {
            if let Some(library_id) = self.family.member(user).and_then(|m| m.personal_library_id) {
                self.library.trigger_library_scan_of(library_id).await;
            }
        }
        info!("FAMILY {pool_path} moved into the shared library as {target}");
        self.carry_over(remaps);
        Ok(target)
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
        let key = key_of(provider, external_id);
        let older = self
            .find(Some(&key), song.isrc.as_deref(), &song.artist, &song.title)
            .filter(|file| file.path != path);
        self.record(PoolFile {
            path: path.to_string(),
            size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            format: extension_of(path),
            outside_ids: vec![key],
            isrc: song.isrc.clone().filter(|i| !i.trim().is_empty()),
            title: song.title.clone(),
            artist: song.artist.clone(),
            album: song.album.clone(),
            refs: Vec::new(),
        });
        let mut first = None;
        for member in members {
            match self.link(path, member).await {
                Ok(link) => {
                    first.get_or_insert(link);
                }
                Err(e) => warn!("FAMILY could not give {member} {path}: {e}"),
            }
        }
        if let Some(older) = older
            && better(path, &older.path)
        {
            self.repoint(&older.path, path).await;
        }
        first
    }

    async fn settle(&self, path: &str, members: &[String], wants_shared: bool) -> String {
        if !self.is_pool_path(path) {
            return path.to_string();
        }
        if wants_shared {
            return match self.promote(path).await {
                Ok(moved) => moved,
                Err(e) => {
                    warn!("FAMILY could not move {path} into the shared library: {e}");
                    path.to_string()
                }
            };
        }
        for member in members {
            if let Err(e) = self.link(path, member).await {
                warn!("FAMILY could not give {member} {path}: {e}");
            }
        }
        path.to_string()
    }
}

/// Whether the file at `new` is a better copy than the one at `old`: a FLAC for a lossy file.
fn better(new: &str, old: &str) -> bool {
    extension_of(new) == "flac" && extension_of(old) != "flac"
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
    let length = std::fs::metadata(from)?.len();
    std::fs::copy(from, to)?;
    if std::fs::metadata(to)?.len() != length {
        let _ = std::fs::remove_file(to);
        return Err(io::Error::other("the copy did not match the original's size"));
    }
    std::fs::remove_file(from)
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
