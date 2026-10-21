//! Which songs are fetched ahead, and which of them are let go. The cases that matter are the
//! ones where a song is kept: a trash that guesses is a trash that loses someone's song.

use super::*;

fn song(id: &str) -> Candidate {
    Candidate {
        provider: "soulseek".into(),
        external_id: id.into(),
        artist: "Artist".into(),
        title: id.into(),
    }
}

fn day(n: i64) -> DateTime<Utc> {
    DateTime::UNIX_EPOCH + TimeDelta::days(n)
}

fn ids(songs: &[Candidate]) -> Vec<&str> {
    songs.iter().map(|c| c.external_id.as_str()).collect()
}

#[test]
fn pick_takes_count_of_the_songs_not_skipped() {
    let all = vec![song("a"), song("b"), song("c"), song("d")];
    let picked = pick(all, 2, |c| c.external_id == "a", |_| 0);
    assert_eq!(picked.len(), 2);
    assert!(picked.iter().all(|c| c.external_id != "a"));
}

#[test]
fn pick_gives_all_there_are_when_fewer_than_count() {
    let picked = pick(vec![song("a"), song("b")], 10, |_| false, |_| 0);
    assert_eq!(picked.len(), 2);
}

#[test]
fn pick_of_nothing_or_count_zero_is_empty() {
    assert!(pick(Vec::new(), 5, |_| false, |_| 0).is_empty());
    assert!(pick(vec![song("a")], 0, |_| false, |_| 0).is_empty());
}

#[test]
fn pick_follows_the_draw_and_never_repeats_a_song() {
    // Always drawing the first index: the back of the pool is swapped with the front each time.
    let picked = pick(
        vec![song("a"), song("b"), song("c"), song("d")],
        3,
        |_| false,
        |_| 0,
    );
    let mut seen = ids(&picked);
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 3);
}

#[test]
fn pick_survives_a_draw_out_of_range() {
    let picked = pick(vec![song("a"), song("b")], 2, |_| false, |n| n + 100);
    assert_eq!(picked.len(), 2);
}

#[test]
fn expiry_waits_until_the_song_is_old_enough() {
    let uptake = Uptake {
        marked: Some(false),
        plays: Some(0),
    };
    assert_eq!(expiry(day(0), day(29), 30, 2, uptake), Expiry::NotYet);
    assert_eq!(expiry(day(0), day(30), 30, 2, uptake), Expiry::Let);
}

#[test]
fn expiry_keeps_a_starred_or_rated_song() {
    let uptake = Uptake {
        marked: Some(true),
        plays: Some(0),
    };
    assert!(matches!(expiry(day(0), day(90), 30, 2, uptake), Expiry::Keep(_)));
}

#[test]
fn expiry_keeps_a_song_played_often_enough() {
    let played = |plays| Uptake {
        marked: Some(false),
        plays: Some(plays),
    };
    assert_eq!(expiry(day(0), day(90), 30, 2, played(1)), Expiry::Let);
    assert!(matches!(
        expiry(day(0), day(90), 30, 2, played(2)),
        Expiry::Keep(_)
    ));
}

#[test]
fn expiry_leaves_a_song_whose_marks_or_plays_are_unknown_to_be_looked_at_again() {
    let unknown_marks = Uptake {
        marked: None,
        plays: Some(0),
    };
    let unknown_plays = Uptake {
        marked: Some(false),
        plays: None,
    };
    assert!(matches!(
        expiry(day(0), day(90), 30, 2, unknown_marks),
        Expiry::Unknown(_)
    ));
    assert!(matches!(
        expiry(day(0), day(90), 30, 2, unknown_plays),
        Expiry::Unknown(_)
    ));
}
