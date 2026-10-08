//! Where a radio's songs from outside the library land. The same rule as the Octo apps'
//! radio (`RadioMix.kt`, `outsideDue`): a draw at the share, so outside songs come at random
//! instead of on a schedule, pulled back once the radio is more than [`SHARE_SLACK`] songs
//! off its share, and never more than [`RARER_RUN`] of the rarer kind in a row.

use super::last_fm_radio_recommendation_service::RadioRandom;
use super::last_fm_radio_spacing::{artist_key, played};

/// How far a radio may drift from its share, in songs, before the next pick puts it right.
pub const SHARE_SLACK: f64 = 1.5;
/// The most songs of the rarer kind in a row.
pub const RARER_RUN: i32 = 2;

/// Whether the next song should be one from outside the library. `run` counts the songs in a
/// row just before: above 0 outside, below 0 library.
pub fn outside_due(
    share: f64,
    picked: usize,
    outside: usize,
    run: i32,
    random: &mut dyn RadioRandom,
) -> bool {
    if share <= 0.0 {
        return false;
    }
    if share >= 1.0 {
        return true;
    }
    let target = share * (picked as f64 + 1.0);
    if target - outside as f64 > SHARE_SLACK {
        return true;
    }
    if outside as f64 + 1.0 - target > SHARE_SLACK {
        return false;
    }
    if share < 0.5 && run >= RARER_RUN {
        return false;
    }
    if share > 0.5 && run <= -RARER_RUN {
        return true;
    }
    random.next_double() < share
}

/// The songs, each kind (`is_outside`) in its own order, interleaved by [`outside_due`] at
/// `share`. Within the due kind a song waits while its artist played in the last
/// [`super::last_fm_radio_spacing::GAP`] songs, counting the seed, as `spread` has it, unless
/// every song of that kind would wait. Nothing is dropped: when one kind runs out the other
/// plays on.
pub fn place<T, O, A>(
    songs: &[T],
    is_outside: O,
    artist_of: A,
    seed_artist: Option<&str>,
    share: f64,
    random: &mut dyn RadioRandom,
) -> Vec<T>
where
    T: Clone,
    O: Fn(&T) -> bool,
    A: Fn(&T) -> Option<String>,
{
    let (mut outside, mut library): (Vec<T>, Vec<T>) =
        songs.iter().cloned().partition(|song| is_outside(song));
    let mut recent: Vec<String> = Vec::new();
    played(&mut recent, seed_artist);
    let mut placed = Vec::with_capacity(songs.len());
    let (mut taken_outside, mut run) = (0usize, 0i32);
    while !outside.is_empty() || !library.is_empty() {
        let want_outside = if outside.is_empty() {
            false
        } else if library.is_empty() {
            true
        } else {
            outside_due(share, placed.len(), taken_outside, run, random)
        };
        let pool = if want_outside { &mut outside } else { &mut library };
        let next = pool
            .iter()
            .position(|song| !recent.contains(&artist_key(artist_of(song).as_deref())))
            .unwrap_or(0);
        let song = pool.remove(next);
        played(&mut recent, artist_of(&song).as_deref());
        if want_outside {
            taken_outside += 1;
            run = run.max(0) + 1;
        } else {
            run = run.min(0) - 1;
        }
        placed.push(song);
    }
    placed
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::common::dotnet_random::DotnetRandom;

    /// A draw that always gives the same number, so a rule, not the draw, decides.
    struct Fixed(f64);

    impl RadioRandom for Fixed {
        fn next_double(&mut self) -> f64 {
            self.0
        }
    }

    /// `(id, outside)`, each song by an artist of its own, so no artist ever waits.
    fn songs(library: usize, outside: usize) -> Vec<(usize, bool)> {
        (0..library)
            .map(|id| (id, false))
            .chain((library..library + outside).map(|id| (id, true)))
            .collect()
    }

    fn placed(songs: &[(usize, bool)], share: f64, seed: i32) -> Vec<(usize, bool)> {
        place(
            songs,
            |(_, outside)| *outside,
            |(id, _)| Some(format!("Artist {id}")),
            None,
            share,
            &mut DotnetRandom::new(seed),
        )
    }

    /// O for a song from outside the library, L for one from it.
    fn pattern(songs: &[(usize, bool)]) -> String {
        songs
            .iter()
            .map(|(_, outside)| if *outside { 'O' } else { 'L' })
            .collect()
    }

    /// The first 30 of each of 200 radios, each placed from 60 songs of each kind.
    fn radios() -> Vec<String> {
        let pool = songs(60, 60);
        (0..200)
            .map(|seed| pattern(&placed(&pool, 0.35, seed)[..30]))
            .collect()
    }

    #[test]
    fn outside_songs_land_at_random_not_on_a_schedule() {
        let radios = radios();
        let openings: HashSet<&str> = radios.iter().map(|radio| &radio[..9]).collect();
        assert!(openings.len() >= 20, "only {} different openings", openings.len());
        let scheduled = radios
            .iter()
            .filter(|radio| radio.starts_with("LLOLLOLLO"))
            .count();
        assert!(
            scheduled < 10,
            "{scheduled} of 200 radios open on the old schedule"
        );

        // The share holds in each radio and across them.
        let mut outside_total = 0;
        for radio in &radios {
            let outside = radio.chars().filter(|kind| *kind == 'O').count();
            assert!((8..=13).contains(&outside), "{outside} outside songs in {radio}");
            outside_total += outside;
        }
        let share = outside_total as f64 / (radios.len() * 30) as f64;
        assert!((0.33..=0.37).contains(&share), "outside share {share}");

        // Under half the radio, outside songs never come three in a row.
        for radio in &radios {
            assert!(!radio.contains("OOO"), "three outside songs in a row in {radio}");
        }
    }

    #[test]
    fn outside_due_follows_its_rules() {
        assert!(
            !outside_due(0.0, 3, 0, 0, &mut Fixed(0.0)),
            "share 0 never plays an outside song"
        );
        assert!(
            outside_due(1.0, 3, 3, 3, &mut Fixed(0.99)),
            "share 1 always plays one"
        );
        assert!(
            outside_due(0.35, 4, 0, -3, &mut Fixed(0.99)),
            "more than SHARE_SLACK short puts it right"
        );
        assert!(
            !outside_due(0.35, 2, 2, 2, &mut Fixed(0.0)),
            "more than SHARE_SLACK over holds it back"
        );
        assert!(
            !outside_due(0.35, 5, 2, 2, &mut Fixed(0.0)),
            "two outside in a row is the most under half"
        );
        assert!(
            outside_due(0.85, 13, 11, -2, &mut Fixed(0.99)),
            "two library in a row is the most over half"
        );
    }

    #[test]
    fn nothing_is_dropped_and_each_kind_keeps_its_order() {
        let pool = songs(7, 20);
        for seed in 0..20 {
            let placed = placed(&pool, 0.35, seed);
            let mut ids: Vec<usize> = placed.iter().map(|(id, _)| *id).collect();
            let library: Vec<usize> = placed.iter().filter(|(_, o)| !o).map(|(id, _)| *id).collect();
            let outside: Vec<usize> = placed.iter().filter(|(_, o)| *o).map(|(id, _)| *id).collect();
            assert_eq!(library, (0..7).collect::<Vec<_>>(), "seed {seed}");
            assert_eq!(outside, (7..27).collect::<Vec<_>>(), "seed {seed}");
            ids.sort_unstable();
            assert_eq!(ids, (0..27).collect::<Vec<_>>(), "seed {seed}");
        }
    }

    #[test]
    fn an_artist_waits_when_another_song_of_the_kind_can_play() {
        let songs = ["A", "A", "B", "C", "D"];
        let placed = place(
            &songs,
            |_| false,
            |artist: &&str| Some(artist.to_string()),
            None,
            0.35,
            &mut DotnetRandom::new(1),
        );
        let first = placed
            .iter()
            .position(|artist| *artist == "A")
            .expect("an A plays");
        let second = placed
            .iter()
            .rposition(|artist| *artist == "A")
            .expect("an A plays");
        assert!(
            second - first > 3,
            "the second A came {} after the first: {placed:?}",
            second - first
        );
        assert_eq!(placed.len(), 5);
    }

    #[test]
    fn share_zero_plays_the_library_first_and_fills_with_outside() {
        let placed = placed(&songs(5, 5), 0.0, 1);
        assert_eq!(pattern(&placed), "LLLLLOOOOO");
    }
}
