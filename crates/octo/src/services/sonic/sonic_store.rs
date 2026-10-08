//! `Services/Sonic/SonicStore.cs`'s file, `sonic-features.json`: what every analyzed song sounds
//! like, by Navidrome song id.

use std::path::Path;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use octo_core::json::datetime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::services::state_file::{self, null_as_default};

/// One analyzed song.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SonicSong {
    /// What the song's file was when it was read, so a changed file is read again.
    #[serde(deserialize_with = "null_as_default")]
    pub stamp: String,
    #[serde(deserialize_with = "null_as_default")]
    pub title: String,
    #[serde(deserialize_with = "null_as_default")]
    pub artist: String,
    pub album: Option<String>,
    pub duration: Option<i32>,
    /// The analysis's features version (bliss-audio's `FeaturesVersion`).
    pub version: i32,
    /// The features: single-precision numbers, written as `float` was.
    #[serde(rename = "F", deserialize_with = "null_as_default")]
    pub f: Vec<f32>,
}

/// A song the analysis could not read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SonicFailure {
    #[serde(deserialize_with = "null_as_default")]
    pub stamp: String,
    #[serde(deserialize_with = "null_as_default")]
    pub error: String,
    #[serde(with = "datetime::utc")]
    pub at_utc: DateTime<Utc>,
}

impl Default for SonicFailure {
    fn default() -> Self {
        SonicFailure {
            stamp: String::new(),
            error: String::new(),
            at_utc: datetime::min_value(),
        }
    }
}

/// The file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SonicState {
    pub paused: bool,
    pub pass: i32,
    #[serde(with = "datetime::utc_option")]
    pub pass_finished_utc: Option<DateTime<Utc>>,
    #[serde(with = "datetime::utc_option")]
    pub next_pass_utc: Option<DateTime<Utc>>,
    /// By Navidrome song id.
    #[serde(deserialize_with = "null_as_default")]
    pub songs: IndexMap<String, SonicSong>,
    #[serde(deserialize_with = "null_as_default")]
    pub failed: IndexMap<String, SonicFailure>,
}

/// The state as the store starts from it. A hand-edited or half-written file can hold nulls; a
/// song with no features or no stamp, or a failure with no stamp, is dropped, not crashed on. A
/// file that cannot be read starts over, with a warning.
pub fn load(path: &Path) -> SonicState {
    let read = || -> anyhow::Result<SonicState> {
        let Some(text) = state_file::read_text(path)? else {
            return Ok(SonicState::default());
        };
        let mut value: Value = serde_json::from_str(&text)?;
        let has = |entry: &Value, name: &str| entry.get(name).is_some_and(|v| !v.is_null());
        if let Some(Value::Object(songs)) = value.get_mut("Songs") {
            songs.retain(|_, song| has(song, "F") && has(song, "Stamp"));
        }
        if let Some(Value::Object(failed)) = value.get_mut("Failed") {
            failed.retain(|_, failure| has(failure, "Stamp"));
        }
        Ok(serde_json::from_value::<Option<SonicState>>(value)?.unwrap_or_default())
    };
    read().unwrap_or_else(|e| {
        warn!("{} could not be read; starting over: {e}", path.display());
        SonicState::default()
    })
}

/// Writes the whole state through `<file>.tmp`. False (with a warning) when it could not.
pub fn save(path: &Path, state: &SonicState) -> bool {
    match state_file::save_atomic(path, &octo_core::json::to_string(state)) {
        Ok(()) => true,
        Err(e) => {
            warn!("Could not save {}: {e}", path.display());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/rust-migration/fixtures/state/sonic-features.json"
    );

    /// Single-precision features are written as .NET wrote a `float`: shortest round trip,
    /// exponents as `E-05` and `E+38`.
    #[test]
    fn the_fixture_round_trips_byte_for_byte() {
        let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
        let state: SonicState = serde_json::from_str(&text).expect("the fixture reads");
        assert_eq!(state.songs["nd-8f7e6d5c"].f.len(), 10);
        assert_eq!(octo_core::json::to_string(&state), text.trim_end_matches('\n'));
    }

    #[test]
    fn a_song_without_features_or_stamp_is_dropped_on_load() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("sonic-features.json");
        std::fs::write(
            &path,
            r#"{"Pass":1,"Songs":{"a":{"Stamp":"1","F":null},"b":{"Stamp":null,"F":[1]},"c":{"Stamp":"3","F":[0.5]},"d":null},"Failed":{"x":{"Stamp":null},"y":{"Stamp":"9","Error":"no"}}}"#,
        )
        .expect("writes");
        let state = load(&path);
        assert_eq!(state.songs.keys().collect::<Vec<_>>(), ["c"]);
        assert_eq!(state.failed.keys().collect::<Vec<_>>(), ["y"]);
        assert_eq!(state.pass, 1);
    }
}
