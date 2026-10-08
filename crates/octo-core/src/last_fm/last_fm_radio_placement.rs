//! Where a radio's songs from outside the library land. The same rule as the Octo apps'
//! radio (`RadioMix.kt`, `outsideDue`): a draw at the share, so outside songs come at random
//! instead of on a schedule, pulled back once the radio is more than [`SHARE_SLACK`] songs
//! off its share, and never more than [`RARER_RUN`] of the rarer kind in a row.

use super::last_fm_radio_recommendation_service::RadioRandom;
use super::last_fm_radio_spacing::artist_key;

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

/// How far apart [`place`] keeps one artist's songs, and one title's.
pub struct Spacing<'a, T> {
    /// A song waits while its artist is among the last this many artists played, counting the
    /// seed's.
    pub artist_gap: usize,
    /// A song's title key, and how many songs must play before that title comes again. None
    /// spaces no titles.
    pub title: Option<(&'a dyn Fn(&T) -> String, usize)>,
}

impl<T> Spacing<'_, T> {
    /// Artists only, as `spread` spaces song radio ([`super::last_fm_radio_spacing::GAP`]).
    pub fn artists(artist_gap: usize) -> Self {
        Spacing {
            artist_gap,
            title: None,
        }
    }
}

/// The songs, each kind (`is_outside`) in its own order, interleaved by [`outside_due`] at
/// `share`. Within the due kind a song waits while its artist or its title is too recent (see
/// [`Spacing`]). When every song of the due kind waits, the first song of the other kind that
/// need not wait plays instead. When no song of either kind can play spaced, the due kind's
/// first goes in at the latest place where it keeps both gaps (putting a song in only moves the
/// songs after it further from their neighbors), and at the end only when there is no such
/// place: an artist plays twice in a row only when nothing else can. Nothing is dropped: when
/// one kind runs out the other plays on.
pub fn place<T, O, A>(
    songs: &[T],
    is_outside: O,
    artist_of: A,
    seed_artist: Option<&str>,
    spacing: Spacing<'_, T>,
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
    let gaps = Gaps {
        seed: artist_key(seed_artist),
        artist: spacing.artist_gap,
        title: spacing.title.map_or(0, |(_, gap)| gap),
    };
    let key_of = |song: &T| Keys {
        artist: artist_key(artist_of(song).as_deref()),
        title: spacing
            .title
            .map(|(title_of, _)| title_of(song))
            .unwrap_or_default(),
    };
    let mut placed: Vec<T> = Vec::with_capacity(songs.len());
    // Beside each placed song, its keys.
    let mut keys: Vec<Keys> = Vec::with_capacity(songs.len());
    let (mut taken_outside, mut run) = (0usize, 0i32);
    while !outside.is_empty() || !library.is_empty() {
        let want_outside = if outside.is_empty() {
            false
        } else if library.is_empty() {
            true
        } else {
            outside_due(share, placed.len(), taken_outside, run, random)
        };
        let (from_outside, next, spaced) = {
            let first = |pool: &[T]| {
                pool.iter()
                    .position(|song| gaps.fit(&keys, keys.len(), &key_of(song)))
            };
            let (due, other) = if want_outside {
                (&outside, &library)
            } else {
                (&library, &outside)
            };
            match (first(due), first(other)) {
                (Some(next), _) => (want_outside, next, true),
                // Every song of the due kind would come too soon: the other kind plays first.
                (None, Some(next)) => (!want_outside, next, true),
                (None, None) => (want_outside, 0, false),
            }
        };
        let song = if from_outside {
            outside.remove(next)
        } else {
            library.remove(next)
        };
        let key = key_of(&song);
        let at = if spaced {
            keys.len()
        } else {
            (0..keys.len())
                .rev()
                .find(|&at| gaps.fit(&keys, at, &key))
                .unwrap_or(keys.len())
        };
        if from_outside {
            taken_outside += 1;
        }
        // The run counts the radio's last songs, which a song put in earlier does not change.
        if at == keys.len() {
            run = if from_outside {
                run.max(0) + 1
            } else {
                run.min(0) - 1
            };
        }
        placed.insert(at, song);
        keys.insert(at, key);
    }
    placed
}

/// A placed song's artist and title keys. A blank key (no artist, or no title spacing) never
/// holds a song back and does not count toward a gap.
struct Keys {
    artist: String,
    title: String,
}

impl Keys {
    fn artist(&self) -> &str {
        &self.artist
    }

    fn title(&self) -> &str {
        &self.title
    }
}

/// The gaps [`place`] keeps, with the seed's artist, which plays before the first song.
struct Gaps {
    seed: String,
    artist: usize,
    title: usize,
}

impl Gaps {
    /// Whether a song with `key` can go in at `at` among the placed songs: its artist is not one
    /// of the [`Gaps::artist`] artists just before it (counting the seed) or just after it, nor
    /// its title one of the [`Gaps::title`] titles either side.
    fn fit(&self, placed: &[Keys], at: usize, key: &Keys) -> bool {
        let clear = |of: fn(&Keys) -> &str, mine: &str, gap: usize, seed: Option<&str>| {
            mine.is_empty()
                || !(among(placed[..at].iter().rev().map(of).chain(seed), gap, mine)
                    || among(placed[at..].iter().map(of), gap, mine))
        };
        clear(Keys::artist, key.artist(), self.artist, Some(&self.seed))
            && clear(Keys::title, key.title(), self.title, None)
    }
}

/// Whether `mine` is one of the first `gap` different keys, blank ones not counted, as
/// `spread` keeps its recent artists.
fn among<'a>(keys: impl Iterator<Item = &'a str>, gap: usize, mine: &str) -> bool {
    let mut seen: Vec<&str> = Vec::with_capacity(gap);
    for key in keys.filter(|key| !key.is_empty()) {
        if seen.len() == gap {
            break;
        }
        if key == mine {
            return true;
        }
        if !seen.contains(&key) {
            seen.push(key);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::common::dotnet_random::DotnetRandom;
    use crate::last_fm::last_fm_radio_spacing::GAP;

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
            Spacing::artists(GAP),
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
            Spacing::artists(GAP),
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

    /// `(name, artist, outside)`.
    type Named = (&'static str, &'static str, bool);

    fn names(placed: &[Named]) -> Vec<&'static str> {
        placed.iter().map(|(name, _, _)| *name).collect()
    }

    #[test]
    fn the_other_kind_plays_before_an_artist_comes_back_to_back() {
        let place_named = |songs: &[Named], share: f64| {
            place(
                songs,
                |(_, _, outside): &Named| *outside,
                |(_, artist, _): &Named| Some(artist.to_string()),
                None,
                Spacing::artists(GAP),
                share,
                &mut Fixed(0.5),
            )
        };
        // The library is due every time (share 0), but its second song is by the artist that
        // just played: the outside song plays between them.
        let songs = [("A1", "A", false), ("A2", "A", false), ("X", "X", true)];
        assert_eq!(names(&place_named(&songs, 0.0)), ["A1", "X", "A2"]);
        // And the other way round: outside is due every time (share 1).
        let songs = [("B1", "B", true), ("B2", "B", true), ("L", "L", false)];
        assert_eq!(names(&place_named(&songs, 1.0)), ["B1", "L", "B2"]);
    }

    #[test]
    fn a_title_waits_its_gap_when_titles_are_spaced() {
        let title_of = |(name, _, _): &Named| name.trim_end_matches(char::is_numeric).to_string();
        let songs = [
            ("Intro1", "A", false),
            ("Intro2", "B", false),
            ("Song", "C", false),
            ("Tune", "D", false),
        ];
        let placed = place(
            &songs,
            |(_, _, outside): &Named| *outside,
            |(_, artist, _): &Named| Some(artist.to_string()),
            None,
            Spacing {
                artist_gap: GAP,
                title: Some((&title_of, 2)),
            },
            0.35,
            &mut Fixed(0.5),
        );
        assert_eq!(names(&placed), ["Intro1", "Song", "Tune", "Intro2"]);
    }
}
