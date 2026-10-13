//! Which one recording a file is, decided by its audio, and what kind of take that recording
//! is.
//!
//! One fingerprint is often tied to several recordings of the same song: the studio take, a
//! live take, an instrumental, a karaoke backing. Their titles read alike and their lengths sit
//! within seconds, so the title, the artist and a whole-second length cannot tell them apart.
//! Two things can: the file's length to the millisecond against each recording's, and what the
//! music database says each recording is (its title's markers, its disambiguation, the releases
//! it is on).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::acoust_id_client::{AcoustIdRecording, AcoustIdRelease};
use crate::common::song_identity::SongIdentity;
use crate::common::version_variant;
use crate::common::{dotnet, live_version};

/// What kind of take a recording is. Everything that is not one of the others is `Studio`,
/// including a remaster or an edit, which are the same performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RecordingKind {
    Studio,
    Live,
    Instrumental,
    Karaoke,
    Cover,
    Acapella,
    Remix,
    Demo,
}

impl RecordingKind {
    /// The word a title carries for this kind, as Octo writes it: "(Live)", "(Instrumental)".
    /// None for a studio take.
    pub fn title_word(self) -> Option<&'static str> {
        match self {
            RecordingKind::Studio => None,
            RecordingKind::Live => Some("Live"),
            RecordingKind::Instrumental => Some("Instrumental"),
            RecordingKind::Karaoke => Some("Karaoke"),
            RecordingKind::Cover => Some("Cover"),
            RecordingKind::Acapella => Some("Acapella"),
            RecordingKind::Remix => Some("Remix"),
            RecordingKind::Demo => Some("Demo"),
        }
    }

    /// The kind's name in a sentence: "a live recording", "an instrumental".
    pub fn described(self) -> &'static str {
        match self {
            RecordingKind::Studio => "the studio recording",
            RecordingKind::Live => "a live recording",
            RecordingKind::Instrumental => "an instrumental",
            RecordingKind::Karaoke => "a karaoke backing track",
            RecordingKind::Cover => "a cover",
            RecordingKind::Acapella => "an acapella",
            RecordingKind::Remix => "a remix",
            RecordingKind::Demo => "a demo",
        }
    }

    /// The kind a version marker names, as SongIdentity and the folder reading name them.
    pub fn of_marker(marker: &str) -> Option<RecordingKind> {
        Some(match marker {
            "live" | "unplugged" => RecordingKind::Live,
            "instrumental" => RecordingKind::Instrumental,
            "karaoke" => RecordingKind::Karaoke,
            "cover" => RecordingKind::Cover,
            "acapella" => RecordingKind::Acapella,
            "remix" | "dj mix" | "vip" | "rework" | "dub" => RecordingKind::Remix,
            "demo" => RecordingKind::Demo,
            _ => return None,
        })
    }

    /// The kinds a text's version markers name: "(Instrumental)", "- Karaoke Version", "(Acoustic
    /// Cover)", "(Tribute to Adele)". Words that are only part of a name ("Cover Me", "Karaoke
    /// Queen") are not markers.
    pub fn marked_in(text: &str) -> BTreeSet<RecordingKind> {
        if dotnet::is_blank(text) {
            return BTreeSet::new();
        }
        let mut found: BTreeSet<RecordingKind> = SongIdentity::parse_title(text, None)
            .versions
            .iter()
            .filter_map(|marker| RecordingKind::of_marker(marker))
            .collect();
        if MARKED_TRIBUTE.is_match(text) {
            found.insert(RecordingKind::Cover);
        }
        found
    }

    /// The kinds a text's version markers name, title words and loose words both read.
    pub fn named_in(text: &str) -> BTreeSet<RecordingKind> {
        if dotnet::is_blank(text) {
            return BTreeSet::new();
        }
        let mut found = RecordingKind::marked_in(text);
        found.extend(
            version_variant::requested(None, Some(text))
                .iter()
                .filter_map(|marker| RecordingKind::of_marker(marker)),
        );
        if LOOSE_COVERS.is_match(text) {
            found.insert(RecordingKind::Cover);
        }
        found
    }

    /// The kinds a request asks for, read from its title alone. An album is never a wish: a
    /// library copy's album may itself be the wrong tag that is being replaced.
    pub fn requested_by(title: &str) -> BTreeSet<RecordingKind> {
        RecordingKind::named_in(title)
    }
}

/// A "tribute to" opening a bracket or a dash tail, which names a cover: "Hello (Tribute to
/// Adele)", "Hello - A Tribute to Adele".
static MARKED_TRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:[\(\[\{]|\s[-\x{2013}]\s)\s*(?:a\s+)?tribute\s+to\b")
        .expect("a fixed pattern compiles")
});

/// What an album or a release of covers is called anywhere in its name: "A Tribute to Adele",
/// "Covers", "Cover Versions".
static LOOSE_COVERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\btribute\s+to\b|\bcovers\b|\bcover\s+versions\b").expect("a fixed pattern compiles")
});

/// What the music database adds about one recording beyond what AcoustID sends: its
/// disambiguation, and every release it is on with that release's status and its group's
/// secondary types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordingFacts {
    pub recording_id: String,
    pub title: String,
    pub disambiguation: Option<String>,
    pub length_ms: Option<i64>,
    pub releases: Vec<FactRelease>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FactRelease {
    pub title: String,
    pub status: Option<String>,
    pub group_title: Option<String>,
    pub group_secondary_types: Vec<String>,
}

/// The kind of take a recording is: its title's markers first, then the database's
/// disambiguation, then the releases it is on (every one of them live, or every one of them
/// named as instrumentals, karaoke, covers, remixes or demos).
pub fn kind_of(recording: &AcoustIdRecording, facts: Option<&RecordingFacts>) -> RecordingKind {
    if let Some(kind) = first_kind(&RecordingKind::named_in(&recording.title)) {
        return kind;
    }
    if let Some(facts) = facts {
        if let Some(kind) = facts.disambiguation.as_deref().and_then(note_kind) {
            return kind;
        }
        if let Some(kind) = kind_from_fact_releases(&facts.releases) {
            return kind;
        }
    }
    kind_from_releases(&recording.releases).unwrap_or(RecordingKind::Studio)
}

/// The kind a database note names: "instrumental", "karaoke", "cover", or a live take's
/// "live, 2007-04-08: Palacio de los Deportes".
fn note_kind(note: &str) -> Option<RecordingKind> {
    let mut kinds = RecordingKind::named_in(note);
    if live_version::mentions(Some(note)) {
        kinds.insert(RecordingKind::Live);
    }
    if NOTE_COVER.is_match(note) {
        kinds.insert(RecordingKind::Cover);
    }
    first_kind(&kinds)
}

/// A database note that calls the recording a cover: "cover", "Adele cover".
static NOTE_COVER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bcover(?:\s+version)?\b").expect("a fixed pattern compiles"));

/// The most telling kind of several: one that changes what is heard most comes first.
fn first_kind(kinds: &BTreeSet<RecordingKind>) -> Option<RecordingKind> {
    [
        RecordingKind::Karaoke,
        RecordingKind::Cover,
        RecordingKind::Instrumental,
        RecordingKind::Acapella,
        RecordingKind::Live,
        RecordingKind::Demo,
        RecordingKind::Remix,
    ]
    .into_iter()
    .find(|kind| kinds.contains(kind))
}

/// A kind every release names, by its own title or by being typed so.
fn kind_from_releases(releases: &[AcoustIdRelease]) -> Option<RecordingKind> {
    if releases.is_empty() {
        return None;
    }
    let per_release: Vec<BTreeSet<RecordingKind>> = releases
        .iter()
        .map(|release| {
            let mut kinds = release_secondary_kinds(&release.secondary_types);
            if let Some(title) = release.title.as_deref() {
                kinds.extend(RecordingKind::named_in(title));
            }
            kinds
        })
        .collect();
    shared_kind(&per_release)
}

fn kind_from_fact_releases(releases: &[FactRelease]) -> Option<RecordingKind> {
    if releases.is_empty() {
        return None;
    }
    let per_release: Vec<BTreeSet<RecordingKind>> = releases
        .iter()
        .map(|release| {
            let mut kinds = release_secondary_kinds(&release.group_secondary_types);
            kinds.extend(RecordingKind::named_in(&release.title));
            kinds
        })
        .collect();
    shared_kind(&per_release)
}

fn release_secondary_kinds(types: &[String]) -> BTreeSet<RecordingKind> {
    types
        .iter()
        .filter_map(|kind| match kind.to_ascii_lowercase().as_str() {
            "live" => Some(RecordingKind::Live),
            "remix" | "dj-mix" => Some(RecordingKind::Remix),
            "demo" => Some(RecordingKind::Demo),
            _ => None,
        })
        .collect()
}

/// The kind every release carries, the most telling first.
fn shared_kind(per_release: &[BTreeSet<RecordingKind>]) -> Option<RecordingKind> {
    let first = per_release.first()?;
    let shared: BTreeSet<RecordingKind> = first
        .iter()
        .copied()
        .filter(|kind| per_release.iter().all(|kinds| kinds.contains(kind)))
        .collect();
    first_kind(&shared)
}

/// How far a recording's listed length may be from the file's and still be the length that
/// decides between takes.
pub const DECISIVE_WITHIN_MS: i64 = 1_000;

/// How much further off every other take must be for that length to decide: at least this
/// much, and at least [`DECISIVE_RATIO`] times the best one's distance.
pub const DECISIVE_MARGIN_MS: i64 = 1_000;
pub const DECISIVE_RATIO: i64 = 4;

/// The one recording among several that agree on title and artist whose length alone says it
/// is the file: within [`DECISIVE_WITHIN_MS`] of the file, and every other agreeing recording
/// at least [`DECISIVE_MARGIN_MS`] further off and [`DECISIVE_RATIO`] times as far. None when no length is known or the lengths do
/// not decide. A recording listed twice (by id) counts once.
pub fn decided_by_length<'a>(
    agreeing: &[&'a AcoustIdRecording],
    file_ms: Option<i64>,
) -> Option<&'a AcoustIdRecording> {
    let file_ms = file_ms.filter(|ms| *ms > 0)?;
    let mut distances: Vec<(i64, &'a AcoustIdRecording)> = Vec::new();
    for recording in agreeing {
        let Some(length) = recording.duration_ms.filter(|ms| *ms > 0) else {
            continue;
        };
        if distances
            .iter()
            .any(|(_, seen)| dotnet::eq_ignore_case(&seen.recording_id, &recording.recording_id))
        {
            continue;
        }
        distances.push(((length - file_ms).abs(), recording));
    }
    distances.sort_by_key(|(distance, _)| *distance);
    let (best, chosen) = *distances.first()?;
    if best > DECISIVE_WITHIN_MS {
        return None;
    }
    // A lone recording with a length is not a choice between takes.
    if distances.len() < 2 {
        return None;
    }
    let decisive = distances[1..]
        .iter()
        .all(|(distance, _)| *distance >= best + DECISIVE_MARGIN_MS && *distance >= best * DECISIVE_RATIO);
    decisive.then_some(chosen)
}

/// The recording a file is, among the ones that agree with the request on title and artist:
/// the one its exact length decides, else the first of the kind asked for (the studio take for
/// a plain request), else the first. `kind` says what each recording is.
pub fn choose<'a>(
    agreeing: &[&'a AcoustIdRecording],
    file_ms: Option<i64>,
    wanted: &BTreeSet<RecordingKind>,
    kind: impl Fn(&AcoustIdRecording) -> RecordingKind,
) -> Option<(&'a AcoustIdRecording, bool)> {
    if let Some(decided) = decided_by_length(agreeing, file_ms) {
        return Some((decided, true));
    }
    let asked = |recording: &&&'a AcoustIdRecording| {
        let found = kind(recording);
        if wanted.is_empty() {
            found == RecordingKind::Studio
        } else {
            wanted.contains(&found)
        }
    };
    agreeing
        .iter()
        .find(asked)
        .or(agreeing.first())
        .map(|recording| (*recording, false))
}

/// Whether a recording's kind was never asked for.
pub fn unwanted(kind: RecordingKind, wanted: &BTreeSet<RecordingKind>) -> bool {
    kind != RecordingKind::Studio && !wanted.contains(&kind)
}

/// Which takes of a song a file may be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Takes {
    /// Any take: a song already in the library, which may be live or an instrumental on
    /// purpose.
    #[default]
    Any,
    /// The studio take and these kinds, nothing else.
    Only(BTreeSet<RecordingKind>),
}

impl Takes {
    /// The takes a download asked for by its title and its album: a live album lets a live
    /// take through ("Decade (live at the El Mocambo)"), and so does a title such as "Live
    /// Forever", generous on purpose. Kinds in `never` are refused even when asked for.
    pub fn for_request(title: Option<&str>, album: Option<&str>, never: &BTreeSet<RecordingKind>) -> Takes {
        let mut wanted: BTreeSet<RecordingKind> = BTreeSet::new();
        for text in [title, album].into_iter().flatten() {
            wanted.extend(RecordingKind::named_in(text));
        }
        if live_version::requested(title, album) {
            wanted.insert(RecordingKind::Live);
        }
        wanted.retain(|kind| !never.contains(kind));
        Takes::Only(wanted)
    }

    /// The kinds this allows besides the studio take; empty for [`Takes::Any`], whose choice
    /// still prefers the studio take.
    pub fn wanted(&self) -> BTreeSet<RecordingKind> {
        match self {
            Takes::Any => BTreeSet::new(),
            Takes::Only(kinds) => kinds.clone(),
        }
    }

    /// Whether a recording of this kind is refused.
    pub fn refuses(&self, kind: RecordingKind) -> bool {
        match self {
            Takes::Any => false,
            Takes::Only(kinds) => unwanted(kind, kinds),
        }
    }
}

/// The one recording a file's audio was found to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub recording_id: String,
    pub kind: RecordingKind,
    /// The file's exact length chose it among takes that share the fingerprint, so no other
    /// recording of the fingerprint may name the file's album.
    pub by_length: bool,
}

impl Identity {
    /// The identity a confirmed verification decided, None otherwise.
    pub fn of(verification: Option<&super::verification::VerificationResult>) -> Option<Identity> {
        let verification = verification?;
        if verification.verdict != super::verification::VerificationVerdict::Confirmed {
            return None;
        }
        let recording_id = verification
            .recording_id
            .clone()
            .filter(|id| !dotnet::is_blank(id))?;
        Some(Identity {
            recording_id,
            kind: verification.kind.unwrap_or(RecordingKind::Studio),
            by_length: verification.by_length,
        })
    }

    /// Whether another recording of the same fingerprint may still name the file's album: never
    /// when the exact length chose this one, and only one of the same kind otherwise (an album
    /// and a compilation often list the same audio under two recordings).
    pub fn admits(&self, recording_id: &str, kind: RecordingKind) -> bool {
        dotnet::eq_ignore_case(recording_id, &self.recording_id) || (!self.by_length && kind == self.kind)
    }

    /// Whether an album or release title names a take this recording is not: "The Black Parade
    /// (instrumentals)" for the studio take. `asked` are the kinds the request's title named.
    pub fn contradicted_by(&self, album: &str, asked: &BTreeSet<RecordingKind>) -> Option<RecordingKind> {
        RecordingKind::named_in(album)
            .into_iter()
            .find(|named| *named != self.kind && !asked.contains(named))
    }
}

#[cfg(test)]
#[path = "recording_identity_tests.rs"]
mod tests;
