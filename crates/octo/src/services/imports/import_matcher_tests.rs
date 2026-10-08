//! The matching half of `ImportMatchTests.cs`.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::spotify_web_api::ImportKeys;
use super::super::test_support::row;
use super::*;

fn r(id: &str, artist: &str, title: &str, seconds: i32) -> LibrarySongRow {
    row(id, artist, title, Some(seconds), None, "mp3", 320)
}

fn track(artist: &str, title: &str, seconds: Option<i32>, album: Option<&str>) -> ImportTrack {
    ImportTrack {
        key: ImportKeys::for_words(Some(artist), Some(title)),
        artist: artist.into(),
        title: title.into(),
        seconds,
        album: album.map(str::to_string),
        ..Default::default()
    }
}

fn found<'a>(index: &'a LibraryIndex, t: &ImportTrack) -> Option<&'a str> {
    index.find(t).map(|row| row.id.as_str())
}

#[test]
fn the_same_song_is_found_however_its_credit_and_version_words_are_written() {
    let index = LibraryIndex::new([
        r("1", "Drake", "Too Good (feat. Rihanna)", 263),
        r("2", "The Beatles", "Let It Be - Remastered 2009", 243),
        r("3", "Beyoncé", "Halo", 261),
        r("4", "Rihanna, Calvin Harris", "We Found Love", 215),
    ]);
    assert_eq!(
        found(&index, &track("Drake, Rihanna", "Too Good", Some(263), None)),
        Some("1")
    );
    assert_eq!(
        found(&index, &track("The Beatles", "Let It Be", Some(243), None)),
        Some("2")
    );
    assert_eq!(
        found(&index, &track("Beyonce", "Halo", Some(262), None)),
        Some("3")
    );
    // The other credit order still names the same artists.
    assert_eq!(
        found(
            &index,
            &track("Calvin Harris, Rihanna", "We Found Love", Some(215), None)
        ),
        Some("4")
    );
}

#[test]
fn a_live_take_or_a_far_length_is_another_song() {
    let index = LibraryIndex::new([
        r("1", "Nirvana", "Lithium (Live)", 260),
        r("2", "Nirvana", "Polly", 300),
    ]);
    assert_eq!(found(&index, &track("Nirvana", "Lithium", Some(257), None)), None);
    assert_eq!(found(&index, &track("Nirvana", "Polly", Some(177), None)), None);
    assert_eq!(
        found(&index, &track("Someone Else", "Polly", Some(300), None)),
        None
    );
}

#[test]
fn a_song_with_no_length_is_matched_on_its_words_and_the_closest_copy_wins() {
    let index = LibraryIndex::new([
        row(
            "mp3",
            "Massive Attack",
            "Teardrop",
            Some(330),
            Some("Mezzanine"),
            "mp3",
            320,
        ),
        row(
            "flac",
            "Massive Attack",
            "Teardrop",
            Some(331),
            Some("Mezzanine"),
            "flac",
            900,
        ),
        row(
            "edit",
            "Massive Attack",
            "Teardrop",
            Some(320),
            Some("Singles"),
            "mp3",
            320,
        ),
    ]);
    assert_eq!(
        found(
            &index,
            &track("Massive Attack", "Teardrop", None, Some("Mezzanine"))
        ),
        Some("flac")
    );
    assert_eq!(
        found(&index, &track("Massive Attack", "Teardrop", Some(320), None)),
        Some("edit")
    );
}

#[tokio::test]
async fn the_matcher_reads_the_whole_library_once_and_again_after_forget() {
    let reads = Arc::new(AtomicUsize::new(0));
    let library: Arc<Vec<LibrarySongRow>> = Arc::new(
        (0..1500)
            .map(|i| r(&i.to_string(), &format!("Artist {i}"), &format!("Song {i}"), 200))
            .collect(),
    );
    let counted = reads.clone();
    let matcher = ImportMatcher::with_pages(Arc::new(move |start, count| {
        counted.fetch_add(1, Ordering::SeqCst);
        let page: Vec<LibrarySongRow> = library
            .iter()
            .skip(start as usize)
            .take(count as usize)
            .cloned()
            .collect();
        let n = page.len();
        Box::pin(async move { Some((page, n)) })
    }));
    let mut tracks = vec![
        track("Artist 1499", "Song 1499", Some(200), None),
        track("Nobody", "Nothing", Some(200), None),
    ];
    assert!(matcher.match_tracks(&mut tracks).await);
    assert_eq!(tracks[0].library_id.as_deref(), Some("1499"));
    assert_eq!(tracks[1].library_id, None);
    assert_eq!(reads.load(Ordering::SeqCst), 2);

    matcher.match_tracks(&mut tracks).await;
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    matcher.forget();
    matcher.match_tracks(&mut tracks).await;
    assert_eq!(reads.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_library_that_cannot_be_read_keeps_what_the_last_match_found() {
    let matcher = ImportMatcher::with_pages(Arc::new(|_, _| Box::pin(async { None })));
    let mut tracks = vec![track("A", "B", Some(200), None)];
    tracks[0].library_id = Some("kept".into());
    assert!(!matcher.match_tracks(&mut tracks).await);
    assert_eq!(tracks[0].library_id.as_deref(), Some("kept"));
    assert_eq!(
        matcher.problem().as_deref(),
        Some(ImportMatcher::CANNOT_READ_LIBRARY)
    );
}
