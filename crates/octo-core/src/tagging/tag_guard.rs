//! The last check before Octo writes a song's tags: every tag must agree with the one recording
//! the audio is.
//!
//! Each rule either fixes the song in place or names what it could not settle. Nothing here
//! guesses: a take is written into the title only when the audio was found to be that take, and
//! an album is only cut back to its plain name when the audio was found to be another take than
//! the one the album names.

use std::collections::BTreeSet;

use crate::common::dotnet::{self, is_blank};
use crate::common::song_identity::SongIdentity;
use crate::fingerprint::recording_identity::{Identity, RecordingKind};
use crate::models::domain::song::Song;
use crate::tagging::tag_evidence::ReleaseCandidate;

/// What a rule changed, or found and left, in words for the log and "How it was tagged".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardFinding {
    pub rule: &'static str,
    pub detail: String,
    /// The rule changed the song; false when it only found something a person should look at.
    pub fixed: bool,
}

impl GuardFinding {
    fn fixed(rule: &'static str, detail: String) -> Self {
        Self {
            rule,
            detail,
            fixed: true,
        }
    }

    fn found(rule: &'static str, detail: String) -> Self {
        Self {
            rule,
            detail,
            fixed: false,
        }
    }
}

/// Runs every rule on the song about to be written. `identity` is the recording the audio was
/// found to be; `chosen` is the release the album came from.
pub fn guard(
    song: &mut Song,
    identity: Option<&Identity>,
    chosen: Option<&ReleaseCandidate>,
) -> Vec<GuardFinding> {
    let mut findings = Vec::new();
    trim_edges(song, &mut findings);
    if let Some(identity) = identity {
        one_recording(song, identity, chosen, &mut findings);
        album_agrees(song, identity, &mut findings);
        take_in_title(song, identity, &mut findings);
    } else {
        unconfirmed_album(song, &mut findings);
    }
    findings
}

/// Titles and albums carry no stray spaces at either end.
fn trim_edges(song: &mut Song, findings: &mut Vec<GuardFinding>) {
    let title = song.title.trim();
    if title.len() != song.title.len() && !title.is_empty() {
        findings.push(GuardFinding::fixed(
            "spaces",
            format!("took the spaces off the title '{title}'"),
        ));
        song.title = title.to_string();
    }
    let album = song.album.trim();
    if album.len() != song.album.len() && !album.is_empty() {
        findings.push(GuardFinding::fixed(
            "spaces",
            format!("took the spaces off the album '{album}'"),
        ));
        song.album = album.to_string();
    }
}

/// The recording id, and the release ids beside it, all belong to the recording the audio is:
/// a file never names one take as its recording and another take's release track.
fn one_recording(
    song: &mut Song,
    identity: &Identity,
    chosen: Option<&ReleaseCandidate>,
    findings: &mut Vec<GuardFinding>,
) {
    if let Some(written) = song
        .music_brainz_recording_id
        .as_deref()
        .filter(|id| !is_blank(id))
        && !dotnet::eq_ignore_case(written, &identity.recording_id)
    {
        // Another recording of the same kind may stand for the audio (an album and a
        // compilation list it twice); one the exact length ruled out may not.
        if identity.by_length {
            findings.push(GuardFinding::fixed(
                "one recording",
                format!(
                    "the recording id {written} is another take than the audio ({}); written as {}",
                    identity.kind.described(),
                    identity.recording_id
                ),
            ));
            song.music_brainz_recording_id = Some(identity.recording_id.clone());
            clear_release_ids(song);
            return;
        }
    }
    let recording = song
        .music_brainz_recording_id
        .as_deref()
        .filter(|id| !is_blank(id))
        .map(str::to_string);
    let release_from_another = match (
        recording.as_deref(),
        chosen.and_then(|c| c.recording_id.as_deref()),
    ) {
        (Some(recording), Some(chosen)) => !dotnet::eq_ignore_case(recording, chosen),
        _ => false,
    };
    if release_from_another && has_release_ids(song) {
        findings.push(GuardFinding::fixed(
            "one recording",
            "the release ids belonged to another recording than the one written; left off".to_string(),
        ));
        clear_release_ids(song);
    }
}

fn has_release_ids(song: &Song) -> bool {
    [
        &song.music_brainz_release_track_id,
        &song.music_brainz_release_id,
        &song.music_brainz_release_group_id,
    ]
    .iter()
    .any(|id| id.as_deref().is_some_and(|id| !is_blank(id)))
}

fn clear_release_ids(song: &mut Song) {
    song.music_brainz_release_track_id = None;
    song.music_brainz_release_id = None;
    song.music_brainz_release_group_id = None;
}

/// An album that names another take than the audio is cut back to its plain name: "The Black
/// Parade (instrumentals)" for the studio take is "The Black Parade".
fn album_agrees(song: &mut Song, identity: &Identity, findings: &mut Vec<GuardFinding>) {
    if is_blank(&song.album) {
        return;
    }
    let asked = RecordingKind::requested_by(&song.title);
    let Some(named) = identity.contradicted_by(&song.album, &asked) else {
        return;
    };
    let plain = plain_album(&song.album);
    match plain {
        Some(plain) => {
            findings.push(GuardFinding::fixed(
                "album agrees",
                format!(
                    "the album '{}' names {}, but the audio is {}; written as '{plain}'",
                    song.album,
                    named.described(),
                    identity.kind.described()
                ),
            ));
            song.album = plain;
            song.music_brainz_album_title = None;
        }
        None => findings.push(GuardFinding::found(
            "album agrees",
            format!(
                "the album '{}' names {}, but the audio is {}; nothing plain was left of the name",
                song.album,
                named.described(),
                identity.kind.described()
            ),
        )),
    }
}

/// The album's name without its bracketed take, or None when nothing would be left.
fn plain_album(album: &str) -> Option<String> {
    let core = SongIdentity::parse_title(album, None).core;
    let core = core.trim();
    (!core.is_empty() && RecordingKind::named_in(core).is_empty()).then(|| core.to_string())
}

/// A take that is not the studio one says so in its title: "Welcome to the Black Parade
/// (Live)". Only when the audio was found to be that take.
fn take_in_title(song: &mut Song, identity: &Identity, findings: &mut Vec<GuardFinding>) {
    let Some(word) = identity.kind.title_word() else {
        return;
    };
    if RecordingKind::named_in(&song.title).contains(&identity.kind) {
        return;
    }
    let titled = format!("{} ({word})", song.title.trim());
    findings.push(GuardFinding::fixed(
        "take in title",
        format!("the audio is {}; titled '{titled}'", identity.kind.described()),
    ));
    song.title = titled;
}

/// With no recording found for the audio, an album that names a take the title does not is
/// only reported: it may be right (an instrumental on its own album) or a wrong tag.
fn unconfirmed_album(song: &Song, findings: &mut Vec<GuardFinding>) {
    if is_blank(&song.album) {
        return;
    }
    let in_title: BTreeSet<RecordingKind> = RecordingKind::named_in(&song.title);
    if let Some(named) = RecordingKind::named_in(&song.album)
        .into_iter()
        .find(|kind| !in_title.contains(kind))
    {
        findings.push(GuardFinding::found(
            "album agrees",
            format!(
                "the album '{}' names {} and nothing confirmed what the audio is",
                song.album,
                named.described()
            ),
        ));
    }
}

#[cfg(test)]
#[path = "tag_guard_tests.rs"]
mod tests;
