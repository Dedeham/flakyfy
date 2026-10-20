//! Which songs play after the one playing now: the window of songs fetched ahead.
//!
//! The play order is known for sure in two cases only. A player that saves its play queue says
//! the order outright, shuffle and skips included. A radio list is the order only when the
//! listener's last two songs are next to each other in it: an app that mixes a station's songs
//! on the phone uses the list as a pool, and its next songs are not the list's.

use chrono::{DateTime, Utc};
use octo_core::settings::PrefetchScope;

/// A play queue as a player saved it (savePlayQueue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedQueue {
    /// The songs in the order they play.
    pub ids: Vec<String>,
    /// The song the player said was current, when it said.
    pub current: Option<String>,
    /// Its place in `ids`, when the player gave one.
    pub current_index: Option<usize>,
    /// When it was saved.
    pub at: DateTime<Utc>,
}

/// The next `count` songs after `last_two.1`, the one playing now. `last_two.0` is the song the
/// same listener played before it. From the saved queue when it holds the playing song, else
/// from the newest radio list where the two are next to each other, else nothing. With
/// [`PrefetchScope::RadioOnly`] only the songs `is_radio` says a radio offered are kept. The
/// playing song is never in the window, and no song is in it twice.
pub fn window(
    saved: Option<&SavedQueue>,
    radio_lists: &[Vec<String>],
    last_two: (Option<&str>, &str),
    count: usize,
    scope: PrefetchScope,
    is_radio: impl Fn(&str) -> bool,
) -> Vec<String> {
    let (previous, playing) = last_two;
    if playing.is_empty() || count == 0 {
        return Vec::new();
    }
    let after: &[String] = match saved.and_then(|queue| place_in_saved(queue, playing)) {
        Some((queue, at)) => &queue.ids[at + 1..],
        None => match previous.and_then(|previous| in_order(radio_lists, previous, playing)) {
            Some((list, at)) => &list[at + 1..],
            None => &[],
        },
    };
    let mut picked: Vec<String> = Vec::new();
    for id in after {
        if picked.len() >= count {
            break;
        }
        if id.is_empty() || id == playing || picked.iter().any(|p| p == id) {
            continue;
        }
        if scope == PrefetchScope::RadioOnly && !is_radio(id) {
            continue;
        }
        picked.push(id.clone());
    }
    picked
}

/// Where the playing song is in the saved queue: at the place the player called current when
/// that is the song, else its first place.
fn place_in_saved<'a>(queue: &'a SavedQueue, playing: &str) -> Option<(&'a SavedQueue, usize)> {
    if let Some(at) = queue
        .current_index
        .filter(|at| queue.ids.get(*at).is_some_and(|id| id == playing))
    {
        return Some((queue, at));
    }
    queue
        .ids
        .iter()
        .position(|id| id == playing)
        .map(|at| (queue, at))
}

/// The newest list in which `previous` comes right before `playing`, and the playing song's place.
fn in_order<'a>(lists: &'a [Vec<String>], previous: &str, playing: &str) -> Option<(&'a [String], usize)> {
    if previous.is_empty() || previous == playing {
        return None;
    }
    lists.iter().find_map(|list| {
        list.windows(2)
            .position(|pair| pair[0] == previous && pair[1] == playing)
            .map(|at| (list.as_slice(), at + 1))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn saved(names: &[&str], current: Option<usize>) -> SavedQueue {
        SavedQueue {
            ids: ids(names),
            current: current.map(|at| names[at].to_string()),
            current_index: current,
            at: Utc::now(),
        }
    }

    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn the_saved_queue_gives_the_next_songs_after_the_playing_one() {
        let queue = saved(&["a", "b", "c", "d", "e"], Some(0));
        let next = window(Some(&queue), &[], (None, "a"), 3, PrefetchScope::All, all);
        assert_eq!(next, ids(&["b", "c", "d"]));
    }

    #[test]
    fn a_skip_moves_the_window_with_the_song_now_playing() {
        // The player saved the queue while "a" was current, then the listener skipped to "c".
        let queue = saved(&["a", "b", "c", "d", "e", "f"], Some(0));
        let next = window(Some(&queue), &[], (Some("a"), "c"), 2, PrefetchScope::All, all);
        assert_eq!(next, ids(&["d", "e"]));
    }

    #[test]
    fn shuffle_follows_the_order_the_player_saved() {
        let queue = saved(&["d", "a", "e", "b", "c"], Some(1));
        let next = window(Some(&queue), &[], (Some("d"), "a"), 3, PrefetchScope::All, all);
        assert_eq!(next, ids(&["e", "b", "c"]));
    }

    #[test]
    fn a_song_removed_from_the_queue_is_not_fetched() {
        let before = saved(&["a", "b", "c", "d"], Some(0));
        assert_eq!(
            window(Some(&before), &[], (None, "a"), 2, PrefetchScope::All, all),
            ids(&["b", "c"])
        );
        let after = saved(&["a", "c", "d"], Some(0));
        assert_eq!(
            window(Some(&after), &[], (None, "a"), 2, PrefetchScope::All, all),
            ids(&["c", "d"])
        );
    }

    #[test]
    fn the_end_of_the_queue_gives_what_is_left() {
        let queue = saved(&["a", "b", "c"], Some(1));
        assert_eq!(
            window(Some(&queue), &[], (Some("a"), "b"), 5, PrefetchScope::All, all),
            ids(&["c"])
        );
        assert!(window(Some(&queue), &[], (Some("b"), "c"), 5, PrefetchScope::All, all).is_empty());
    }

    #[test]
    fn the_current_place_decides_between_two_copies_of_a_song() {
        let queue = saved(&["a", "b", "x", "a", "c"], Some(3));
        assert_eq!(
            window(Some(&queue), &[], (None, "a"), 1, PrefetchScope::All, all),
            ids(&["c"])
        );
    }

    #[test]
    fn a_radio_list_counts_only_when_the_last_two_plays_are_next_to_each_other_in_it() {
        let lists = vec![ids(&["r1", "r2", "r3", "r4", "r5"])];
        assert_eq!(
            window(None, &lists, (Some("r2"), "r3"), 2, PrefetchScope::All, all),
            ids(&["r4", "r5"])
        );
        // An app that mixes the station on the phone plays it out of order: a pool, not the order.
        assert!(window(None, &lists, (Some("r1"), "r4"), 2, PrefetchScope::All, all).is_empty());
        // One play is no proof of order.
        assert!(window(None, &lists, (None, "r3"), 2, PrefetchScope::All, all).is_empty());
    }

    #[test]
    fn the_newest_list_holding_the_pair_wins() {
        let lists = vec![ids(&["x", "a", "b", "new"]), ids(&["a", "b", "old"])];
        assert_eq!(
            window(None, &lists, (Some("a"), "b"), 1, PrefetchScope::All, all),
            ids(&["new"])
        );
    }

    #[test]
    fn the_saved_queue_goes_before_a_radio_list() {
        let queue = saved(&["a", "b", "q"], Some(0));
        let lists = vec![ids(&["a", "b", "r"])];
        assert_eq!(
            window(Some(&queue), &lists, (Some("a"), "b"), 1, PrefetchScope::All, all),
            ids(&["q"])
        );
        // A saved queue without the playing song leaves it to the radio list.
        let other = saved(&["x", "y"], Some(0));
        assert_eq!(
            window(Some(&other), &lists, (Some("a"), "b"), 1, PrefetchScope::All, all),
            ids(&["r"])
        );
    }

    #[test]
    fn radio_only_keeps_the_songs_a_radio_offered() {
        let queue = saved(&["a", "r1", "s1", "r2", "s2", "r3"], Some(0));
        let is_radio = |id: &str| id.starts_with('r');
        assert_eq!(
            window(
                Some(&queue),
                &[],
                (None, "a"),
                2,
                PrefetchScope::RadioOnly,
                is_radio
            ),
            ids(&["r1", "r2"])
        );
        assert_eq!(
            window(Some(&queue), &[], (None, "a"), 2, PrefetchScope::All, is_radio),
            ids(&["r1", "s1"])
        );
    }

    #[test]
    fn nothing_twice_and_never_the_playing_song() {
        let queue = saved(&["a", "b", "a", "b", "c"], Some(0));
        assert_eq!(
            window(Some(&queue), &[], (None, "a"), 3, PrefetchScope::All, all),
            ids(&["b", "c"])
        );
    }
}
