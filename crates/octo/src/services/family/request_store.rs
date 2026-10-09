//! `family-requests.json`: every member's requests for a copy, newest last. Finished requests
//! are kept for 180 days and at most 2,000 of them; pending and approved ones are never
//! dropped.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::family::request::{FamilyRequest, RequestState, quota_used};
use parking_lot::Mutex;

use super::store_file;

/// Finished requests older than this are dropped.
pub const KEEP_DAYS: i64 = 180;
/// At most this many requests are kept (the oldest finished ones go first).
pub const KEEP_COUNT: usize = 2_000;

/// The store behind `family-requests.json`.
pub struct RequestStore {
    requests: Mutex<Vec<FamilyRequest>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
    load_warning: Option<String>,
}

fn is_live(request: &FamilyRequest) -> bool {
    matches!(request.state, RequestState::Pending | RequestState::Approved)
}

/// A new request id: `r_` and 12 URL-safe characters.
pub fn new_request_id() -> String {
    use base64::Engine;
    format!(
        "r_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 9]>())
    )
}

impl RequestStore {
    pub fn in_memory() -> Self {
        Self::new(None::<PathBuf>)
    }

    pub fn new(path: Option<impl AsRef<Path>>) -> Self {
        let path = store_file::clean_path(path);
        let (requests, load_warning) = store_file::load::<Vec<FamilyRequest>>(path.as_deref());
        RequestStore {
            requests: Mutex::new(requests),
            path,
            save_lock: Mutex::new(()),
            load_warning,
        }
    }

    pub fn load_warning(&self) -> Option<&str> {
        self.load_warning.as_deref()
    }

    /// Keeps a new request (an empty id gets a fresh one) and answers it as stored.
    pub fn create(&self, mut request: FamilyRequest, now: DateTime<Utc>) -> FamilyRequest {
        if request.id.trim().is_empty() {
            request.id = new_request_id();
        }
        {
            let mut requests = self.requests.lock();
            requests.push(request.clone());
            prune(&mut requests, now);
        }
        self.save();
        request
    }

    pub fn get(&self, id: &str) -> Option<FamilyRequest> {
        self.requests.lock().iter().find(|r| r.id == id).cloned()
    }

    /// One member's requests, oldest first (names compared ignoring case).
    pub fn for_user(&self, name: &str) -> Vec<FamilyRequest> {
        self.requests
            .lock()
            .iter()
            .filter(|r| eq_ignore_case(&r.username, name))
            .cloned()
            .collect()
    }

    /// Every request waiting for an approver, oldest first.
    pub fn pending(&self) -> Vec<FamilyRequest> {
        self.requests
            .lock()
            .iter()
            .filter(|r| r.state == RequestState::Pending)
            .cloned()
            .collect()
    }

    pub fn all(&self) -> Vec<FamilyRequest> {
        self.requests.lock().clone()
    }

    /// Keeps a new request unless one `same` answers true for is kept already, in one step, so
    /// two identical asks at the same moment make one request. Answers the request kept and
    /// whether it is the new one.
    pub fn create_unless(
        &self,
        mut request: FamilyRequest,
        now: DateTime<Utc>,
        same: impl Fn(&FamilyRequest) -> bool,
    ) -> (FamilyRequest, bool) {
        if request.id.trim().is_empty() {
            request.id = new_request_id();
        }
        {
            let mut requests = self.requests.lock();
            if let Some(existing) = requests.iter().find(|r| same(r)) {
                return (existing.clone(), false);
            }
            requests.push(request.clone());
            prune(&mut requests, now);
        }
        self.save();
        (request, true)
    }

    /// Changes one request and answers it as changed; None when there is no such request.
    pub fn update(&self, id: &str, change: impl FnOnce(&mut FamilyRequest)) -> Option<FamilyRequest> {
        let changed = {
            let mut requests = self.requests.lock();
            let request = requests.iter_mut().find(|r| r.id == id)?;
            change(request);
            request.clone()
        };
        self.save();
        Some(changed)
    }

    /// Changes one request only while `allowed` holds for it, checked and changed in one step,
    /// so two callers never both see a waiting request and both change it. Err(None) when there
    /// is no such request, Err(Some(it)) when `allowed` refused it as it is.
    pub fn update_if(
        &self,
        id: &str,
        allowed: impl FnOnce(&FamilyRequest) -> bool,
        change: impl FnOnce(&mut FamilyRequest),
    ) -> Result<FamilyRequest, Option<Box<FamilyRequest>>> {
        let changed = {
            let mut requests = self.requests.lock();
            let request = requests.iter_mut().find(|r| r.id == id).ok_or(None)?;
            if !allowed(request) {
                return Err(Some(Box::new(request.clone())));
            }
            change(request);
            request.clone()
        };
        self.save();
        Ok(changed)
    }

    /// Cancels every waiting request of `name` (names compared ignoring case) and answers them.
    pub fn cancel_pending_of(&self, name: &str) -> Vec<FamilyRequest> {
        let canceled: Vec<FamilyRequest> = {
            let mut requests = self.requests.lock();
            requests
                .iter_mut()
                .filter(|r| r.state == RequestState::Pending && eq_ignore_case(&r.username, name))
                .map(|r| {
                    r.state = RequestState::Cancelled;
                    r.clone()
                })
                .collect()
        };
        if !canceled.is_empty() {
            self.save();
        }
        canceled
    }

    /// The member's weekly slots in use at `now`.
    pub fn quota_used(&self, name: &str, now: DateTime<Utc>) -> u32 {
        quota_used(&self.requests.lock(), name, now)
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let _saving = self.save_lock.lock();
        let requests = self.requests.lock().clone();
        store_file::save(Some(path), &requests);
    }
}

/// Drops finished requests older than [`KEEP_DAYS`], then the oldest finished ones beyond
/// [`KEEP_COUNT`].
fn prune(requests: &mut Vec<FamilyRequest>, now: DateTime<Utc>) {
    let cutoff = now - TimeDelta::days(KEEP_DAYS);
    requests.retain(|r| is_live(r) || r.created > cutoff);
    let mut extra = requests.len().saturating_sub(KEEP_COUNT);
    if extra == 0 {
        return;
    }
    // Requests are kept in the order they were made, so the first finished ones are the oldest.
    requests.retain(|r| {
        if extra > 0 && !is_live(r) {
            extra -= 1;
            false
        } else {
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use octo_core::family::abilities::RequestQuality;
    use octo_core::family::request::{RequestKind, RequestOutcome};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 20, 18, 0, 0).unwrap()
    }

    fn request(user: &str, state: RequestState, created: DateTime<Utc>) -> FamilyRequest {
        FamilyRequest {
            id: String::new(),
            username: user.to_string(),
            kind: RequestKind::Song,
            target: "ext-deezer-song-1".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            cover_art: String::new(),
            quality: RequestQuality::Best,
            state,
            created,
            decided_by: None,
            decided: None,
            note: String::new(),
            library_song_id: None,
            failure: None,
            outcome: None,
        }
    }

    #[test]
    fn create_get_and_list() {
        let store = RequestStore::in_memory();
        let made = store.create(request("alex", RequestState::Pending, now()), now());
        assert!(made.id.starts_with("r_") && made.id.len() == 14, "{}", made.id);
        store.create(request("sam", RequestState::Done, now()), now());
        assert_eq!(store.get(&made.id), Some(made.clone()));
        assert_eq!(store.for_user("ALEX").len(), 1);
        assert_eq!(store.pending(), vec![made]);
        assert_eq!(store.all().len(), 2);
        assert_eq!(store.quota_used("alex", now()), 1);
    }

    #[test]
    fn update_changes_one_request_and_survives_a_restart() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("family-requests.json");
        let store = RequestStore::new(Some(&path));
        let made = store.create(request("alex", RequestState::Pending, now()), now());
        let done = store
            .update(&made.id, |r| {
                r.state = RequestState::Done;
                r.outcome = Some(RequestOutcome::Downloaded);
                r.decided_by = Some("winters".into());
            })
            .expect("found");
        assert_eq!(done.state, RequestState::Done);
        assert!(store.update("r_missing", |_| {}).is_none());
        let reloaded = RequestStore::new(Some(&path));
        assert_eq!(reloaded.get(&made.id), Some(done));
    }

    #[test]
    fn finished_requests_go_after_180_days_live_ones_stay() {
        let store = RequestStore::in_memory();
        let old = now() - TimeDelta::days(181);
        store.create(request("alex", RequestState::Done, old), old);
        store.create(request("alex", RequestState::Pending, old), old);
        store.create(request("alex", RequestState::Declined, now()), now());
        let states: Vec<RequestState> = store.all().iter().map(|r| r.state).collect();
        assert_eq!(states, [RequestState::Pending, RequestState::Declined]);
    }

    #[test]
    fn at_most_two_thousand_are_kept() {
        let store = RequestStore::in_memory();
        let live = store.create(request("alex", RequestState::Approved, now()), now());
        for n in 0..KEEP_COUNT + 10 {
            let at = now() + TimeDelta::seconds(n as i64);
            store.create(request("alex", RequestState::Done, at), at);
        }
        let all = store.all();
        assert_eq!(all.len(), KEEP_COUNT);
        assert_eq!(all[0].id, live.id, "a live request is never dropped");
    }

    #[test]
    fn two_callers_racing_change_a_waiting_request_once() {
        let store = std::sync::Arc::new(RequestStore::in_memory());
        for _ in 0..50 {
            let made = store.create(request("alex", RequestState::Pending, now()), now());
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let racers: Vec<_> = (0..2)
                .map(|n| {
                    let (store, barrier, id) = (store.clone(), barrier.clone(), made.id.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        store
                            .update_if(
                                &id,
                                |r| r.state == RequestState::Pending,
                                |r| {
                                    r.state = RequestState::Approved;
                                    r.decided_by = Some(format!("approver-{n}"));
                                },
                            )
                            .is_ok()
                    })
                })
                .collect();
            let won: usize = racers
                .into_iter()
                .map(|racer| usize::from(racer.join().expect("joins")))
                .sum();
            assert_eq!(won, 1, "exactly one approver decides");
        }
        assert!(matches!(
            store.update_if("r_missing", |_| true, |_| {}),
            Err(None)
        ));
    }

    #[test]
    fn the_same_ask_twice_at_once_is_one_request() {
        let store = std::sync::Arc::new(RequestStore::in_memory());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let askers: Vec<_> = (0..4)
            .map(|_| {
                let (store, barrier) = (store.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .create_unless(request("alex", RequestState::Pending, now()), now(), |r| {
                            eq_ignore_case(&r.username, "alex") && is_live(r)
                        })
                        .1
                })
            })
            .collect();
        let made = askers
            .into_iter()
            .map(|asker| asker.join().expect("joins"))
            .filter(|new| *new)
            .count();
        assert_eq!(made, 1);
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn a_removed_members_waiting_requests_are_canceled() {
        let store = RequestStore::in_memory();
        store.create(request("alex", RequestState::Pending, now()), now());
        store.create(request("ALEX", RequestState::Pending, now()), now());
        store.create(request("alex", RequestState::Done, now()), now());
        store.create(request("sam", RequestState::Pending, now()), now());
        assert_eq!(store.cancel_pending_of("alex").len(), 2);
        let states: Vec<(String, RequestState)> =
            store.all().into_iter().map(|r| (r.username, r.state)).collect();
        assert_eq!(
            states,
            [
                ("alex".to_string(), RequestState::Cancelled),
                ("ALEX".to_string(), RequestState::Cancelled),
                ("alex".to_string(), RequestState::Done),
                ("sam".to_string(), RequestState::Pending),
            ]
        );
    }

    #[test]
    fn a_corrupt_file_is_kept_aside() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("family-requests.json");
        std::fs::write(&path, "[{").expect("writes");
        let store = RequestStore::new(Some(&path));
        assert!(store.load_warning().is_some());
        assert!(store.all().is_empty());
    }
}
