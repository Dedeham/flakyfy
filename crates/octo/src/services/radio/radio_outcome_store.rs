//! `Services/Radio/RadioOutcomeStore.cs`'s file, `radio-outcomes.json`: each listener's radio
//! plays per source, faded by age. Only the counts are saved; what was served lives in memory.

use std::path::Path;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use octo_core::json::datetime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::warn;

use crate::services::state_file;

/// A listener's radio plays from one source, faded by age.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct OutcomeCounts {
    pub kept: f64,
    pub skipped: f64,
    #[serde(with = "datetime::utc")]
    pub updated_utc: DateTime<Utc>,
}

impl Default for OutcomeCounts {
    fn default() -> Self {
        OutcomeCounts {
            kept: 0.0,
            skipped: 0.0,
            updated_utc: datetime::min_value(),
        }
    }
}

/// The file: listener, then provider key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct OutcomeState {
    #[serde(deserialize_with = "state_file::null_as_default")]
    pub listeners: IndexMap<String, IndexMap<String, OutcomeCounts>>,
}

/// The state as the store starts from it. A hand-edited or half-written file can hold nulls;
/// they are dropped, not crashed on: a blank listener, a listener with no sources, a source with
/// no counts. A file that cannot be read starts learning again, with a warning.
pub fn load(path: &Path) -> OutcomeState {
    let read = || -> anyhow::Result<OutcomeState> {
        let Some(text) = state_file::read_text(path)? else {
            return Ok(OutcomeState::default());
        };
        let mut value: Value = serde_json::from_str(&text)?;
        if let Some(Value::Object(listeners)) = value.get_mut("Listeners") {
            listeners.retain(|listener, sources| match sources {
                Value::Object(sources) => {
                    sources.retain(|_, counts| !counts.is_null());
                    !listener.trim().is_empty() && !sources.is_empty()
                }
                _ => false,
            });
        }
        Ok(serde_json::from_value::<Option<OutcomeState>>(value)?.unwrap_or_default())
    };
    read().unwrap_or_else(|e| {
        warn!(
            "{} could not be read; radio starts learning again: {e}",
            path.display()
        );
        OutcomeState::default()
    })
}

/// Writes the whole state through `<file>.tmp`. False (with a warning) when it could not.
pub fn save(path: &Path, state: &OutcomeState) -> bool {
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
        "/../../docs/rust-migration/fixtures/state/radio-outcomes.json"
    );

    #[test]
    fn the_fixture_round_trips_byte_for_byte() {
        let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
        let state: OutcomeState = serde_json::from_str(&text).expect("the fixture reads");
        assert_eq!(state.listeners["brandon"]["lastfm"].kept, 12.75);
        assert_eq!(octo_core::json::to_string(&state), text.trim_end_matches('\n'));
    }

    /// A hand-edited or half-written file can hold nulls; they are dropped, not crashed on.
    #[test]
    fn nulls_in_the_file_are_dropped() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("radio-outcomes.json");
        std::fs::write(
            &path,
            r#"{"Listeners":{"  ":{"lastfm":{"Kept":1}},"bob":null,"ann":{"lastfm":null},"cy":{"lastfm":null,"sonic":{"Kept":2,"Skipped":1,"UpdatedUtc":"2026-10-04T00:00:00Z"}}}}"#,
        )
        .expect("writes");
        let state = load(&path);
        assert_eq!(state.listeners.keys().collect::<Vec<_>>(), ["cy"]);
        assert_eq!(state.listeners["cy"].keys().collect::<Vec<_>>(), ["sonic"]);
        std::fs::write(&path, "[").expect("writes");
        assert_eq!(load(&path), OutcomeState::default());
    }
}
