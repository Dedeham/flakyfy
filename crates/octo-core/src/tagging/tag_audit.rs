//! A library file's tags held against the one recording its audio was found to be.
//!
//! The same rules the write-time guard runs on a download, run on a file already in the
//! library: stray spaces, an album that names another take, a take the title hides, and ids
//! that belong to another recording than the audio. Each finding says in words what is wrong,
//! and the audit names the tags that would put it right. Nothing here writes; the library
//! sweep and Library health decide what is written, and when.

use std::collections::{BTreeSet, HashMap};

use indexmap::IndexMap;

use crate::common::dotnet::{self, is_blank};
use crate::common::song_identity::SongIdentity;
use crate::fingerprint::recording_identity::{Identity, RecordingKind};
use crate::models::domain::song::Song;
use crate::tagging::tag_guard::{self, GuardFinding};

/// The names the apps and Library health use for the tags an audit may write.
pub mod audit_fields {
    pub const TITLE: &str = "title";
    pub const ALBUM: &str = "album";
    pub const RECORDING_ID: &str = "recordingId";
    pub const RELEASE_TRACK_ID: &str = "releaseTrackId";
    pub const RELEASE_ID: &str = "releaseId";
    pub const RELEASE_GROUP_ID: &str = "releaseGroupId";
}

use audit_fields as fields;

/// What a library file's tags say now, as written: spaces and all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTags {
    pub title: String,
    pub album: String,
    pub recording_id: Option<String>,
    pub release_track_id: Option<String>,
    pub release_id: Option<String>,
    pub release_group_id: Option<String>,
}

/// One kind of disagreement between a file's tags and its audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AuditRule {
    /// A title or an album with spaces at either end.
    Spaces,
    /// The album names a take the audio is not: studio audio on "(instrumentals)".
    Album,
    /// The audio is a take its title does not say: a live take titled plainly.
    Title,
    /// The recording id or the release track id belongs to another recording than the audio.
    Ids,
    /// Another copy of the same song in the library is a different take.
    Takes,
}

impl AuditRule {
    pub const ALL: [AuditRule; 5] = [
        AuditRule::Spaces,
        AuditRule::Album,
        AuditRule::Title,
        AuditRule::Ids,
        AuditRule::Takes,
    ];

    /// The rule's name, as the state file and the dashboard keep it.
    pub fn name(self) -> &'static str {
        match self {
            AuditRule::Spaces => "spaces",
            AuditRule::Album => "album",
            AuditRule::Title => "title",
            AuditRule::Ids => "ids",
            AuditRule::Takes => "takes",
        }
    }

    pub fn parse(name: &str) -> Option<AuditRule> {
        AuditRule::ALL.into_iter().find(|rule| rule.name() == name)
    }

    /// A few words for the rule, as a badge shows it.
    pub fn label(self) -> &'static str {
        match self {
            AuditRule::Spaces => "Stray spaces",
            AuditRule::Album => "Album names another take",
            AuditRule::Title => "Title hides the take",
            AuditRule::Ids => "Ids of another recording",
            AuditRule::Takes => "Another take beside it",
        }
    }
}

/// One thing an audit found, in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFinding {
    pub rule: AuditRule,
    pub words: String,
}

/// What the fingerprint said about a file's audio, beyond the one recording it decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeardAs {
    pub identity: Identity,
    /// The decision cannot be another recording: the exact length chose it, or it was the only
    /// recording that agreed with the song. Only a sure identity writes ids, and only sure
    /// findings are fixed all at once.
    pub sure: bool,
    /// The kind of take the file's own recording id is, when the fingerprint named it too.
    pub written_kind: Option<RecordingKind>,
    /// The recording the file's release track id belongs to, and its kind, when known.
    pub track_owner: Option<(String, RecordingKind)>,
    /// Whether the file's release lists the audio's recording: `Some(Some(track))` with the
    /// track it is there, `Some(None)` when it does not list it, None when that is unknown.
    pub on_release: Option<Option<String>>,
}

/// What an audit found, and the tags that would put it right: a value to write, or None to
/// take a tag off the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagAudit {
    pub findings: Vec<AuditFinding>,
    pub changes: IndexMap<String, Option<String>>,
    pub sure: bool,
}

impl TagAudit {
    /// Whether the audit found anything.
    pub fn found(&self) -> bool {
        !self.findings.is_empty()
    }

    /// Whether every finding is only stray spaces, which are put right without asking.
    pub fn only_spaces(&self) -> bool {
        self.found() && self.findings.iter().all(|f| f.rule == AuditRule::Spaces)
    }

    /// The changes that take stray spaces off and nothing else.
    pub fn space_changes(&self, tags: &FileTags) -> IndexMap<String, Option<String>> {
        let mut changes = IndexMap::new();
        let title = tags.title.trim();
        if title.len() != tags.title.len() && !title.is_empty() {
            changes.insert(fields::TITLE.to_string(), Some(title.to_string()));
        }
        let album = tags.album.trim();
        if album.len() != tags.album.len() && !album.is_empty() {
            changes.insert(fields::ALBUM.to_string(), Some(album.to_string()));
        }
        changes
    }
}

/// Holds a file's tags against its audio. Without `heard` only the spaces are checked: with no
/// recording found for the audio there is nothing to hold the rest against.
pub fn audit(tags: &FileTags, heard: Option<&HeardAs>) -> TagAudit {
    let mut song = Song {
        title: tags.title.clone(),
        album: tags.album.clone(),
        ..Default::default()
    };
    // No ids on the song the guard sees: a library file's ids are judged below, against the
    // release they came from, which the guard of a download never has.
    let guarded = tag_guard::guard(&mut song, heard.map(|h| &h.identity), None);
    let mut found = TagAudit {
        sure: heard.is_some_and(|h| h.sure),
        ..TagAudit::default()
    };
    // The guard decides; the words are said here, about the file as it is now.
    for GuardFinding { rule, detail, fixed } in guarded {
        let (rule, words) = match (rule, heard) {
            ("spaces", _) => {
                let (what, value) = if detail.contains("the album") {
                    ("album", tags.album.trim())
                } else {
                    ("title", tags.title.trim())
                };
                (
                    AuditRule::Spaces,
                    format!("The {what} '{value}' has stray spaces at its ends."),
                )
            }
            ("album agrees", Some(heard)) => {
                let asked = RecordingKind::requested_by(&tags.title);
                let named = heard
                    .identity
                    .contradicted_by(&tags.album, &asked)
                    .map_or("another take", RecordingKind::described);
                let left = if fixed {
                    ""
                } else {
                    " Nothing plain is left of the name, so it waits for you."
                };
                (
                    AuditRule::Album,
                    format!(
                        "The album '{}' names {named}, but the audio is {}.{left}",
                        tags.album.trim(),
                        heard.identity.kind.described()
                    ),
                )
            }
            ("take in title", Some(heard)) => (
                AuditRule::Title,
                format!(
                    "The audio is {}, and the title does not say so.",
                    heard.identity.kind.described()
                ),
            ),
            _ => continue,
        };
        found.findings.push(AuditFinding { rule, words });
    }
    if song.title != tags.title {
        found
            .changes
            .insert(fields::TITLE.to_string(), Some(song.title.clone()));
    }
    if song.album != tags.album {
        found
            .changes
            .insert(fields::ALBUM.to_string(), Some(song.album.clone()));
    }
    if let Some(heard) = heard {
        ids(tags, heard, &mut found);
    }
    found
}

/// The recording id and the release ids beside it all belong to the recording the audio is.
fn ids(tags: &FileTags, heard: &HeardAs, found: &mut TagAudit) {
    let identity = &heard.identity;
    let written = tags.recording_id.as_deref().filter(|id| !is_blank(id));
    let mut wrong_ids = false;
    let mut wrong_recording = false;
    if let Some(written) = written
        && !dotnet::eq_ignore_case(written, &identity.recording_id)
        && !identity.admits(written, heard.written_kind.unwrap_or(identity.kind))
    {
        wrong_recording = true;
        let what = heard
            .written_kind
            .map_or("another recording", RecordingKind::described);
        found.findings.push(AuditFinding {
            rule: AuditRule::Ids,
            words: format!(
                "The recording id {written} is {what}, but the audio is {} ({}).",
                identity.kind.described(),
                identity.recording_id
            ),
        });
        wrong_ids = true;
    }
    if let (Some(track), Some((owner, owner_kind))) = (
        tags.release_track_id.as_deref().filter(|id| !is_blank(id)),
        heard.track_owner.as_ref(),
    ) && !identity.admits(owner, *owner_kind)
    {
        found.findings.push(AuditFinding {
            rule: AuditRule::Ids,
            words: format!(
                "The release track id {track} is a track of {} ({owner}), but the audio is {}.",
                owner_kind.described(),
                identity.kind.described()
            ),
        });
        wrong_ids = true;
    }
    if !heard.sure {
        return;
    }
    // A file with no recording id gets the audio's; one with another recording's id gets it in
    // its place. One that names another recording of the same take keeps what it has.
    if written.is_none() || wrong_recording {
        found.changes.insert(
            fields::RECORDING_ID.to_string(),
            Some(identity.recording_id.clone()),
        );
    }
    if !wrong_ids {
        return;
    }
    match &heard.on_release {
        // The file's release has the audio's recording: its track there is the one to name.
        Some(Some(own)) => {
            if tags
                .release_track_id
                .as_deref()
                .is_none_or(|track| !dotnet::eq_ignore_case(track, own))
            {
                found
                    .changes
                    .insert(fields::RELEASE_TRACK_ID.to_string(), Some(own.clone()));
            }
        }
        // A release the audio is not on: none of its ids describe this file.
        Some(None) => {
            for (field, value) in [
                (fields::RELEASE_TRACK_ID, &tags.release_track_id),
                (fields::RELEASE_ID, &tags.release_id),
                (fields::RELEASE_GROUP_ID, &tags.release_group_id),
            ] {
                if value.as_deref().is_some_and(|id| !is_blank(id)) {
                    found.changes.insert(field.to_string(), None);
                }
            }
        }
        // Whether the release has it is unknown: only the track known to be wrong comes off.
        None => {
            if tags.release_track_id.as_deref().is_some_and(|id| !is_blank(id)) {
                found.changes.insert(fields::RELEASE_TRACK_ID.to_string(), None);
            }
        }
    }
}

/// One copy of a song, as the takes check reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeCopy {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub kind: RecordingKind,
}

/// Copies of one song in the library where one is another take than the other, and the title
/// does not say so: a live take beside the studio copy, both titled plainly. Answers each such
/// copy's id with what it is beside.
pub fn takes_beside(copies: &[TakeCopy]) -> Vec<(String, String)> {
    let mut by_song: HashMap<String, Vec<&TakeCopy>> = HashMap::new();
    for copy in copies {
        if is_blank(&copy.title) {
            continue;
        }
        by_song
            .entry(SongIdentity::match_key(&copy.artist, &copy.title))
            .or_default()
            .push(copy);
    }
    let mut found: Vec<(String, String)> = Vec::new();
    for group in by_song.values().filter(|group| group.len() > 1) {
        let kinds: BTreeSet<RecordingKind> = group.iter().map(|copy| copy.kind).collect();
        if kinds.len() < 2 {
            continue;
        }
        for copy in group {
            if copy.kind == RecordingKind::Studio
                || RecordingKind::marked_in(&copy.title).contains(&copy.kind)
            {
                continue;
            }
            let Some(other) = group
                .iter()
                .find(|other| other.kind != copy.kind && other.id != copy.id)
            else {
                continue;
            };
            let place = if is_blank(&other.album) {
                String::new()
            } else {
                format!(" on '{}'", other.album.trim())
            };
            found.push((
                copy.id.clone(),
                format!(
                    "This copy is {}, and the library also has {}{place}. Both are titled '{}'.",
                    copy.kind.described(),
                    other.kind.described(),
                    copy.title.trim()
                ),
            ));
        }
    }
    found.sort();
    found
}

#[cfg(test)]
#[path = "tag_audit_tests.rs"]
mod tests;
