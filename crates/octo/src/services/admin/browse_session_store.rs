//! Port of `Services/Admin/BrowseSessionStore.cs`, the store behind `browse-sessions.json`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use indexmap::IndexMap;
use octo_core::common::Clock;
use octo_core::json::datetime;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::services::state_file;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Session {
    user: String,
    expires: DateTime<Utc>,
    saved_expires: DateTime<Utc>,
    /// How far each use slides the expiry: 90 days, or an hour for the recovery code.
    lifetime: TimeDelta,
    /// Written to disk. A recovery-code session never is.
    persist: bool,
}

/// One line of `browse-sessions.json` (the private record `Saved(string Hash, string User, DateTime Expires)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Saved {
    #[serde(deserialize_with = "state_file::null_as_default")]
    hash: String,
    #[serde(deserialize_with = "state_file::null_as_default")]
    user: String,
    #[serde(with = "datetime::utc")]
    expires: DateTime<Utc>,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            hash: String::new(),
            user: String::new(),
            expires: datetime::min_value(),
        }
    }
}

/// Tokens proving the holder authenticated as a Navidrome admin.
///
/// The dashboard and the admin API sign in with the Navidrome that Octo already fronts rather
/// than a login system of their own, and hand back one of these. A session made with the
/// recovery code is one too, but it names [`BrowseSessionStore::RECOVERY_USER`], lasts an hour,
/// is never written to disk, and the endpoints that touch library files do not accept it
/// ([`BrowseSessionStore::navidrome_user_of`]).
///
/// Remembered per browser, on disk, for as long as it keeps being used. These used to live
/// in memory for twelve hours, so every restart and every deploy asked for the sign-in again,
/// which made the dashboard tiresome to use (Brandon, 2026-10-03). Only a hash of each token
/// is written, so the file holds nothing a browser could present; the token itself lives only
/// in the browser's HttpOnly, SameSite=Strict cookie.
pub struct BrowseSessionStore {
    /// Keyed by the token's hash. (A `ConcurrentDictionary` in the C#, whose order, and so the
    /// file's, was unspecified; this keeps insertion order.)
    sessions: Mutex<IndexMap<String, Session>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
    clock: Clock,
}

impl BrowseSessionStore {
    /// Sliding lifetime of a session: it lapses only after this long without a visit.
    pub const TTL: TimeDelta = TimeDelta::days(90);

    /// The name a recovery-code session carries. Not a name Navidrome would give anyone.
    pub const RECOVERY_USER: &'static str = "(recovery code)";

    /// Sliding lifetime of a recovery-code session: long enough to fix the settings.
    pub const RECOVERY_TTL: TimeDelta = TimeDelta::hours(1);

    /// A slide is written to disk at most once a day per session; the rest only moves it in memory.
    const SAVE_SLIDE_EVERY: TimeDelta = TimeDelta::days(1);

    /// In memory only, for tests and for a host with nowhere to keep it.
    pub fn in_memory() -> Self {
        Self::new(None::<PathBuf>)
    }

    /// A store kept at `path` (loaded now; in memory when absent or blank). Sessions already
    /// expired by the system clock are dropped on load.
    pub fn new(path: Option<impl AsRef<Path>>) -> Self {
        let path = path
            .map(|p| p.as_ref().to_path_buf())
            .filter(|p| !p.as_os_str().to_string_lossy().trim().is_empty());
        let mut sessions = IndexMap::new();
        if let Some(path) = &path {
            let loaded = (|| -> anyhow::Result<Vec<Saved>> {
                let Some(text) = state_file::read_text(path)? else {
                    return Ok(Vec::new());
                };
                Ok(serde_json::from_str::<Option<Vec<Saved>>>(&text)?.unwrap_or_default())
            })();
            match loaded {
                Ok(list) => {
                    // The C# compared with DateTime.UtcNow here, not with its settable Clock,
                    // which a test could only set after construction.
                    let now = Utc::now();
                    for saved in list {
                        if saved.expires > now {
                            sessions.insert(
                                saved.hash,
                                Session {
                                    user: saved.user,
                                    expires: saved.expires,
                                    saved_expires: saved.expires,
                                    lifetime: Self::TTL,
                                    persist: true,
                                },
                            );
                        }
                    }
                }
                Err(e) => warn!("browse sessions could not be read, so everyone signs in again: {e}"),
            }
        }
        BrowseSessionStore {
            sessions: Mutex::new(sessions),
            path,
            save_lock: Mutex::new(()),
            clock: Clock::system(),
        }
    }

    /// Reads the time from `clock` from now on (the C# `internal Func<DateTime> Clock`).
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// Mint a token for a verified admin. Sliding expiry starts now.
    pub fn create(&self, username: &str) -> String {
        self.mint(username, Self::TTL, true)
    }

    /// Mint a recovery-code session: an hour, in memory only.
    pub fn create_recovery(&self) -> String {
        self.mint(Self::RECOVERY_USER, Self::RECOVERY_TTL, false)
    }

    fn mint(&self, username: &str, lifetime: TimeDelta, persist: bool) -> String {
        self.prune();
        let token = hex::encode_upper(rand::random::<[u8; 32]>());
        let expires = self.clock.now() + lifetime;
        self.sessions.lock().insert(
            Self::hash(&token),
            Session {
                user: username.to_string(),
                expires,
                saved_expires: expires,
                lifetime,
                persist,
            },
        );
        if persist {
            self.save();
        }
        token
    }

    /// True when the token is live. Valid use slides the expiry, so a browser in use stays
    /// signed in while an abandoned one still lapses.
    pub fn validate(&self, token: Option<&str>) -> bool {
        self.touch(token).is_some()
    }

    /// The Navidrome admin a live token was minted for, or None. Slides the expiry like
    /// Validate. What the Better quality page acts as, so a file is only ever changed for a
    /// person who signed in.
    pub fn user_of(&self, token: Option<&str>) -> Option<String> {
        self.touch(token).map(|s| s.user)
    }

    /// Like [`BrowseSessionStore::user_of`], but None for a recovery-code session: what the
    /// endpoints that read or change library files accept, so the recovery code fixes settings
    /// and nothing else.
    pub fn navidrome_user_of(&self, token: Option<&str>) -> Option<String> {
        self.user_of(token).filter(|user| user != Self::RECOVERY_USER)
    }

    /// Forget a token: the browser's Sign out.
    pub fn revoke(&self, token: Option<&str>) {
        let Some(token) = token.filter(|t| !t.trim().is_empty()) else {
            return;
        };
        let removed = self.sessions.lock().shift_remove(&Self::hash(token));
        if removed.is_some_and(|gone| gone.persist) {
            self.save();
        }
    }

    /// Forget every session of one person: Sign out everywhere, or Navidrome took their admin
    /// role away. Answers how many were ended.
    pub fn revoke_user(&self, user: &str) -> usize {
        let gone = {
            let mut sessions = self.sessions.lock();
            let before = sessions.len();
            sessions.retain(|_, session| !octo_core::common::dotnet::eq_ignore_case(&session.user, user));
            before - sessions.len()
        };
        if gone > 0 {
            self.save();
        }
        gone
    }

    fn touch(&self, token: Option<&str>) -> Option<Session> {
        let token = token.filter(|t| !t.trim().is_empty())?;
        let key = Self::hash(token);
        let now = self.clock.now();
        let mut sessions = self.sessions.lock();
        let session = sessions.get(&key)?.clone();
        if session.expires <= now {
            sessions.shift_remove(&key);
            drop(sessions);
            if session.persist {
                self.save();
            }
            return None;
        }
        let mut slid = Session {
            expires: now + session.lifetime,
            ..session.clone()
        };
        let save = session.persist && slid.expires - session.saved_expires >= Self::SAVE_SLIDE_EVERY;
        if save {
            slid.saved_expires = slid.expires;
        }
        sessions.insert(key, slid.clone());
        drop(sessions);
        if save {
            self.save();
        }
        Some(slid)
    }

    fn hash(token: &str) -> String {
        hex::encode_upper(Sha256::digest(token.as_bytes()))
    }

    fn prune(&self) {
        let now = self.clock.now();
        self.sessions.lock().retain(|_, s| s.expires > now);
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let _saving = self.save_lock.lock();
        let saved: Vec<Saved> = self
            .sessions
            .lock()
            .iter()
            .filter(|(_, s)| s.persist)
            .map(|(hash, s)| Saved {
                hash: hash.clone(),
                user: s.user.clone(),
                expires: s.expires,
            })
            .collect();
        if let Err(e) = state_file::save_atomic(path, &octo_core::json::to_string(&saved)) {
            warn!("browse sessions could not be written: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Arc;

    fn temp_file() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("octo-browse").join("browse-sessions.json");
        (dir, path)
    }

    #[test]
    fn a_minted_token_validates() {
        let store = BrowseSessionStore::in_memory();
        let token = store.create("winters");
        assert!(store.validate(Some(&token)));
    }

    #[test]
    fn anything_not_minted_by_us_is_rejected() {
        for token in [None, Some(""), Some("   "), Some("not-a-real-token")] {
            let store = BrowseSessionStore::in_memory();
            store.create("winters"); // a live session must not make other tokens valid
            assert!(!store.validate(token), "{token:?}");
        }
    }

    #[test]
    fn tokens_are_unpredictable_and_not_shared_between_sessions() {
        let store = BrowseSessionStore::in_memory();
        let first = store.create("winters");
        let second = store.create("winters");
        assert_ne!(first, second);
        // 32 bytes hex. Guessing is not meant to be on the table.
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn a_sign_in_survives_a_restart() {
        let (_dir, path) = temp_file();
        let token = BrowseSessionStore::new(Some(&path)).create("winters");
        let after_restart = BrowseSessionStore::new(Some(&path));
        assert_eq!(after_restart.user_of(Some(&token)).as_deref(), Some("winters"));
    }

    #[test]
    fn the_file_holds_no_token_a_browser_could_present() {
        let (_dir, path) = temp_file();
        let token = BrowseSessionStore::new(Some(&path)).create("winters");
        let written = std::fs::read_to_string(&path).expect("reads");
        assert!(!written.to_lowercase().contains(&token.to_lowercase()));
        assert!(written.contains("winters"));
    }

    #[test]
    fn a_browser_in_use_stays_signed_in_an_unused_one_lapses_after_ninety_days() {
        let now = Arc::new(Mutex::new(Utc.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap()));
        let clock = {
            let now = now.clone();
            Clock::new(move || *now.lock())
        };
        let store = BrowseSessionStore::in_memory().with_clock(clock);
        let token = store.create("winters");

        let mut day = 0;
        while day < 200 {
            *now.lock() += TimeDelta::days(30);
            assert!(
                store.validate(Some(&token)),
                "lapsed while in use, day {}",
                day + 30
            );
            day += 30;
        }

        *now.lock() += TimeDelta::days(91);
        assert!(!store.validate(Some(&token)));
        assert!(store.user_of(Some(&token)).is_none());
    }

    #[test]
    fn sign_out_forgets_the_browser_and_stays_forgotten_after_a_restart() {
        let (_dir, path) = temp_file();
        let store = BrowseSessionStore::new(Some(&path));
        let token = store.create("winters");
        let other = store.create("winters");
        store.revoke(Some(&token));
        assert!(!store.validate(Some(&token)));
        assert!(store.validate(Some(&other)));
        assert!(!BrowseSessionStore::new(Some(&path)).validate(Some(&token)));
    }

    #[test]
    fn an_unreadable_file_means_signing_in_again_not_a_crash() {
        let (_dir, path) = temp_file();
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("creates");
        std::fs::write(&path, "{ not json").expect("writes");
        let store = BrowseSessionStore::new(Some(&path));
        assert!(!store.validate(Some("anything")));
        let token = store.create("winters");
        assert!(store.validate(Some(&token)));
    }

    #[test]
    fn a_recovery_sign_in_lapses_after_an_hour_a_navidrome_one_does_not() {
        let now = Arc::new(Mutex::new(Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()));
        let clock = {
            let now = now.clone();
            Clock::new(move || *now.lock())
        };
        let store = BrowseSessionStore::in_memory().with_clock(clock);
        let recovery = store.create_recovery();
        let admin = store.create("admin");

        *now.lock() += TimeDelta::minutes(61);
        assert!(!store.validate(Some(&recovery)));
        assert!(store.validate(Some(&admin)));
    }

    #[test]
    fn a_recovery_sign_in_never_counts_as_a_navidrome_admin() {
        let store = BrowseSessionStore::in_memory();
        let recovery = store.create_recovery();
        let admin = store.create("admin");

        assert_eq!(
            store.user_of(Some(&recovery)).as_deref(),
            Some(BrowseSessionStore::RECOVERY_USER)
        );
        assert_eq!(store.navidrome_user_of(Some(&recovery)), None);
        assert_eq!(store.navidrome_user_of(Some(&admin)).as_deref(), Some("admin"));
    }

    #[test]
    fn revoke_user_ends_every_sign_in_of_that_person_and_no_one_else() {
        let store = BrowseSessionStore::in_memory();
        let first = store.create("Admin");
        let second = store.create("admin");
        let other = store.create("other");

        assert_eq!(store.revoke_user("ADMIN"), 2);
        assert!(!store.validate(Some(&first)));
        assert!(!store.validate(Some(&second)));
        assert!(store.validate(Some(&other)));
    }

    // AdminSignInTests.ARecoverySignInIsNeverWrittenToDisk
    #[test]
    fn a_recovery_sign_in_is_never_written_to_disk() {
        let (_dir, path) = temp_file();
        let store = BrowseSessionStore::new(Some(&path));
        store.create("admin");
        let recovery = store.create_recovery();

        assert_eq!(
            store.user_of(Some(&recovery)).as_deref(),
            Some(BrowseSessionStore::RECOVERY_USER)
        );
        assert_eq!(store.navidrome_user_of(Some(&recovery)), None);
        let written: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("reads")).expect("parses");
        assert_eq!(written.len(), 1);
    }

    // ---- Rust-only ----------------------------------------------------------------------

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/rust-migration/fixtures/state/browse-sessions.json"
    );

    #[test]
    fn the_fixture_round_trips_byte_for_byte() {
        let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
        let saved: Vec<Saved> = serde_json::from_str(&text).expect("the fixture reads");
        assert_eq!(saved.len(), 2);
        assert_eq!(octo_core::json::to_string(&saved), text.trim_end_matches('\n'));
    }

    #[test]
    fn the_hash_is_upper_case_hex_sha256_of_the_token() {
        // The fixture's first hash is SHA-256("test").
        assert_eq!(
            BrowseSessionStore::hash("test"),
            "9F86D081884C7D659A2FEAA0C55AD015A3BF4F1B2B0B822CD15D6C15B0F00A08"
        );
    }

    #[test]
    fn a_slide_is_written_at_most_once_a_day_and_expired_sessions_drop_on_load() {
        let (_dir, path) = temp_file();
        let start = Utc::now();
        let now = Arc::new(Mutex::new(start));
        let clock = {
            let now = now.clone();
            Clock::new(move || *now.lock())
        };
        let store = BrowseSessionStore::new(Some(&path)).with_clock(clock);
        let token = store.create("winters");
        let first = std::fs::read_to_string(&path).expect("reads");

        *now.lock() = start + TimeDelta::hours(23);
        assert!(store.validate(Some(&token)));
        assert_eq!(
            std::fs::read_to_string(&path).expect("reads"),
            first,
            "under a day: memory only"
        );

        *now.lock() = start + TimeDelta::hours(25);
        assert!(store.validate(Some(&token)));
        assert_ne!(
            std::fs::read_to_string(&path).expect("reads"),
            first,
            "a day on: written"
        );

        // A session that expired while Octo was down is gone after a restart.
        let lapsed = format!(
            r#"[{{"Hash":"AB","User":"old","Expires":"{}"}}]"#,
            datetime::format_utc(&(Utc::now() - TimeDelta::minutes(1)))
        );
        std::fs::write(&path, lapsed).expect("writes");
        let reloaded = BrowseSessionStore::new(Some(&path));
        assert!(reloaded.sessions.lock().is_empty());
    }
}
