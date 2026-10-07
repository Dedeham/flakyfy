//! `Services/Common/DownloadPicks.cs`: the copy someone picked in Find songs. Only the
//! [`PickedCopy`] a waiting upgrade job carries in `upgrades.json` is here; Find songs and the
//! picks themselves are group D's.

use serde::{Deserialize, Serialize};

use crate::services::state_file;

/// Which source a picked copy is from: Soulseek's peer file, or Lidarr's release.
pub const SOULSEEK_SOURCE: &str = "Soulseek";

/// One copy someone picked in Find songs: a Soulseek peer's file, or a Lidarr release. Kept as
/// plain values, so an upgrade job waiting for its turn can carry it on disk. The C# class's
/// derived `IsSoulseek` and `Release` were `[JsonIgnore]`d, so the file holds these fields only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct PickedCopy {
    /// "Soulseek" or "Lidarr".
    #[serde(deserialize_with = "state_file::null_as_default")]
    pub source: String,

    // A Soulseek file, as the search found it.
    pub peer: Option<String>,
    pub file: Option<String>,
    pub size: i64,
    pub format: Option<String>,
    pub bit_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub sample_rate: Option<i32>,
    pub length: Option<i32>,

    // A Lidarr release, and its size in Size. Without one, Lidarr chooses by its quality profile.
    pub release_guid: Option<String>,
    pub indexer_id: Option<i32>,
    pub release_title: Option<String>,
}

impl Default for PickedCopy {
    fn default() -> Self {
        PickedCopy {
            source: SOULSEEK_SOURCE.to_string(),
            peer: None,
            file: None,
            size: 0,
            format: None,
            bit_rate: None,
            bit_depth: None,
            sample_rate: None,
            length: None,
            release_guid: None,
            indexer_id: None,
            release_title: None,
        }
    }
}

impl PickedCopy {
    /// A Soulseek file rather than a Lidarr release.
    pub fn is_soulseek(&self) -> bool {
        octo_core::common::dotnet::eq_ignore_case(&self.source, SOULSEEK_SOURCE)
    }

    /// Whether this is the same copy: the same peer's same file, or the same release.
    pub fn same_as(&self, other: Option<&PickedCopy>) -> bool {
        other.is_some_and(|other| {
            octo_core::common::dotnet::eq_ignore_case(&self.source, &other.source)
                && self.peer == other.peer
                && self.file == other.file
                && self.release_guid == other.release_guid
                && self.indexer_id == other.indexer_id
        })
    }
}
