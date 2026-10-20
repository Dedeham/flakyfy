//! A play's genre: the song's own tag, else its album's, else the artist's top tags on Last.fm.
//! Album and artist answers are kept in the listening log's meta file, so each is asked once.

use indexmap::IndexMap;
use octo_core::json::element::try_get_property;
use octo_core::last_fm::last_fm_radio_recommendation_service::canonical_tag;
use serde_json::Value;
use tracing::debug;

use super::listening_log::ListeningLog;
use crate::services::last_fm::LastFmService;
use crate::services::subsonic::SubsonicProxyService;

/// Last.fm tags that name where an artist is from, or who sings, rather than the music.
const NOT_GENRES: [&str; 16] = [
    "american",
    "british",
    "canadian",
    "english",
    "uk",
    "usa",
    "french",
    "german",
    "swedish",
    "australian",
    "korean",
    "japanese",
    "male vocalists",
    "female vocalists",
    "singer-songwriter",
    "legend",
];

/// A library album to ask for its genre: its id, and the sign-in to ask with.
pub struct AlbumLookup {
    pub proxy: SubsonicProxyService,
    pub parameters: IndexMap<String, String>,
    pub album_id: String,
}

/// How genres are told apart: lower case, letters, digits and `&` only. "Hip-Hop", "Hip Hop"
/// and "hip hop" are one genre.
pub fn genre_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || *c == '&')
        .flat_map(char::to_lowercase)
        .collect()
}

/// A genre as the page shows it: as written, unless it is all lower case (as Last.fm writes
/// tags), then with each word capitalized ("hip-hop" is "Hip-Hop", "r&b" is "R&B").
pub fn display_genre(name: &str) -> String {
    let name = name.trim();
    if name.chars().any(char::is_uppercase) {
        return name.to_string();
    }
    if matches!(name, "edm" | "idm" | "uk garage") {
        return name.to_uppercase().replace("GARAGE", "garage");
    }
    let mut out = String::with_capacity(name.len());
    let mut start = true;
    for c in name.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            start = false;
        } else {
            out.push(c);
            start = c != '\'';
        }
    }
    out
}

/// The artist Last.fm is asked about: the first credited one ("Drake, 21 Savage" and
/// "Drake feat. Rihanna" are Drake).
pub fn main_artist(artist: &str) -> String {
    let mut name = artist.trim();
    let lower = name.to_lowercase();
    for cut in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(at) = lower.find(cut) {
            name = &name[..at];
            break;
        }
    }
    let name = name.split([',', ';']).next().unwrap_or(name).trim();
    name.to_string()
}

/// The first Last.fm tag for the artist that names a genre. None with no Last.fm key, when
/// Last.fm could not be asked, or when no tag is a genre.
pub async fn artist_genre_from_last_fm(last_fm: &LastFmService, artist: &str) -> Option<String> {
    if !last_fm.has_api_key() || artist.trim().is_empty() {
        return None;
    }
    let tags = match last_fm.get_artist_top_tags(&main_artist(artist), 10).await {
        Ok(tags) => tags,
        Err(e) => {
            debug!("No Last.fm tags for {artist}: {e}");
            return None;
        }
    };
    tags.iter()
        .map(|tag| canonical_tag(tag))
        .find(|tag| is_genre(tag))
        .map(|tag| display_genre(&tag))
}

fn is_genre(tag: &str) -> bool {
    if tag.is_empty() || NOT_GENRES.contains(&tag) {
        return false;
    }
    // Decades ("80s", "2010s") are when, not what.
    !(tag.ends_with('s') && tag[..tag.len() - 1].chars().all(|c| c.is_ascii_digit()))
}

/// The genre a play is filed under, from the song's own tags, its album's, or its artist's.
pub async fn resolve(
    log: &ListeningLog,
    last_fm: &LastFmService,
    song_genres: &[String],
    album: Option<AlbumLookup>,
    artist: &str,
) -> Option<String> {
    if let Some(own) = song_genres.iter().map(|g| g.trim()).find(|g| !g.is_empty()) {
        return Some(display_genre(own));
    }
    if let Some(album) = album.filter(|a| !a.album_id.is_empty()) {
        let kept = log.album_genre(&album.album_id);
        let genre = match kept {
            Some(kept) => kept,
            None => match album_genre(&album).await {
                Some(found) => {
                    log.remember_album_genre(&album.album_id, found.as_deref());
                    found
                }
                None => None,
            },
        };
        if let Some(genre) = genre {
            return Some(display_genre(&genre));
        }
    }
    if let Some(kept) = log.artist_genre(artist) {
        return kept;
    }
    if !last_fm.has_api_key() {
        return None;
    }
    let found = artist_genre_from_last_fm(last_fm, artist).await;
    log.remember_artist_genre(artist, found.as_deref());
    found
}

/// The album's genre as the library has it: Some(None) when it has none, None when the library
/// could not be asked.
async fn album_genre(album: &AlbumLookup) -> Option<Option<String>> {
    let mut parameters = album.parameters.clone();
    parameters.insert("id".into(), album.album_id.clone());
    parameters.insert("f".into(), "json".into());
    let result = album.proxy.relay_safe("rest/getAlbum", &parameters).await?;
    let document: Value = serde_json::from_slice(&result.body).ok()?;
    let response = try_get_property(&document, "subsonic-response").ok().flatten()?;
    if response.get("status").and_then(Value::as_str) != Some("ok") {
        return None;
    }
    let found = response.get("album").and_then(|album| {
        album
            .get("genres")
            .and_then(Value::as_array)
            .and_then(|genres| {
                genres
                    .iter()
                    .filter_map(|g| g.get("name").and_then(Value::as_str))
                    .map(str::trim)
                    .find(|g| !g.is_empty())
            })
            .or_else(|| {
                album
                    .get("genre")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|g| !g.is_empty())
            })
            .map(str::to_string)
    });
    Some(found)
}

/// The genres a library song's `getSong` entry lists: `genres[].name`, then `genre`.
pub fn song_genres(song: &Value) -> Vec<String> {
    let mut genres: Vec<String> = song
        .get("genres")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|g| g.get("name").and_then(Value::as_str))
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let Some(genre) = song
        .get("genre")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|g| !g.is_empty())
        && !genres.iter().any(|g| g.eq_ignore_ascii_case(genre))
    {
        genres.push(genre.to_string());
    }
    genres
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_that_differ_only_in_case_and_marks_are_one_genre() {
        assert_eq!(genre_key("Hip-Hop"), genre_key("hip hop"));
        assert_eq!(genre_key("Hip Hop"), genre_key("HIP-HOP"));
        assert_ne!(genre_key("Hip-Hop"), genre_key("Rap"));
        assert_ne!(genre_key("R&B"), genre_key("RB"));
    }

    #[test]
    fn last_fm_tags_are_capitalized_and_library_spellings_kept() {
        assert_eq!(display_genre("hip-hop"), "Hip-Hop");
        assert_eq!(display_genre("r&b"), "R&B");
        assert_eq!(display_genre("alternative rock"), "Alternative Rock");
        assert_eq!(display_genre("edm"), "EDM");
        assert_eq!(display_genre("Hip-Hop/Rap"), "Hip-Hop/Rap");
        assert_eq!(display_genre("drum 'n' bass"), "Drum 'n' Bass");
    }

    #[test]
    fn the_first_credited_artist_is_asked_about() {
        assert_eq!(main_artist("Drake feat. Rihanna"), "Drake");
        assert_eq!(main_artist("Drake, 21 Savage"), "Drake");
        assert_eq!(main_artist("Simon & Garfunkel"), "Simon & Garfunkel");
        assert_eq!(main_artist(" Kavinsky "), "Kavinsky");
    }

    #[test]
    fn where_from_and_decades_are_not_genres() {
        assert!(is_genre("rap"));
        assert!(!is_genre("american"));
        assert!(!is_genre("2010s"));
        assert!(!is_genre("80s"));
        assert!(!is_genre(""));
    }

    #[test]
    fn a_songs_genres_are_read_from_both_fields() {
        let song =
            serde_json::json!({ "genre": "Rap", "genres": [{ "name": "Hip-Hop" }, { "name": "Rap" }] });
        assert_eq!(song_genres(&song), vec!["Hip-Hop", "Rap"]);
        let bare = serde_json::json!({ "genre": " " });
        assert!(song_genres(&bare).is_empty());
    }
}
