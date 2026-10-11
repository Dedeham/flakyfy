//! Replacing a library song with a copy picked in Find songs that is no better than it: what
//! stands in the way, the words that ask the person, and the song a forced replacement is held
//! to.
//!
//! A pick that only fails on quality (the copy is the same or lower quality than the library's)
//! is asked about rather than refused. So is a pick of the plain song in place of a library file
//! that is another take of it (a live recording, a remix or edit, or a length far from the one
//! most copies of the song have): the file's own title, album and length describe that take, so
//! the plain song is what the copy is held to instead. A copy that is the wrong song, or the
//! wrong recording, stays refused.

use octo_core::common::song_identity::SongIdentity;
use octo_core::common::{dotnet, live_version, version_variant};
use octo_core::settings::SoulseekSettings;
use octo_core::soulseek::SoulseekFileHit;
use octo_core::soulseek::soulseek_download_service::DURATION_TOLERANCE_SECONDS;

use super::download_picks::AnywaySong;
use super::song_finder::{FindTarget, FoundCopy, is_lossless};
use crate::services::soulseek::rejected_peer_registry::RejectedPeerRegistry;
use crate::services::soulseek::soulseek_candidates::{self, Judged};

/// How many listed copies must agree on a length before it counts as the song's usual one.
pub const USUAL_LENGTH_COPIES: usize = 3;

/// What stands between a picked copy and the library song it would replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceCheck {
    /// Nothing: the copy is better and fits the library song.
    Fits,
    /// Only the person's word. `words` ask them; `song` is what the copy is held to once they
    /// say yes.
    Confirm { words: String, song: AnywaySong },
    /// The copy may not replace the library song, with the words why.
    Refused(String),
}

/// How the library file differs from the plain song.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtherTake {
    Live,
    Version,
    /// Its length is far from the length most listed copies of the song agree on.
    Length {
        yours: i32,
        usual: i32,
    },
}

/// What the checks are given besides the target and the copy.
#[derive(Clone, Copy)]
pub struct CheckContext<'a> {
    pub settings: &'a SoulseekSettings,
    pub rejected: Option<&'a RejectedPeerRegistry>,
    pub remembers: bool,
    /// Every copy the look listed, for the song's usual length.
    pub listed: &'a [FoundCopy],
}

/// Why a copy, by quality alone, would not replace the library's: None when it is better. The
/// same rule Better quality holds a replacement to when it arrives: only a FLAC-class file
/// replaces a lossy one, and nothing replaces a FLAC-class file.
pub fn quality_words(target: &FindTarget, copy: &FoundCopy) -> Option<String> {
    let owned_format = target.owned_format.as_deref();
    let yours = owned_short(target);
    let lidarr_choice = !copy.pick.is_soulseek() && copy.pick.release().is_none();
    let theirs = if lidarr_choice { None } else { copy_short(copy) };
    let copy_lossless = if copy.pick.is_soulseek() {
        is_lossless(copy.shown.format.as_deref())
    } else {
        is_lossless(first_word(copy.shown.format.as_deref()))
    };

    if is_lossless(owned_format) {
        let Some(theirs) = theirs else {
            return Some(if lidarr_choice {
                format!("Yours is already {yours}, so Lidarr's choice cannot be better.")
            } else {
                format!("Yours is already {yours}, so this copy cannot be better.")
            });
        };
        return Some(if !copy_lossless {
            format!("This copy is {theirs} and yours is {yours}, so it sounds worse.")
        } else if dotnet::eq_ignore_case(&theirs, &yours) {
            format!("This copy is {theirs} like yours, so it is not better.")
        } else {
            format!("This copy is {theirs} and yours is {yours}, so it is not better.")
        });
    }
    // Lidarr's choice searches FLAC-class releases only for a lossy library song.
    if lidarr_choice || copy_lossless {
        return None;
    }
    let theirs = theirs.unwrap_or_else(|| "a copy of unknown quality".to_string());
    Some(if dotnet::eq_ignore_case(&theirs, &yours) {
        format!("This copy is {theirs} like yours, so it is not better.")
    } else if matches!((bit_rate_of(&theirs), bit_rate_of(&yours)), (Some(a), Some(b)) if a < b) {
        format!("This copy is {theirs} and yours is {yours}, so it sounds worse.")
    } else {
        format!("This copy is {theirs} and yours is {yours}, so Octo does not count it as better.")
    })
}

/// The song the library file is, as the download's checks judge a copy against it.
pub fn library_song(target: &FindTarget) -> AnywaySong {
    AnywaySong {
        title: target.title.clone(),
        album: target.album.clone(),
        length: target.duration,
    }
}

/// Why a Soulseek copy is not this song, or None when it is (or when the copy is not a Soulseek
/// file, which has no name to judge): the download's own name, length and version checks, and
/// never a file already found to be the wrong recording.
pub fn wrong_song(
    target: &FindTarget,
    song: &AnywaySong,
    copy: &FoundCopy,
    cx: CheckContext<'_>,
) -> Option<String> {
    let hit = copy.pick.to_hit()?;
    soulseek_candidates::pick_refusal(&hit, judged(target, song, cx), true)
}

/// How the library file is another take of the song, and the plain song a copy is held to in
/// its place; None when nothing says it is one.
///
/// Its title, album, or album folder naming a version (live, remix, edit, ...) makes it one.
/// So does a length far from the song's usual length, which is the length at least
/// [`USUAL_LENGTH_COPIES`] listed Soulseek copies that fit the plain song by name and version
/// agree on (their middle value), when the picked copy has that length too.
///
/// The plain song is the title without its version, the album only when it names no version,
/// and the usual length, else the picked copy's own length (so its arrival is still checked
/// against what the peer said it was).
pub fn other_take(
    target: &FindTarget,
    copy: &FoundCopy,
    cx: CheckContext<'_>,
) -> Option<(OtherTake, AnywaySong)> {
    let artist = Some(target.artist.as_str());
    let title_versions = version_variant::requested(Some(&target.title), None);
    let title_live = live_version::mentions(Some(&target.title));
    let album = target.album.as_deref();
    let album_versions = album
        .map(|a| version_variant::requested(None, Some(a)))
        .unwrap_or_default();
    let album_live =
        album.is_some_and(|a| live_version::mentions(Some(&version_variant::without_artist(a, artist))));
    let folder_versions = target
        .owned_path
        .as_deref()
        .map(|path| version_variant::carried(path, artist))
        .unwrap_or_default();

    let marked_title = title_live || !title_versions.is_empty();
    let marked_album = album_live || !album_versions.is_empty() || !folder_versions.is_empty();
    let title = if marked_title {
        let core = SongIdentity::parse_title(&target.title, artist).core;
        if dotnet::is_blank(&core) {
            target.title.clone()
        } else {
            core
        }
    } else {
        target.title.clone()
    };
    let album = if marked_album { None } else { target.album.clone() };
    let usual = usual_length(target, &title, album.as_deref(), cx);
    let picked_length = copy.pick.length.filter(|l| *l > 0);

    let take = if title_live || album_live || folder_versions.contains("live") {
        OtherTake::Live
    } else if marked_title || marked_album {
        OtherTake::Version
    } else {
        let yours = target.duration.filter(|d| *d > 0)?;
        let usual = usual?;
        let picked = picked_length?;
        if (yours - usual).abs() <= DURATION_TOLERANCE_SECONDS
            || (picked - usual).abs() > DURATION_TOLERANCE_SECONDS
        {
            return None;
        }
        OtherTake::Length { yours, usual }
    };
    Some((
        take,
        AnywaySong {
            title,
            album,
            length: usual.or(picked_length),
        },
    ))
}

/// What stands between this copy and the library song: nothing, only the person's word, or a
/// refusal. Read again when the person says yes, so the answer never rests on an earlier one.
pub fn check(target: &FindTarget, copy: &FoundCopy, cx: CheckContext<'_>) -> ReplaceCheck {
    let quality = quality_words(target, copy);
    let own = library_song(target);
    let Some(wrong) = wrong_song(target, &own, copy, cx) else {
        return match quality {
            None => ReplaceCheck::Fits,
            Some(quality) => ReplaceCheck::Confirm {
                words: format!("{quality} Replace yours anyway?"),
                song: own,
            },
        };
    };
    let refused = || {
        ReplaceCheck::Refused(format!(
            "{wrong}, so it cannot take the place of the one in your library."
        ))
    };
    let Some((take, plain)) = other_take(target, copy, cx) else {
        return refused();
    };
    if wrong_song(target, &plain, copy, cx).is_some() {
        return refused();
    }
    let take_words = match take {
        OtherTake::Live => "Yours looks like a live recording and this copy is the plain song.".to_string(),
        OtherTake::Version => "Yours looks like another version and this copy is the plain song.".to_string(),
        OtherTake::Length { yours, usual } => format!(
            "Yours runs {} and this song usually runs {}, so yours looks like another take.",
            soulseek_candidates::length_text(yours),
            soulseek_candidates::length_text(usual)
        ),
    };
    let words = match quality {
        Some(quality) => format!("{take_words} {quality} Replace yours anyway?"),
        None => format!("{take_words} Replace yours anyway?"),
    };
    ReplaceCheck::Confirm { words, song: plain }
}

/// The length most listed copies of the plain song agree on: the middle one of the Soulseek
/// copies with a length that fit it by name and version, when there are enough of them.
fn usual_length(target: &FindTarget, title: &str, album: Option<&str>, cx: CheckContext<'_>) -> Option<i32> {
    let plain = AnywaySong {
        title: title.to_string(),
        album: album.map(str::to_string),
        length: None,
    };
    let song = judged(target, &plain, cx);
    let mut lengths: Vec<i32> = cx
        .listed
        .iter()
        .filter_map(|copy| copy.pick.to_hit())
        .filter(|hit| hit.length.is_some_and(|l| l > 0))
        .filter(|hit| soulseek_candidates::judge(hit, song).1 <= 1)
        .filter_map(|hit: SoulseekFileHit| hit.length)
        .collect();
    if lengths.len() < USUAL_LENGTH_COPIES {
        return None;
    }
    lengths.sort_unstable();
    Some(lengths[lengths.len() / 2])
}

fn judged<'a>(target: &'a FindTarget, song: &'a AnywaySong, cx: CheckContext<'a>) -> Judged<'a> {
    Judged {
        title: &song.title,
        album: song.album.as_deref(),
        duration: song.length,
        settings: cx.settings,
        rejected: cx.rejected,
        remembers_rejections: cx.remembers,
        artist: Some(&target.artist),
    }
}

/// The library file's quality in a word or two: "FLAC", "MP3 320".
fn owned_short(target: &FindTarget) -> String {
    let format = target
        .owned_format
        .as_deref()
        .map(|f| f.trim().trim_start_matches('.'))
        .unwrap_or("");
    let upper = dotnet::to_upper_invariant(format);
    if is_lossless(Some(format)) {
        return upper;
    }
    let rate = target.owned_quality.as_deref().and_then(kbps_in);
    match rate {
        Some(rate) if !upper.is_empty() => format!("{upper} {rate}"),
        _ if !upper.is_empty() => upper,
        _ => "a copy of unknown quality".to_string(),
    }
}

/// The copy's quality in a word or two: "FLAC", "MP3 320". Lidarr names its qualities "FLAC",
/// "FLAC 24bit", "MP3-320".
fn copy_short(copy: &FoundCopy) -> Option<String> {
    let format = copy
        .shown
        .format
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())?;
    if !copy.pick.is_soulseek() {
        if is_lossless(first_word(Some(format))) {
            return first_word(Some(format)).map(dotnet::to_upper_invariant);
        }
        return Some(dotnet::to_upper_invariant(&format.replace('-', " ")));
    }
    let upper = dotnet::to_upper_invariant(format.trim_start_matches('.'));
    if is_lossless(Some(format)) {
        return Some(upper);
    }
    Some(match copy.shown.bit_rate.filter(|r| *r > 0) {
        Some(rate) => format!("{upper} {rate}"),
        None => upper,
    })
}

/// "MP3 320 kbps" -> 320.
fn kbps_in(quality: &str) -> Option<i32> {
    let words: Vec<&str> = quality.split_whitespace().collect();
    words
        .windows(2)
        .find(|pair| dotnet::eq_ignore_case(pair[1], "kbps"))
        .and_then(|pair| pair[0].parse().ok())
}

/// "MP3 320" -> 320.
fn bit_rate_of(short: &str) -> Option<i32> {
    short.split_whitespace().nth(1).and_then(|rate| rate.parse().ok())
}

fn first_word(quality: Option<&str>) -> Option<&str> {
    quality?.split([' ', '-']).find(|part| !part.is_empty())
}

#[cfg(test)]
#[path = "replace_anyway_tests.rs"]
mod tests;
