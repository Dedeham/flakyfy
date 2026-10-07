//! What `Services/Library/LibraryTagEdits.cs` keeps on disk, `tag-edits.json`: the tag edits
//! made from the apps (and by the explicit marking), newest last, so each can be undone and a
//! person can see later which files were changed, by whom and how. The edits themselves are
//! group F's; the explicit marking's are group P's.

use std::path::Path;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use octo_core::json::datetime;
use octo_media::tags::kept_identity::KeptIdentity;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::services::state_file::{self, null_as_default};

/// The kinds of edit, as the journal names them.
pub mod tag_edit_kinds {
    pub const RETAG: &str = "retag";
    pub const JOIN_ALBUM: &str = "joinAlbum";
    pub const COVER: &str = "cover";
    /// The explicit advisory, written by the library-wide explicit marking.
    pub const ADVISORY: &str = "advisory";
}

/// Room for a library-wide run's edits beside the apps' own: the oldest go past this many.
pub const MAX_ENTRIES: usize = 20_000;

/// One edit the apps made to one file, kept so it can be undone and so a person can see later
/// which files were changed, by whom and how. The record's fields in order, then its body
/// properties, as System.Text.Json wrote them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TagEditEntry {
    #[serde(deserialize_with = "null_as_default")]
    pub id: String,
    #[serde(deserialize_with = "null_as_default")]
    pub navidrome_id: String,
    #[serde(deserialize_with = "null_as_default")]
    pub path: String,
    #[serde(deserialize_with = "null_as_default")]
    pub kind: String,
    #[serde(deserialize_with = "null_as_default")]
    pub username: String,
    /// The fields as they were, by tag name; None for a field the file did not have.
    #[serde(deserialize_with = "null_as_default")]
    pub before: IndexMap<String, Option<String>>,
    #[serde(deserialize_with = "null_as_default")]
    pub after: IndexMap<String, Option<String>>,
    /// The file as the edit left it, so an undo leaves alone a file changed since.
    pub size_after: i64,
    pub write_ticks_after: i64,
    #[serde(with = "datetime::utc")]
    pub at_utc: DateTime<Utc>,
    /// For an album join: the album as it was, so the undo can put it back exactly.
    pub album_before: Option<KeptIdentity>,
    pub undone: bool,
    /// The library-wide run that made this edit, when one did (the explicit marking), so the
    /// whole run can be undone at once.
    pub run_id: Option<String>,
}

impl Default for TagEditEntry {
    fn default() -> Self {
        TagEditEntry {
            id: String::new(),
            navidrome_id: String::new(),
            path: String::new(),
            kind: String::new(),
            username: String::new(),
            before: IndexMap::new(),
            after: IndexMap::new(),
            size_after: 0,
            write_ticks_after: 0,
            at_utc: datetime::min_value(),
            album_before: None,
            undone: false,
            run_id: None,
        }
    }
}

/// The journal as `TagEditJournal` starts from it: empty when there is no file, and empty with a
/// warning when it cannot be read.
pub fn load(path: &Path) -> Vec<TagEditEntry> {
    let read = || -> anyhow::Result<Vec<TagEditEntry>> {
        let Some(text) = state_file::read_text(path)? else {
            return Ok(Vec::new());
        };
        Ok(serde_json::from_str::<Option<Vec<TagEditEntry>>>(&text)?.unwrap_or_default())
    };
    read().unwrap_or_else(|e| {
        warn!("tag edit journal could not be read: {e}");
        Vec::new()
    })
}

/// Rewrites the whole journal through `<file>.tmp`. False (with a warning) when it could not be
/// kept on disk.
pub fn save(path: &Path, entries: &[TagEditEntry]) -> bool {
    match state_file::save_atomic(path, &octo_core::json::to_string(entries)) {
        Ok(()) => true,
        Err(e) => {
            warn!("tag edit journal could not be written: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/rust-migration/fixtures/state/tag-edits.json"
    );

    #[test]
    fn the_fixture_round_trips_byte_for_byte() {
        let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
        let entries: Vec<TagEditEntry> = serde_json::from_str(&text).expect("the fixture reads");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].before.get("isrc"), Some(&None));
        let album = entries[1]
            .album_before
            .as_ref()
            .expect("an album join keeps the album");
        assert_eq!(album.album.as_deref(), Some("Jazz (Deluxe)"));
        assert_eq!(album.track, 12);
        assert_eq!(entries[2].run_id.as_deref(), Some("run-explicit-1"));
        assert_eq!(octo_core::json::to_string(&entries), text.trim_end_matches('\n'));
    }

    #[test]
    fn load_and_save_go_through_the_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("tag-edits.json");
        assert!(load(&path).is_empty());
        let entries: Vec<TagEditEntry> =
            serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("the fixture")).expect("reads");
        assert!(save(&path, &entries));
        assert_eq!(load(&path), entries);
        std::fs::write(&path, "not json").expect("writes");
        assert!(load(&path).is_empty());
    }
}
