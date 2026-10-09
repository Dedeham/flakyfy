//! `family-pool.json`: the one real copy of every song a member downloaded, under
//! `<members>/.pool/`, and which members link to it from their own library. This file is the
//! record of who links what; Octo never counts links by walking folders.

use std::path::{Path, PathBuf};

use octo_core::common::dotnet::eq_ignore_case;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::store_file;

/// One file in the pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PoolFile {
    /// The file's path inside the container (`/members/.pool/<Artist>/<Album>/<file>`).
    pub path: String,
    /// Bytes.
    #[serde(default)]
    pub size: u64,
    /// The file's format (`flac`, `mp3`, ...).
    #[serde(default)]
    pub format: String,
    /// The outside ids this file is a copy of.
    #[serde(default)]
    pub outside_ids: Vec<String>,
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    /// The members whose library links to this file.
    #[serde(default)]
    pub refs: Vec<String>,
}

/// The store behind `family-pool.json`.
pub struct FamilyPoolStore {
    files: Mutex<Vec<PoolFile>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
    load_warning: Option<String>,
}

impl FamilyPoolStore {
    pub fn in_memory() -> Self {
        Self::new(None::<PathBuf>)
    }

    pub fn new(path: Option<impl AsRef<Path>>) -> Self {
        let path = store_file::clean_path(path);
        let (files, load_warning) = store_file::load::<Vec<PoolFile>>(path.as_deref());
        FamilyPoolStore {
            files: Mutex::new(files),
            path,
            save_lock: Mutex::new(()),
            load_warning,
        }
    }

    pub fn load_warning(&self) -> Option<&str> {
        self.load_warning.as_deref()
    }

    pub fn get(&self, path: &str) -> Option<PoolFile> {
        self.files.lock().iter().find(|f| f.path == path).cloned()
    }

    pub fn list(&self) -> Vec<PoolFile> {
        self.files.lock().clone()
    }

    /// The pool file that is a copy of this outside id.
    pub fn find_by_outside_id(&self, outside_id: &str) -> Option<PoolFile> {
        self.files
            .lock()
            .iter()
            .find(|f| f.outside_ids.iter().any(|id| id == outside_id))
            .cloned()
    }

    /// The pool file with this ISRC (ignoring case).
    pub fn find_by_isrc(&self, isrc: &str) -> Option<PoolFile> {
        let isrc = isrc.trim();
        if isrc.is_empty() {
            return None;
        }
        self.files
            .lock()
            .iter()
            .find(|f| f.isrc.as_deref().is_some_and(|i| i.eq_ignore_ascii_case(isrc)))
            .cloned()
    }

    /// Adds a file, or replaces the record with the same path (keeping its refs when the new
    /// record has none).
    pub fn upsert(&self, mut file: PoolFile) {
        {
            let mut files = self.files.lock();
            match files.iter_mut().find(|f| f.path == file.path) {
                Some(existing) => {
                    if file.refs.is_empty() {
                        file.refs = std::mem::take(&mut existing.refs);
                    }
                    *existing = file;
                }
                None => files.push(file),
            }
        }
        self.save();
    }

    /// Records that `user` links this file. False when there is no such file or they already
    /// did.
    pub fn add_ref(&self, path: &str, user: &str) -> bool {
        let added = {
            let mut files = self.files.lock();
            match files.iter_mut().find(|f| f.path == path) {
                Some(file) if !file.refs.iter().any(|r| eq_ignore_case(r, user)) => {
                    file.refs.push(user.to_string());
                    true
                }
                _ => false,
            }
        };
        if added {
            self.save();
        }
        added
    }

    /// Records that `user` no longer links this file and answers how many members still do;
    /// None when there is no such file. A file nobody links stays recorded until
    /// [`FamilyPoolStore::remove`] (the caller trashes it first).
    pub fn remove_ref(&self, path: &str, user: &str) -> Option<usize> {
        let left = {
            let mut files = self.files.lock();
            let file = files.iter_mut().find(|f| f.path == path)?;
            file.refs.retain(|r| !eq_ignore_case(r, user));
            file.refs.len()
        };
        self.save();
        Some(left)
    }

    /// Forgets a file. False when it was not recorded.
    pub fn remove(&self, path: &str) -> bool {
        let removed = {
            let mut files = self.files.lock();
            let before = files.len();
            files.retain(|f| f.path != path);
            before != files.len()
        };
        if removed {
            self.save();
        }
        removed
    }

    /// The files `user` links.
    pub fn refs_of(&self, user: &str) -> Vec<PoolFile> {
        self.files
            .lock()
            .iter()
            .filter(|f| f.refs.iter().any(|r| eq_ignore_case(r, user)))
            .cloned()
            .collect()
    }

    /// The bytes of every file `user` links, each at full size.
    pub fn bytes_linked_by(&self, user: &str) -> u64 {
        self.refs_of(user).iter().map(|f| f.size).sum()
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let _saving = self.save_lock.lock();
        let files = self.files.lock().clone();
        store_file::save(Some(path), &files);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, refs: &[&str]) -> PoolFile {
        PoolFile {
            path: path.to_string(),
            size: 30_000_000,
            format: "flac".into(),
            outside_ids: vec!["ext-deezer-song-1".into()],
            isrc: Some("USUM71703861".into()),
            title: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            refs: refs.iter().map(|r| r.to_string()).collect(),
        }
    }

    const POOL: &str = "/members/.pool/Artist/Album/01 Song.flac";

    #[test]
    fn lookups_by_outside_id_and_isrc() {
        let store = FamilyPoolStore::in_memory();
        store.upsert(file(POOL, &["sam"]));
        assert_eq!(
            store.find_by_outside_id("ext-deezer-song-1").expect("found").path,
            POOL
        );
        assert!(store.find_by_outside_id("ext-deezer-song-2").is_none());
        assert_eq!(store.find_by_isrc(" usum71703861 ").expect("found").path, POOL);
        assert!(store.find_by_isrc("").is_none());
    }

    #[test]
    fn refs_count_each_member_once() {
        let store = FamilyPoolStore::in_memory();
        store.upsert(file(POOL, &["sam"]));
        assert!(store.add_ref(POOL, "alex"));
        assert!(!store.add_ref(POOL, "ALEX"));
        assert!(!store.add_ref("/members/.pool/none.flac", "alex"));
        assert_eq!(store.bytes_linked_by("alex"), 30_000_000);
        assert_eq!(store.remove_ref(POOL, "sam"), Some(1));
        assert_eq!(store.remove_ref(POOL, "alex"), Some(0));
        assert_eq!(store.remove_ref("/nowhere", "alex"), None);
        assert!(store.get(POOL).is_some(), "kept until the caller removes it");
        assert!(store.remove(POOL));
        assert!(!store.remove(POOL));
    }

    #[test]
    fn upsert_keeps_refs_and_survives_a_restart() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("family-pool.json");
        let store = FamilyPoolStore::new(Some(&path));
        store.upsert(file(POOL, &["sam", "alex"]));
        let mut better = file(POOL, &[]);
        better.size = 40_000_000;
        store.upsert(better);
        let reloaded = FamilyPoolStore::new(Some(&path));
        let got = reloaded.get(POOL).expect("there");
        assert_eq!(got.size, 40_000_000);
        assert_eq!(got.refs, ["sam", "alex"]);
        assert_eq!(reloaded.refs_of("sam").len(), 1);
        assert!(reloaded.refs_of("kim").is_empty());
    }
}
