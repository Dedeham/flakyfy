//! `Services/Imports/SpotifyAccountStore.cs`'s file, `spotify-accounts.json`: Spotify sign-ins by
//! Navidrome user, in their own file beside settings.json rather than in it (the access token
//! changes every hour, settings.json is watched and shown on the dashboard, and these must never
//! be). Written whole each time, through a temporary file readable by its owner alone.

use std::path::Path;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use octo_core::json::datetime;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::services::state_file::{self, null_as_default};

/// One person's Spotify sign-in. The tokens never leave Octo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SpotifyAccount {
    #[serde(deserialize_with = "null_as_default")]
    pub access_token: String,
    #[serde(deserialize_with = "null_as_default")]
    pub refresh_token: String,
    #[serde(with = "datetime::utc")]
    pub expires_utc: DateTime<Utc>,
    /// When they allowed Octo. Spotify ends a sign-in six months after this, however often it is
    /// renewed.
    #[serde(with = "datetime::utc")]
    pub connected_utc: DateTime<Utc>,
    pub spotify_id: Option<String>,
    pub display_name: Option<String>,
    /// Why the sign-in no longer works, in words, or None while it does.
    pub problem: Option<String>,
}

impl Default for SpotifyAccount {
    fn default() -> Self {
        SpotifyAccount {
            access_token: String::new(),
            refresh_token: String::new(),
            expires_utc: datetime::min_value(),
            connected_utc: datetime::min_value(),
            spotify_id: None,
            display_name: None,
            problem: None,
        }
    }
}

/// The file: each user's account, by name (the C# dictionary compared names ignoring case).
pub type SpotifyAccounts = IndexMap<String, SpotifyAccount>;

pub fn load(path: &Path) -> SpotifyAccounts {
    super::load_or_default(path, "the Spotify sign-ins")
}

pub fn save(path: &Path, accounts: &SpotifyAccounts) -> bool {
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let temp = state_file::tmp_path(path);
        std::fs::write(&temp, octo_core::json::to_string(accounts))?;
        // Only Octo reads it. Where the platform lets us say so, nobody else may.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temp, path)
    };
    match write() {
        Ok(()) => true,
        Err(e) => {
            warn!("the Spotify sign-ins could not be written: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/rust-migration/fixtures/state/spotify-accounts.json"
    );

    #[test]
    fn the_fixture_round_trips_byte_for_byte() {
        let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
        let accounts: SpotifyAccounts = serde_json::from_str(&text).expect("the fixture reads");
        assert_eq!(accounts.len(), 2);
        assert_eq!(octo_core::json::to_string(&accounts), text.trim_end_matches('\n'));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_by_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("spotify-accounts.json");
        let accounts: SpotifyAccounts =
            serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("the fixture")).expect("reads");
        assert!(save(&path, &accounts));
        let mode = std::fs::metadata(&path).expect("exists").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(load(&path), accounts);
    }
}
