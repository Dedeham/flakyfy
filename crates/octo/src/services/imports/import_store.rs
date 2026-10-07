//! `Services/Imports/ImportStore.cs`'s file, `imports.json`: every imported list, with its songs
//! and what the last match found, so a restart keeps them and their playlist and fetch switches.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::ImportList;
use crate::services::state_file::null_as_default;

/// `ImportStore.FileShape`: `{"Lists": [...]}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ImportFile {
    #[serde(deserialize_with = "null_as_default")]
    pub lists: Vec<ImportList>,
}

pub fn load(path: &Path) -> ImportFile {
    super::load_or_default(path, "the imported lists")
}

pub fn save(path: &Path, file: &ImportFile) -> bool {
    super::save(path, file, "the imported lists")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/rust-migration/fixtures/state/imports.json"
    );

    #[test]
    fn the_fixture_round_trips_byte_for_byte() {
        let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
        let file: ImportFile = serde_json::from_str(&text).expect("the fixture reads");
        assert_eq!(file.lists.len(), 2);
        assert_eq!(file.lists[0].tracks[0].library_id.as_deref(), Some("nd-8f7e6d5c"));
        assert_eq!(octo_core::json::to_string(&file), text.trim_end_matches('\n'));
    }

    #[test]
    fn load_and_save_go_through_the_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config").join("imports.json");
        assert_eq!(load(&path), ImportFile::default(), "no file is no lists");
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("creates");
        std::fs::write(&path, "{ not json").expect("writes");
        assert_eq!(
            load(&path),
            ImportFile::default(),
            "an unreadable file is no lists"
        );
        let file: ImportFile =
            serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("the fixture")).expect("reads");
        assert!(save(&path, &file));
        assert_eq!(load(&path), file);
    }
}
