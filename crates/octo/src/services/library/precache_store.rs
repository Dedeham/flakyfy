//! What the playlist pre-cache was asked to keep ready, and what it fetched: `precache.json`.
//!
//! Changes are rare (a pick edited, a song landed, a song let go), so every one is written at once
//! rather than on a timer.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use octo_core::json::datetime;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::services::state_file;

/// How long a song that was let go is not fetched again while its list still holds it.
pub const LET_GO_MEMORY_DAYS: i64 = 90;

/// A list the admin asked to keep ready: a listener's Made for you list or a chart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Pick {
    /// The listener, in lower case.
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub owner: String,
    /// The list's key, such as `foryou:newreleases` or a chart's id.
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub key: String,
    /// How many songs of it to keep ready; None follows `PrecacheCount`.
    #[serde(default)]
    pub count: Option<i32>,
}

/// A song the pre-cache fetched and has not yet let go of or handed over to the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Cached {
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub owner: String,
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub list: String,
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub provider: String,
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub external_id: String,
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub artist: String,
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub title: String,
    /// Where the file landed.
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub path: String,
    #[serde(default = "datetime::min_value", with = "datetime::utc")]
    pub cached_utc: DateTime<Utc>,
    /// Why it is still here when its time is up, such as "dry run: it would go".
    #[serde(default)]
    pub note: Option<String>,
}

impl Cached {
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider, self.external_id)
    }
}

/// A song let go, so its list holding it still does not fetch it again at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct LetGo {
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub key: String,
    #[serde(default = "datetime::min_value", with = "datetime::utc")]
    pub at_utc: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PrecacheState {
    #[serde(default)]
    pub picks: Vec<Pick>,
    #[serde(default)]
    pub cached: Vec<Cached>,
    #[serde(default)]
    pub let_go: Vec<LetGo>,
    #[serde(default, with = "datetime::utc_option")]
    pub last_fill_utc: Option<DateTime<Utc>>,
    #[serde(default, with = "datetime::utc_option")]
    pub last_expiry_utc: Option<DateTime<Utc>>,
}

impl PrecacheState {
    /// Whether `key` (`provider:id`) was let go recently enough to be left alone.
    pub fn recently_let_go(&self, key: &str, now: DateTime<Utc>) -> bool {
        self.let_go
            .iter()
            .any(|gone| gone.key == key && now - gone.at_utc < TimeDelta::days(LET_GO_MEMORY_DAYS))
    }
}

pub struct PrecacheStore {
    path: Option<PathBuf>,
    state: Mutex<PrecacheState>,
}

impl PrecacheStore {
    /// A store kept in `path` (None keeps it in memory). A file that cannot be read is set aside
    /// as `<file>.corrupt` and the pre-cache starts over: nothing here is worth stopping for.
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut state = PrecacheState::default();
        if let Some(path) = &path
            && path.exists()
        {
            match Self::load(path) {
                Ok(loaded) => state = loaded,
                Err(e) => {
                    warn!("the pre-cache state could not be read ({e}); starting over");
                    let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
                }
            }
        }
        PrecacheStore {
            path,
            state: Mutex::new(state),
        }
    }

    fn load(path: &Path) -> anyhow::Result<PrecacheState> {
        let text = state_file::read_all_text(path)?;
        Ok(serde_json::from_str::<Option<PrecacheState>>(&text)?.unwrap_or_default())
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn read<T>(&self, read: impl FnOnce(&PrecacheState) -> T) -> T {
        read(&self.state.lock())
    }

    /// Changes the state and writes it.
    pub fn update<T>(&self, change: impl FnOnce(&mut PrecacheState) -> T) -> T {
        let mut state = self.state.lock();
        let result = change(&mut state);
        if let Some(path) = &self.path
            && let Err(e) = state_file::save_atomic(path, &octo_core::json::to_string(&*state))
        {
            warn!("the pre-cache state could not be written: {e}");
        }
        result
    }
}

#[cfg(test)]
#[path = "precache_store_tests.rs"]
mod tests;
