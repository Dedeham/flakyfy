//! Which bytes one play of an outside song gets: the ready FLAC or the preview. A play keeps the
//! source it started on. Octo's apps keep what they streamed under the song's id, and a song
//! partly heard as the preview is later played on from where that stopped (`Range: bytes=N-`):
//! FLAC bytes there would follow the preview's in one file. So a request from the first byte
//! decides, and every later range of that play follows it.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use octo_core::common::Clock;
use parking_lot::Mutex;

/// How long a play's source is kept beyond the song's own length.
const GRACE: Duration = Duration::minutes(10);
/// The length assumed for a song whose length is not known.
const UNKNOWN_LENGTH: Duration = Duration::minutes(10);
/// Plays remembered at most; the oldest go first.
const MAX_PLAYS: usize = 10_000;

/// The source of one play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The ready FLAC.
    File,
    /// The preview from the song's source.
    Preview,
}

/// The source each listener's play of each song started on, for a while.
pub struct PlaySources {
    clock: Clock,
    plays: Mutex<HashMap<(String, String), (Source, DateTime<Utc>)>>,
}

impl PlaySources {
    pub fn new(clock: Clock) -> Self {
        PlaySources {
            clock,
            plays: Mutex::new(HashMap::new()),
        }
    }

    /// The source for one request of `who` (the listener, or the address and app) for `id`. A
    /// request from the first byte gets the file when one is ready, and the preview otherwise; a
    /// later range gets the file only when its play started on it. With `remember`, a request from
    /// the first byte starts a play that keeps its source for the song's length and ten minutes.
    pub fn choose(
        &self,
        who: &str,
        id: &str,
        from_start: bool,
        file_ready: bool,
        length_seconds: Option<i32>,
        remember: bool,
    ) -> Source {
        let now = self.clock.now();
        let key = (who.to_string(), id.to_string());
        let mut plays = self.plays.lock();
        if from_start {
            let source = if file_ready { Source::File } else { Source::Preview };
            if remember {
                let length = length_seconds
                    .filter(|s| *s > 0)
                    .map_or(UNKNOWN_LENGTH, |s| Duration::seconds(i64::from(s)));
                if plays.len() >= MAX_PLAYS {
                    plays.retain(|_, (_, until)| *until > now);
                    if plays.len() >= MAX_PLAYS
                        && let Some(oldest) = plays
                            .iter()
                            .min_by_key(|(_, (_, until))| *until)
                            .map(|(key, _)| key.clone())
                    {
                        plays.remove(&oldest);
                    }
                }
                plays.insert(key, (source, now + length + GRACE));
            }
            return source;
        }
        match plays.get(&key) {
            Some((Source::File, until)) if *until > now && file_ready => Source::File,
            Some((_, until)) if *until <= now => {
                plays.remove(&key);
                Source::Preview
            }
            _ => Source::Preview,
        }
    }
}

/// Whether a Range header asks from the first byte: none, blank, or `bytes=0-`.
pub fn from_start(range: Option<&str>) -> bool {
    match range.map(str::trim) {
        None | Some("") => true,
        Some(range) => range
            .get(..8)
            .is_some_and(|start| start.eq_ignore_ascii_case("bytes=0-")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sources() -> (PlaySources, Arc<Mutex<DateTime<Utc>>>) {
        let now = Arc::new(Mutex::new(Utc::now()));
        let clock_now = Arc::clone(&now);
        (PlaySources::new(Clock::new(move || *clock_now.lock())), now)
    }

    #[test]
    fn a_play_from_the_start_takes_the_file_when_one_is_ready() {
        let (plays, _) = sources();
        assert_eq!(
            plays.choose("alice", "s1", true, true, Some(200), true),
            Source::File
        );
        assert_eq!(
            plays.choose("alice", "s2", true, false, Some(200), true),
            Source::Preview
        );
    }

    #[test]
    fn a_seek_after_the_file_lands_stays_on_the_preview() {
        let (plays, _) = sources();
        assert_eq!(
            plays.choose("alice", "s1", true, false, Some(200), true),
            Source::Preview
        );
        // The file is ready now; the play that started on the preview keeps it.
        assert_eq!(
            plays.choose("alice", "s1", false, true, Some(200), true),
            Source::Preview
        );
    }

    #[test]
    fn a_later_range_with_no_play_started_on_the_file_gets_the_preview() {
        let (plays, _) = sources();
        assert_eq!(
            plays.choose("alice", "s1", false, true, Some(200), true),
            Source::Preview
        );
    }

    #[test]
    fn a_play_that_started_on_the_file_keeps_it_for_its_later_ranges() {
        let (plays, now) = sources();
        assert_eq!(
            plays.choose("alice", "s1", true, true, Some(200), true),
            Source::File
        );
        *now.lock() += Duration::seconds(150);
        assert_eq!(
            plays.choose("alice", "s1", false, true, Some(200), true),
            Source::File
        );
        // Another listener's play is their own.
        assert_eq!(
            plays.choose("bob", "s1", false, true, Some(200), true),
            Source::Preview
        );
    }

    #[test]
    fn a_play_keeps_its_source_for_the_song_s_length_and_ten_minutes() {
        let (plays, now) = sources();
        plays.choose("alice", "s1", true, true, Some(60), true);
        *now.lock() += Duration::seconds(60) + Duration::minutes(9);
        assert_eq!(
            plays.choose("alice", "s1", false, true, Some(60), true),
            Source::File
        );
        *now.lock() += Duration::minutes(2);
        assert_eq!(
            plays.choose("alice", "s1", false, true, Some(60), true),
            Source::Preview
        );
    }

    #[test]
    fn a_file_gone_since_the_play_started_gives_the_preview() {
        let (plays, _) = sources();
        plays.choose("alice", "s1", true, true, Some(200), true);
        assert_eq!(
            plays.choose("alice", "s1", false, false, Some(200), true),
            Source::Preview
        );
    }

    #[test]
    fn a_new_play_from_the_start_decides_again() {
        let (plays, _) = sources();
        plays.choose("alice", "s1", true, false, Some(200), true);
        assert_eq!(
            plays.choose("alice", "s1", true, true, Some(200), true),
            Source::File
        );
        assert_eq!(
            plays.choose("alice", "s1", false, true, Some(200), true),
            Source::File
        );
    }

    #[test]
    fn a_look_that_is_not_a_play_remembers_nothing() {
        let (plays, _) = sources();
        assert_eq!(
            plays.choose("alice", "s1", true, true, Some(200), false),
            Source::File
        );
        assert_eq!(
            plays.choose("alice", "s1", false, true, Some(200), true),
            Source::Preview
        );
    }

    #[test]
    fn ranges_from_the_first_byte_are_told_apart() {
        for (range, start) in [
            (None, true),
            (Some(""), true),
            (Some("bytes=0-"), true),
            (Some("BYTES=0-1023"), true),
            (Some("bytes=500000-"), false),
            (Some("bytes=-500"), false),
        ] {
            assert_eq!(from_start(range), start, "{range:?}");
        }
    }
}
