//! `family.json`: the owner, the members, their devices (secrets sealed), devices seen for the
//! owner and unmanaged accounts, invites and pairing codes. Invite tokens and pairing codes are
//! kept only as SHA-256 hashes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::family::abilities::{Abilities, Preset};
use octo_core::family::codes::constant_time_eq;
use octo_core::family::device::{Device, DeviceKind, NetworkPlace};
use octo_core::family::quality::{DeviceQualityMode, QualityPreference};
use octo_core::json::datetime;
use parking_lot::Mutex;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::store_file;
use super::vault::SEALED_PREFIX;

/// Which shared (non-personal) Navidrome libraries a member sees: every one, including those
/// added later, or a fixed list. Written as `"All"` or a list of ids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SharedLibraries {
    #[default]
    All,
    Ids(Vec<i64>),
}

impl Serialize for SharedLibraries {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            SharedLibraries::All => s.serialize_str("All"),
            SharedLibraries::Ids(ids) => ids.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for SharedLibraries {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(text) if text.trim().eq_ignore_ascii_case("all") => Ok(SharedLibraries::All),
            Value::Null => Ok(SharedLibraries::All),
            Value::Array(items) => items
                .into_iter()
                .map(|item| item.as_i64().ok_or_else(|| de::Error::custom("a library id")))
                .collect::<Result<Vec<_>, _>>()
                .map(SharedLibraries::Ids),
            other => Err(de::Error::custom(format!(
                "\"All\" or a list of library ids, not {other}"
            ))),
        }
    }
}

impl SharedLibraries {
    /// The ids this member sees out of `shared` (every non-personal library there is now).
    pub fn resolve(&self, shared: &[i64]) -> Vec<i64> {
        match self {
            SharedLibraries::All => shared.to_vec(),
            SharedLibraries::Ids(ids) => ids.iter().copied().filter(|id| shared.contains(id)).collect(),
        }
    }
}

/// One member as `family.json` keeps them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Member {
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    pub preset: Preset,
    /// Abilities changed from the preset, by name.
    #[serde(default)]
    pub edits: Map<String, Value>,
    #[serde(default)]
    pub navidrome_user_id: String,
    /// The hidden Navidrome password Octo signs in with for this member, sealed.
    #[serde(default)]
    pub navidrome_password: String,
    #[serde(default)]
    pub personal_library_id: Option<i64>,
    #[serde(default)]
    pub shared_libraries: SharedLibraries,
    /// The account password's Argon2id hash; empty until the member sets one.
    #[serde(default)]
    pub account_password: String,
    #[serde(with = "datetime::utc")]
    pub created: DateTime<Utc>,
    #[serde(default)]
    pub suspended: bool,
    #[serde(default)]
    pub ntfy_topic: String,
}

impl Member {
    /// The member's abilities: their preset with their edits.
    pub fn abilities(&self) -> Abilities {
        Abilities::for_member(self.preset, &self.edits).0
    }

    /// The name shown for the member: their display name, or their username.
    pub fn shown_name(&self) -> &str {
        if self.display_name.trim().is_empty() {
            &self.username
        } else {
            &self.display_name
        }
    }
}

/// An invite link waiting to be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Invite {
    pub token_hash: String,
    pub username: String,
    #[serde(with = "datetime::utc")]
    pub expires: DateTime<Utc>,
}

/// A pairing code waiting for an Octo app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PairCode {
    pub code_hash: String,
    pub username: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(with = "datetime::utc")]
    pub expires: DateTime<Utc>,
    #[serde(default)]
    pub tries: u32,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FamilyFile {
    pub version: u32,
    pub owner: String,
    pub members: Vec<Member>,
    pub devices: Vec<Device>,
    /// Devices seen for the owner and accounts outside the family (no secret).
    pub detected: Vec<Device>,
    /// The audio quality each account chose, by username; only accounts that chose
    /// something other than Original are listed.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub preferences: IndexMap<String, QualityPreference>,
    pub invites: Vec<Invite>,
    pub pair_codes: Vec<PairCode>,
}

impl Default for FamilyFile {
    fn default() -> Self {
        FamilyFile {
            version: 1,
            owner: String::new(),
            members: Vec::new(),
            devices: Vec::new(),
            detected: Vec::new(),
            preferences: IndexMap::new(),
            invites: Vec::new(),
            pair_codes: Vec::new(),
        }
    }
}

/// What trying a pairing code came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairOutcome {
    /// The code was right: the member and the device name it was made for. It is used up.
    Ok { username: String, device_name: String },
    /// Not the member's live code (a wrong guess counts against it).
    Wrong,
    /// The code was right but too late.
    Expired,
    /// Five wrong guesses: the code no longer works.
    TooManyTries,
}

/// Wrong guesses a pairing code takes before it stops working.
pub const PAIR_CODE_TRIES: u32 = 5;

/// Detected devices kept at most; the longest unseen go first.
pub const DETECTED_LIMIT: usize = 200;

/// The store behind `family.json`. Every change but a device's last-seen details is written at
/// once; those are kept in memory and written by [`FamilyStore::flush_if_dirty`].
pub struct FamilyStore {
    state: Mutex<FamilyFile>,
    dirty: AtomicBool,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
    load_warning: Option<String>,
}

fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

impl FamilyStore {
    /// In memory only, for tests.
    pub fn in_memory() -> Self {
        Self::new(None::<PathBuf>)
    }

    /// The store kept at `path`, loaded now.
    pub fn new(path: Option<impl AsRef<Path>>) -> Self {
        let path = store_file::clean_path(path);
        let (file, load_warning) = store_file::load::<FamilyFile>(path.as_deref());
        FamilyStore {
            state: Mutex::new(file),
            dirty: AtomicBool::new(false),
            path,
            save_lock: Mutex::new(()),
            load_warning,
        }
    }

    /// Why the file could not be read at start, when it could not.
    pub fn load_warning(&self) -> Option<&str> {
        self.load_warning.as_deref()
    }

    /// Whether anything in the store is sealed (and so needs `family.key` to open).
    pub fn has_sealed_secrets(&self) -> bool {
        let state = self.state.lock();
        state
            .members
            .iter()
            .any(|m| m.navidrome_password.starts_with(SEALED_PREFIX))
            || state.devices.iter().any(|d| d.secret.starts_with(SEALED_PREFIX))
    }

    pub fn owner(&self) -> String {
        self.state.lock().owner.clone()
    }

    pub fn set_owner(&self, name: &str) {
        self.state.lock().owner = name.trim().to_string();
        self.save();
    }

    /// Whether `name` is the owner (ignoring case).
    pub fn is_owner(&self, name: &str) -> bool {
        let owner = self.state.lock().owner.clone();
        !owner.is_empty() && eq_ignore_case(&owner, name)
    }

    /// The member named `name` (ignoring case).
    pub fn member(&self, name: &str) -> Option<Member> {
        self.state
            .lock()
            .members
            .iter()
            .find(|m| eq_ignore_case(&m.username, name))
            .cloned()
    }

    pub fn members(&self) -> Vec<Member> {
        self.state.lock().members.clone()
    }

    /// Each member's own library id with their name, without copying anything else.
    pub fn personal_libraries(&self) -> Vec<(i64, String)> {
        self.state
            .lock()
            .members
            .iter()
            .filter_map(|m| m.personal_library_id.map(|id| (id, m.username.clone())))
            .collect()
    }

    /// Whether any member has a library of their own.
    pub fn has_personal_libraries(&self) -> bool {
        self.state
            .lock()
            .members
            .iter()
            .any(|m| m.personal_library_id.is_some())
    }

    /// Adds the member, or replaces the one with the same name.
    pub fn upsert_member(&self, member: Member) {
        {
            let mut state = self.state.lock();
            match state
                .members
                .iter_mut()
                .find(|m| eq_ignore_case(&m.username, &member.username))
            {
                Some(existing) => *existing = member,
                None => state.members.push(member),
            }
        }
        self.save();
    }

    /// Removes a member with their devices, invites and pairing codes. Answers whether they
    /// were there.
    pub fn remove_member(&self, name: &str) -> bool {
        let removed = {
            let mut state = self.state.lock();
            let before = state.members.len();
            state.members.retain(|m| !eq_ignore_case(&m.username, name));
            state.devices.retain(|d| !eq_ignore_case(&d.username, name));
            state.invites.retain(|i| !eq_ignore_case(&i.username, name));
            state.pair_codes.retain(|c| !eq_ignore_case(&c.username, name));
            before != state.members.len()
        };
        if removed {
            self.save();
        }
        removed
    }

    /// The member's signed-in devices.
    pub fn devices_of(&self, name: &str) -> Vec<Device> {
        self.state
            .lock()
            .devices
            .iter()
            .filter(|d| eq_ignore_case(&d.username, name))
            .cloned()
            .collect()
    }

    /// Every signed-in device of every member.
    pub fn devices(&self) -> Vec<Device> {
        self.state.lock().devices.clone()
    }

    pub fn device(&self, id: &str) -> Option<Device> {
        self.state.lock().devices.iter().find(|d| d.id == id).cloned()
    }

    /// Adds a device, or replaces the one with the same id.
    pub fn add_device(&self, device: Device) {
        {
            let mut state = self.state.lock();
            match state.devices.iter_mut().find(|d| d.id == device.id) {
                Some(existing) => *existing = device,
                None => state.devices.push(device),
            }
        }
        self.save();
    }

    /// Signs a device out. Answers whether it was there.
    pub fn remove_device(&self, id: &str) -> bool {
        let removed = {
            let mut state = self.state.lock();
            let before = state.devices.len();
            state.devices.retain(|d| d.id != id);
            before != state.devices.len()
        };
        if removed {
            self.save();
        }
        removed
    }

    /// A device was just used: its last seen time, app and place change in memory only.
    /// Answers whether the device is known.
    pub fn touch_device(&self, id: &str, app: &str, place: NetworkPlace, now: DateTime<Utc>) -> bool {
        let mut state = self.state.lock();
        let Some(device) = state.devices.iter_mut().find(|d| d.id == id) else {
            return false;
        };
        touch(device, app, place, now);
        self.dirty.store(true, Ordering::Release);
        true
    }

    /// A member's device was just used: like [`FamilyStore::touch_device`], and the name the
    /// device gave is kept when it has none yet (a name given at pairing or by a manager
    /// stays). Answers None when the device is unknown, else whether this is the first time it
    /// has been seen.
    pub fn seen_device(
        &self,
        id: &str,
        app: &str,
        place: NetworkPlace,
        name: Option<&str>,
        now: DateTime<Utc>,
    ) -> Option<bool> {
        let mut state = self.state.lock();
        let device = state.devices.iter_mut().find(|d| d.id == id)?;
        let first = device.last_seen.is_none();
        touch(device, app, place, now);
        if let Some(name) = name.filter(|n| !n.trim().is_empty())
            && device.name.trim().is_empty()
        {
            device.name = name.to_string();
        }
        self.dirty.store(true, Ordering::Release);
        Some(first)
    }

    /// Keeps a web device's real Navidrome sign-in (sealed), in memory until the next flush:
    /// Navidrome hands out a fresh one on every call, so writing each would write the file on
    /// every page a member opens. Answers whether the device is known.
    pub fn set_device_jwt(&self, id: &str, sealed: Option<String>) -> bool {
        let mut state = self.state.lock();
        let Some(device) = state.devices.iter_mut().find(|d| d.id == id) else {
            return false;
        };
        device.navidrome_jwt = sealed;
        self.dirty.store(true, Ordering::Release);
        true
    }

    /// How a device picks its audio quality: a member's device or a detected one, by id.
    /// [`DeviceQualityMode::Account`] for a device Octo does not know.
    pub fn device_quality(&self, id: &str) -> DeviceQualityMode {
        let state = self.state.lock();
        state
            .devices
            .iter()
            .chain(state.detected.iter())
            .find(|d| d.id == id)
            .map(|d| d.quality)
            .unwrap_or_default()
    }

    /// Sets how a device picks its audio quality (a member's device or a detected one).
    /// Answers whether the device is known.
    pub fn set_device_quality(&self, id: &str, mode: DeviceQualityMode) -> bool {
        let found = {
            let mut state = self.state.lock();
            let state = &mut *state;
            match state
                .devices
                .iter_mut()
                .chain(state.detected.iter_mut())
                .find(|d| d.id == id)
            {
                Some(device) => {
                    device.quality = mode;
                    true
                }
                None => false,
            }
        };
        if found {
            self.save();
        }
        found
    }

    /// The audio quality an account chose, by username (ignoring case). Any account has one:
    /// a member, the owner or an account outside the family. Original at home and away until
    /// they choose.
    pub fn preference(&self, username: &str) -> QualityPreference {
        self.state
            .lock()
            .preferences
            .iter()
            .find(|(name, _)| eq_ignore_case(name, username))
            .map(|(_, preference)| *preference)
            .unwrap_or_default()
    }

    /// Sets an account's audio quality. Choosing Original for both places removes the entry,
    /// so the file only lists accounts that chose something else.
    pub fn set_preference(&self, username: &str, preference: QualityPreference) {
        {
            let mut state = self.state.lock();
            state
                .preferences
                .retain(|name, _| !eq_ignore_case(name, username));
            if preference != QualityPreference::default() {
                state.preferences.insert(username.trim().to_string(), preference);
            }
        }
        self.save();
    }

    /// Devices seen for the owner and accounts outside the family.
    pub fn detected(&self) -> Vec<Device> {
        self.state.lock().detected.clone()
    }

    /// A device of the owner or an unmanaged account was seen. Answers true the first time
    /// this device is seen. Kept in memory until the next flush; at most [`DETECTED_LIMIT`]
    /// are kept, the longest unseen dropped first.
    pub fn note_detected(
        &self,
        id: &str,
        username: &str,
        app: &str,
        place: NetworkPlace,
        name: Option<&str>,
        now: DateTime<Utc>,
    ) -> bool {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        let mut state = self.state.lock();
        let new = match state.detected.iter_mut().find(|d| d.id == id) {
            Some(device) => {
                touch(device, app, place, now);
                if let Some(name) = name {
                    device.name = name.to_string();
                }
                false
            }
            None => {
                let app = app_label_kept(app);
                state.detected.push(Device {
                    id: id.to_string(),
                    username: username.to_string(),
                    name: name.unwrap_or_default().to_string(),
                    kind: DeviceKind::Detected,
                    secret: String::new(),
                    created: now,
                    last_seen: Some(now),
                    app: app.clone(),
                    place,
                    seen_apps: vec![app],
                    navidrome_jwt: None,
                    quality: DeviceQualityMode::Account,
                });
                if state.detected.len() > DETECTED_LIMIT {
                    state.detected.sort_by_key(|d| std::cmp::Reverse(d.last_seen));
                    state.detected.truncate(DETECTED_LIMIT);
                }
                true
            }
        };
        self.dirty.store(true, Ordering::Release);
        new
    }

    /// Writes the file when a device has been seen since the last write.
    pub fn flush_if_dirty(&self) {
        if self.dirty.swap(false, Ordering::AcqRel) {
            self.save();
        }
    }

    /// Keeps an invite for `username` (only its hash is stored).
    pub fn add_invite(&self, token: &str, username: &str, expires: DateTime<Utc>) {
        self.state.lock().invites.push(Invite {
            token_hash: sha256_hex(token),
            username: username.to_string(),
            expires,
        });
        self.save();
    }

    /// The member an invite was for, using it up; None when unknown or expired. Expired
    /// invites are dropped on the way.
    pub fn take_invite(&self, token: &str, now: DateTime<Utc>) -> Option<String> {
        let hash = sha256_hex(token);
        let (found, changed) = {
            let mut state = self.state.lock();
            let before = state.invites.len();
            let found = state
                .invites
                .iter()
                .position(|i| constant_time_eq(i.token_hash.as_bytes(), hash.as_bytes()));
            let taken = found.map(|at| state.invites.remove(at));
            state.invites.retain(|i| i.expires > now);
            let found = taken.filter(|i| i.expires > now).map(|i| i.username);
            (found, before != state.invites.len())
        };
        if changed {
            self.save();
        }
        found
    }

    /// Keeps a pairing code for `username`, replacing any code they had.
    pub fn add_pair_code(&self, code: &str, username: &str, device_name: &str, expires: DateTime<Utc>) {
        {
            let mut state = self.state.lock();
            state
                .pair_codes
                .retain(|c| !eq_ignore_case(&c.username, username));
            state.pair_codes.push(PairCode {
                code_hash: sha256_hex(code),
                username: username.to_string(),
                device_name: device_name.to_string(),
                expires,
                tries: 0,
            });
        }
        self.save();
    }

    /// Whether any member has a pairing code that still works.
    pub fn any_live_pair_code(&self, now: DateTime<Utc>) -> bool {
        self.state
            .lock()
            .pair_codes
            .iter()
            .any(|c| c.expires > now && c.tries < PAIR_CODE_TRIES)
    }

    /// Tries `code` against `username`'s pairing code. A wrong guess counts against that code,
    /// and the fifth burns it.
    pub fn take_pair_code(&self, username: &str, code: &str, now: DateTime<Utc>) -> PairOutcome {
        let hash = sha256_hex(code.trim());
        let outcome = {
            let mut state = self.state.lock();
            let Some(at) = state
                .pair_codes
                .iter()
                .position(|c| eq_ignore_case(&c.username, username))
            else {
                return PairOutcome::Wrong;
            };
            let entry = &mut state.pair_codes[at];
            if entry.tries >= PAIR_CODE_TRIES {
                if entry.expires <= now {
                    state.pair_codes.remove(at);
                }
                PairOutcome::TooManyTries
            } else if constant_time_eq(entry.code_hash.as_bytes(), hash.as_bytes()) {
                let entry = state.pair_codes.remove(at);
                if entry.expires <= now {
                    PairOutcome::Expired
                } else {
                    PairOutcome::Ok {
                        username: entry.username,
                        device_name: entry.device_name,
                    }
                }
            } else {
                entry.tries += 1;
                if entry.tries >= PAIR_CODE_TRIES {
                    PairOutcome::TooManyTries
                } else {
                    PairOutcome::Wrong
                }
            }
        };
        self.save();
        outcome
    }

    /// A copy of the whole file, for tests and the Family page's summary.
    pub fn snapshot(&self) -> FamilyFile {
        self.state.lock().clone()
    }

    fn save(&self) {
        store_file::save_with(self.path.as_deref(), &self.save_lock, || {
            // Whatever is in memory now goes out, so a pending last-seen change is written too.
            self.dirty.store(false, Ordering::Release);
            self.state.lock().clone()
        });
    }
}

/// The most characters an app label keeps. Labels come from what clients send.
pub const APP_LABEL_LIMIT: usize = 64;
/// The most apps one device remembers; the oldest go first.
pub const SEEN_APPS_LIMIT: usize = 16;

/// An app label cut to [`APP_LABEL_LIMIT`] characters.
pub fn app_label_kept(app: &str) -> String {
    app.chars().take(APP_LABEL_LIMIT).collect()
}

fn touch(device: &mut Device, app: &str, place: NetworkPlace, now: DateTime<Utc>) {
    device.last_seen = Some(now);
    device.place = place;
    let app = app_label_kept(app);
    if !app.is_empty() {
        if !device.seen_apps.contains(&app) {
            device.seen_apps.push(app.clone());
        }
        let extra = device.seen_apps.len().saturating_sub(SEEN_APPS_LIMIT);
        device.seen_apps.drain(..extra);
        device.app = app;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, TimeZone};
    use octo_core::family::quality::StreamQuality;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 18, 0, 0).unwrap()
    }

    fn member(name: &str) -> Member {
        Member {
            username: name.to_string(),
            display_name: "Alex".into(),
            preset: Preset::Listener,
            edits: serde_json::from_str(r#"{"OfflineCopies":true}"#).expect("an object"),
            navidrome_user_id: "a1b2c3".into(),
            navidrome_password: "sealed:AAAA".into(),
            personal_library_id: Some(3),
            shared_libraries: SharedLibraries::Ids(vec![1]),
            account_password: "$argon2id$v=19$m=19456,t=2,p=1$salt$hash".into(),
            created: now(),
            suspended: false,
            ntfy_topic: String::new(),
        }
    }

    fn device(id: &str, user: &str) -> Device {
        Device {
            id: id.to_string(),
            username: user.to_string(),
            name: "Pixel 9".into(),
            kind: DeviceKind::OctoApp,
            secret: "sealed:BBBB".into(),
            created: now(),
            last_seen: None,
            app: String::new(),
            place: NetworkPlace::Home,
            seen_apps: Vec::new(),
            navidrome_jwt: None,
            quality: DeviceQualityMode::Account,
        }
    }

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("family.json");
        (dir, path)
    }

    const FIXTURE: &str = r#"{"Version":1,"Owner":"winters","Members":[{"Username":"alex","DisplayName":"Alex","Preset":"Listener","Edits":{"OfflineCopies":true},"NavidromeUserId":"a1b2c3","NavidromePassword":"sealed:AAAA","PersonalLibraryId":3,"SharedLibraries":[1],"AccountPassword":"$argon2id$v=19$m=19456,t=2,p=1$salt$hash","Created":"2026-10-20T18:00:00Z","Suspended":false,"NtfyTopic":""}],"Devices":[{"Id":"d_1","Username":"alex","Name":"Pixel 9","Kind":"OctoApp","Secret":"sealed:BBBB","Created":"2026-10-20T18:00:00Z","LastSeen":null,"App":"","Place":"Home","SeenApps":[]}],"Detected":[],"Invites":[{"TokenHash":"abc","Username":"alex","Expires":"2026-10-27T18:00:00Z"}],"PairCodes":[{"CodeHash":"def","Username":"alex","DeviceName":"","Expires":"2026-10-20T18:10:00Z","Tries":0}]}"#;

    #[test]
    fn round_trip_fixture() {
        let (_dir, path) = temp();
        std::fs::write(&path, FIXTURE).expect("writes");
        let store = FamilyStore::new(Some(&path));
        assert!(store.load_warning().is_none());
        assert_eq!(store.owner(), "winters");
        assert_eq!(store.member("alex"), Some(member("alex")));
        assert_eq!(store.devices_of("alex"), vec![device("d_1", "alex")]);
        assert!(store.has_sealed_secrets());
        // Written back byte for byte.
        store.set_owner("winters");
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), FIXTURE);
    }

    #[test]
    fn shared_libraries_read_all_or_ids() {
        for (text, expected) in [
            ("\"All\"", SharedLibraries::All),
            ("\"all\"", SharedLibraries::All),
            ("[1,4]", SharedLibraries::Ids(vec![1, 4])),
            ("[]", SharedLibraries::Ids(vec![])),
        ] {
            assert_eq!(
                serde_json::from_str::<SharedLibraries>(text).expect(text),
                expected
            );
        }
        assert_eq!(
            serde_json::to_string(&SharedLibraries::All).expect("writes"),
            "\"All\""
        );
        assert!(serde_json::from_str::<SharedLibraries>("\"some\"").is_err());
        assert_eq!(SharedLibraries::All.resolve(&[1, 5]), vec![1, 5]);
        assert_eq!(SharedLibraries::Ids(vec![1, 9]).resolve(&[1, 5]), vec![1]);
    }

    #[test]
    fn member_lookup_ignores_case() {
        let store = FamilyStore::in_memory();
        store.upsert_member(member("alex"));
        assert!(store.member("ALEX").is_some());
        assert!(store.member("sam").is_none());
        let mut renamed = member("Alex");
        renamed.display_name = "Alexandra".into();
        store.upsert_member(renamed);
        assert_eq!(store.members().len(), 1);
        assert_eq!(store.member("alex").expect("there").display_name, "Alexandra");
        assert!(store.member("alex").expect("there").abilities().offline_copies);
        assert!(store.remove_member("aLeX"));
        assert!(!store.remove_member("alex"));
    }

    #[test]
    fn removing_a_member_signs_out_their_devices() {
        let store = FamilyStore::in_memory();
        store.upsert_member(member("alex"));
        store.add_device(device("d_1", "alex"));
        store.add_device(device("d_2", "sam"));
        store.add_invite("tok", "alex", now() + TimeDelta::days(7));
        store.remove_member("alex");
        assert!(store.device("d_1").is_none());
        assert!(store.device("d_2").is_some());
        assert_eq!(store.take_invite("tok", now()), None);
    }

    #[test]
    fn invite_is_single_use() {
        let store = FamilyStore::in_memory();
        store.add_invite("the-token", "alex", now() + TimeDelta::days(7));
        assert!(!store.snapshot().invites[0].token_hash.contains("the-token"));
        assert_eq!(store.take_invite("wrong", now()), None);
        assert_eq!(store.take_invite("the-token", now()).as_deref(), Some("alex"));
        assert_eq!(store.take_invite("the-token", now()), None);
    }

    #[test]
    fn expired_invite_is_refused() {
        let store = FamilyStore::in_memory();
        store.add_invite("old", "alex", now() - TimeDelta::seconds(1));
        store.add_invite("live", "sam", now() + TimeDelta::days(1));
        assert_eq!(store.take_invite("old", now()), None);
        // Expired ones are dropped on the way; live ones stay.
        assert_eq!(store.snapshot().invites.len(), 1);
        assert_eq!(store.take_invite("live", now()).as_deref(), Some("sam"));
    }

    #[test]
    fn pair_code_locks_after_five_tries() {
        let store = FamilyStore::in_memory();
        store.add_pair_code("123456", "alex", "Pixel 9", now() + TimeDelta::minutes(10));
        assert!(store.any_live_pair_code(now()));
        for _ in 0..4 {
            assert_eq!(store.take_pair_code("alex", "000000", now()), PairOutcome::Wrong);
        }
        assert_eq!(
            store.take_pair_code("alex", "000000", now()),
            PairOutcome::TooManyTries
        );
        // Burned: even the right code no longer works.
        assert_eq!(
            store.take_pair_code("alex", "123456", now()),
            PairOutcome::TooManyTries
        );
        assert!(!store.any_live_pair_code(now()));
    }

    #[test]
    fn pair_code_works_once_for_its_member_only() {
        let store = FamilyStore::in_memory();
        store.add_pair_code("123456", "alex", "Pixel 9", now() + TimeDelta::minutes(10));
        assert_eq!(store.take_pair_code("sam", "123456", now()), PairOutcome::Wrong);
        assert_eq!(
            store.take_pair_code("ALEX", " 123456 ", now()),
            PairOutcome::Ok {
                username: "alex".into(),
                device_name: "Pixel 9".into()
            }
        );
        assert_eq!(store.take_pair_code("alex", "123456", now()), PairOutcome::Wrong);
        store.add_pair_code("654321", "alex", "", now() - TimeDelta::seconds(1));
        assert_eq!(
            store.take_pair_code("alex", "654321", now()),
            PairOutcome::Expired
        );
    }

    #[test]
    fn corrupt_file_is_kept_aside() {
        let (dir, path) = temp();
        std::fs::write(&path, "{ not json").expect("writes");
        let store = FamilyStore::new(Some(&path));
        assert!(store.load_warning().expect("a warning").contains("family.json"));
        assert!(store.members().is_empty());
        assert!(!path.exists());
        let kept: Vec<String> = std::fs::read_dir(dir.path())
            .expect("lists")
            .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].starts_with("family.json.broken-"), "{kept:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&kept[0])).expect("reads"),
            "{ not json"
        );
    }

    #[test]
    fn touch_does_not_write_until_flush() {
        let (_dir, path) = temp();
        let store = FamilyStore::new(Some(&path));
        store.add_device(device("d_1", "alex"));
        let written = std::fs::read_to_string(&path).expect("reads");
        assert!(store.touch_device("d_1", "Octo 1.6 (Android)", NetworkPlace::Away, now()));
        assert!(!store.touch_device("d_9", "x", NetworkPlace::Away, now()));
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), written);
        store.flush_if_dirty();
        let flushed = std::fs::read_to_string(&path).expect("reads");
        assert!(flushed.contains("Octo 1.6 (Android)"));
        assert!(flushed.contains("\"Place\":\"Away\""));
        // Nothing new: no write.
        std::fs::remove_file(&path).expect("removes");
        store.flush_if_dirty();
        assert!(!path.exists());
    }

    #[test]
    fn a_device_keeps_a_bounded_app_label_and_list_of_apps() {
        let store = FamilyStore::in_memory();
        store.add_device(device("d_1", "alex"));
        for n in 0..(SEEN_APPS_LIMIT * 3) {
            assert!(store.touch_device("d_1", &format!("App {n}"), NetworkPlace::Home, now()));
        }
        let long = "x".repeat(APP_LABEL_LIMIT * 10);
        assert!(store.touch_device("d_1", &long, NetworkPlace::Home, now()));
        let kept = store.device("d_1").expect("there");
        assert_eq!(kept.app.chars().count(), APP_LABEL_LIMIT);
        assert_eq!(kept.seen_apps.len(), SEEN_APPS_LIMIT);
        assert_eq!(kept.seen_apps.last(), Some(&kept.app), "the newest app stays");
        assert!(store.note_detected("x_1", "winters", &long, NetworkPlace::Home, None, now()));
        let detected = store
            .detected()
            .into_iter()
            .find(|d| d.id == "x_1")
            .expect("there");
        assert_eq!(detected.app.chars().count(), APP_LABEL_LIMIT);
    }

    #[test]
    fn seen_device_reports_the_first_sight_and_keeps_a_given_name() {
        let store = FamilyStore::in_memory();
        let mut unnamed = device("d_1", "alex");
        unnamed.name = String::new();
        store.add_device(unnamed);
        store.add_device(device("d_2", "alex"));
        let seen = |id: &str, name: Option<&str>| {
            store.seen_device(id, "Octo 1.6 (Android)", NetworkPlace::Away, name, now())
        };
        assert_eq!(seen("d_1", Some("Sam\u{2019}s phone")), Some(true));
        assert_eq!(seen("d_1", Some("Other")), Some(false));
        assert_eq!(store.device("d_1").expect("there").name, "Sam\u{2019}s phone");
        // A device named at pairing keeps its name.
        assert_eq!(seen("d_2", Some("Other")), Some(true));
        assert_eq!(store.device("d_2").expect("there").name, "Pixel 9");
        assert_eq!(seen("d_9", None), None);
        assert!(store.set_device_jwt("d_1", Some("sealed:J".into())));
        assert_eq!(
            store.device("d_1").expect("there").navidrome_jwt.as_deref(),
            Some("sealed:J")
        );
        assert!(!store.set_device_jwt("d_9", None));
    }

    #[test]
    fn preferences_and_device_quality_round_trip_and_stay_out_of_the_file_while_unused() {
        let (_dir, path) = temp();
        let store = FamilyStore::new(Some(&path));
        store.add_device(device("d_1", "alex"));
        store.note_detected("x1", "winters", "Feishin", NetworkPlace::Home, None, now());
        store.flush_if_dirty();
        let written = std::fs::read_to_string(&path).expect("reads");
        assert!(!written.contains("Preferences"), "{written}");
        assert!(!written.contains("Quality"), "{written}");

        assert_eq!(store.preference("WINTERS"), QualityPreference::default());
        let chosen = QualityPreference {
            home: StreamQuality::Original,
            away: StreamQuality::Standard,
        };
        store.set_preference("winters", chosen);
        store.set_preference(
            "Alex",
            QualityPreference {
                home: StreamQuality::High,
                away: StreamQuality::DataSaver,
            },
        );
        assert!(store.set_device_quality("d_1", DeviceQualityMode::App));
        assert!(store.set_device_quality("x1", DeviceQualityMode::App));
        assert!(!store.set_device_quality("d_9", DeviceQualityMode::App));

        let reloaded = FamilyStore::new(Some(&path));
        assert_eq!(reloaded.preference("Winters"), chosen);
        assert_eq!(reloaded.preference("alex").away, StreamQuality::DataSaver);
        assert_eq!(reloaded.device_quality("d_1"), DeviceQualityMode::App);
        assert_eq!(reloaded.device_quality("x1"), DeviceQualityMode::App);
        assert_eq!(reloaded.device_quality("d_9"), DeviceQualityMode::Account);
        let written = std::fs::read_to_string(&path).expect("reads");
        assert!(
            written.contains("\"Preferences\":{\"winters\":{\"Home\":\"Original\",\"Away\":\"Standard\"}"),
            "{written}"
        );

        // Back to Original everywhere: the entry goes.
        reloaded.set_preference("ALEX", QualityPreference::default());
        assert_eq!(reloaded.snapshot().preferences.len(), 1);
    }

    #[test]
    fn detected_devices_are_capped_and_flushed() {
        let (_dir, path) = temp();
        let store = FamilyStore::new(Some(&path));
        assert!(store.note_detected("x1", "winters", "Feishin", NetworkPlace::Home, None, now()));
        assert!(!store.note_detected(
            "x1",
            "winters",
            "Feishin 0.12",
            NetworkPlace::Away,
            Some("Desk"),
            now()
        ));
        assert!(!path.exists());
        store.flush_if_dirty();
        let reloaded = FamilyStore::new(Some(&path));
        let detected = reloaded.detected();
        assert_eq!(detected.len(), 1);
        assert_eq!(detected[0].seen_apps, ["Feishin", "Feishin 0.12"]);
        assert_eq!(detected[0].kind, DeviceKind::Detected);
        assert_eq!(detected[0].name, "Desk");
        for n in 0..DETECTED_LIMIT + 5 {
            store.note_detected(
                &format!("y{n}"),
                "bob",
                "DSub",
                NetworkPlace::Home,
                None,
                now() + TimeDelta::seconds(n as i64),
            );
        }
        let kept = store.detected();
        assert_eq!(kept.len(), DETECTED_LIMIT);
        assert!(
            !kept.iter().any(|d| d.id == "x1"),
            "the longest unseen goes first"
        );
    }
}
