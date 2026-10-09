//! Badged covers of songs found outside the library, kept for an hour so a list scrolling past
//! the same songs again does not fetch and stamp each picture again. Bounded by count and by
//! bytes; the oldest go first.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// How long a badged cover is served from memory.
pub const KEEP_FOR: Duration = Duration::from_secs(60 * 60);
/// The most badged covers kept.
pub const MAX_ENTRIES: usize = 2000;
/// The most bytes kept, all covers together.
pub const MAX_BYTES: usize = 128 * 1024 * 1024;

struct Kept {
    bytes: Arc<Vec<u8>>,
    until: Instant,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Kept>,
    /// Keys oldest first.
    order: VecDeque<String>,
    bytes: usize,
}

/// The badged covers, by key.
pub struct BadgedCovers {
    max_entries: usize,
    max_bytes: usize,
    state: Mutex<State>,
}

impl Default for BadgedCovers {
    fn default() -> Self {
        BadgedCovers::new(MAX_ENTRIES, MAX_BYTES)
    }
}

impl BadgedCovers {
    pub fn new(max_entries: usize, max_bytes: usize) -> BadgedCovers {
        BadgedCovers {
            max_entries,
            max_bytes,
            state: Mutex::new(State::default()),
        }
    }

    /// The cover kept under `key`, while it is fresh at `now`.
    pub fn get(&self, key: &str, now: Instant) -> Option<Arc<Vec<u8>>> {
        let mut state = self.state.lock();
        let fresh = state.entries.get(key).map(|kept| kept.until > now)?;
        if fresh {
            return state.entries.get(key).map(|kept| Arc::clone(&kept.bytes));
        }
        remove(&mut state, key);
        None
    }

    /// Keeps a cover under `key` from `now`. One larger than the whole budget is not kept.
    pub fn put(&self, key: String, bytes: Arc<Vec<u8>>, now: Instant) {
        if bytes.len() > self.max_bytes || self.max_entries == 0 {
            return;
        }
        let mut state = self.state.lock();
        remove(&mut state, &key);
        // Expired covers go first, then the oldest, until the new one fits.
        let expired: Vec<String> = state
            .entries
            .iter()
            .filter(|(_, kept)| kept.until <= now)
            .map(|(key, _)| key.clone())
            .collect();
        for old in expired {
            remove(&mut state, &old);
        }
        while state.entries.len() >= self.max_entries || state.bytes + bytes.len() > self.max_bytes {
            let Some(oldest) = state.order.front().cloned() else {
                break;
            };
            remove(&mut state, &oldest);
        }
        state.bytes += bytes.len();
        state.order.push_back(key.clone());
        state.entries.insert(
            key,
            Kept {
                bytes,
                until: now + KEEP_FOR,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.state.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn remove(state: &mut State, key: &str) {
    if let Some(kept) = state.entries.remove(key) {
        state.bytes -= kept.bytes.len();
        state.order.retain(|k| k != key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cover(size: usize) -> Arc<Vec<u8>> {
        Arc::new(vec![7; size])
    }

    #[test]
    fn a_kept_cover_is_served_for_an_hour_and_then_not() {
        let covers = BadgedCovers::default();
        let now = Instant::now();
        covers.put("a".into(), cover(10), now);

        assert_eq!(
            covers.get("a", now + Duration::from_secs(59 * 60)),
            Some(cover(10))
        );
        assert_eq!(covers.get("a", now + KEEP_FOR), None);
        assert!(covers.is_empty());
    }

    #[test]
    fn the_oldest_go_first_when_the_count_is_reached() {
        let covers = BadgedCovers::new(2, 1000);
        let now = Instant::now();
        covers.put("a".into(), cover(1), now);
        covers.put("b".into(), cover(1), now);
        covers.put("c".into(), cover(1), now);

        assert_eq!(covers.len(), 2);
        assert_eq!(covers.get("a", now), None);
        assert!(covers.get("b", now).is_some() && covers.get("c", now).is_some());
    }

    #[test]
    fn the_oldest_go_first_when_the_bytes_are_reached_and_a_huge_one_is_not_kept() {
        let covers = BadgedCovers::new(100, 10);
        let now = Instant::now();
        covers.put("a".into(), cover(6), now);
        covers.put("b".into(), cover(6), now);
        assert_eq!(covers.get("a", now), None);
        assert!(covers.get("b", now).is_some());

        covers.put("huge".into(), cover(11), now);
        assert_eq!(covers.get("huge", now), None);
        assert!(covers.get("b", now).is_some());
    }
}
