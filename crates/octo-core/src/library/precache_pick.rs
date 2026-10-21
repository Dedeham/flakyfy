//! The pure half of the playlist pre-cache: which outside songs of a list to fetch ahead of a
//! play, and when a fetched song that nobody took up is let go.

use chrono::{DateTime, TimeDelta, Utc};

/// An outside song of a list that is not in the library yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub provider: String,
    pub external_id: String,
    pub artist: String,
    pub title: String,
}

impl Candidate {
    /// `provider:id`, the key the acquisition queue and the history use for a song.
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider, self.external_id)
    }
}

/// Up to `count` of `candidates` that `skip` does not refuse, in a random order. `below(n)` gives
/// a number in `0..n`, so a test can fix the draw. A song is drawn at random rather than from the
/// top: a shuffle then starts on a song that is already here more often than not, while later
/// rounds, which skip what is here, work through the rest.
pub fn pick(
    candidates: Vec<Candidate>,
    count: usize,
    skip: impl Fn(&Candidate) -> bool,
    mut below: impl FnMut(usize) -> usize,
) -> Vec<Candidate> {
    let mut pool: Vec<Candidate> = candidates.into_iter().filter(|c| !skip(c)).collect();
    // Fisher-Yates from the back, stopping once `count` are settled.
    let settled = count.min(pool.len());
    for i in 0..settled {
        let last = pool.len() - 1 - i;
        let chosen = below(last + 1).min(last);
        pool.swap(last, chosen);
    }
    pool.split_off(pool.len() - settled)
}

/// What anyone did with a song that was fetched ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Uptake {
    /// Someone starred or rated it. None: not every listener's marks could be read.
    pub marked: Option<bool>,
    /// The plays Navidrome counts; None when they could not be read.
    pub plays: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    /// Not old enough to look at yet.
    NotYet,
    /// Someone took it up (or that cannot be told): it is an ordinary library song from now on.
    Keep(&'static str),
    /// Nobody did: it goes to the trash.
    Let,
}

/// Whether a song fetched at `cached_at` is let go at `now`. Anything unknown keeps it: a song
/// is only ever trashed when every listener's marks were read and none is there.
pub fn expiry(
    cached_at: DateTime<Utc>,
    now: DateTime<Utc>,
    keep_days: i32,
    min_plays: i32,
    uptake: Uptake,
) -> Expiry {
    if now - cached_at < TimeDelta::days(i64::from(keep_days)) {
        return Expiry::NotYet;
    }
    match uptake.marked {
        Some(true) => return Expiry::Keep("starred or rated"),
        None => return Expiry::Keep("not every listener's marks could be read"),
        Some(false) => {}
    }
    match uptake.plays {
        Some(plays) if plays >= i64::from(min_plays) => Expiry::Keep("played"),
        Some(_) => Expiry::Let,
        None => Expiry::Keep("its plays could not be read"),
    }
}

#[cfg(test)]
#[path = "precache_pick_tests.rs"]
mod tests;
