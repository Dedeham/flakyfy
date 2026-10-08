//! `Services/Imports`: importing songs from Spotify, a public link or a file. What is here so far
//! is what the import keeps on disk (`imports.json`, `imports-trickle.json`,
//! `spotify-accounts.json`), read and written as the C# stores did; the import itself, the
//! Spotify sign-in and the trickle are group H's.

pub mod import_models;
pub mod import_store;
pub mod spotify_account_store;
pub mod trickle_queue;

pub use import_models::{ImportList, ImportTrack, TrickleJob};
pub use spotify_account_store::SpotifyAccount;

use std::path::Path;

use serde::de::DeserializeOwned;
use tracing::warn;

use super::state_file;

/// How the C# import stores read their file at start: missing is empty; one that cannot be read
/// is a warning ("the imported lists could not be read: ...") and empty too.
pub(crate) fn load_or_default<T: DeserializeOwned + Default>(path: &Path, what: &str) -> T {
    let loaded = (|| -> anyhow::Result<Option<T>> {
        let Some(text) = state_file::read_text(path)? else {
            return Ok(None);
        };
        Ok(serde_json::from_str::<Option<T>>(&text)?)
    })();
    match loaded {
        Ok(value) => value.unwrap_or_default(),
        Err(e) => {
            warn!("{what} could not be read: {e}");
            T::default()
        }
    }
}

/// How they wrote it: the whole file through `<file>.tmp`, compact, with a warning when it
/// fails. False when it could not be written.
pub(crate) fn save<T: serde::Serialize>(path: &Path, value: &T, what: &str) -> bool {
    match state_file::save_atomic(path, &octo_core::json::to_string(value)) {
        Ok(()) => true,
        Err(e) => {
            warn!("{what} could not be written: {e}");
            false
        }
    }
}
