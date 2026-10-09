//! The family's answer rules for lists. One pass over an answer (JSON, JSONP or XML) does what
//! applies to the caller:
//!
//! - a kid profile's answers lose explicit songs ([`super::kid_filter`]);
//! - the owner's answers lose the music in members' own libraries, and members' playlists,
//!   unless the owner chose to see them ([`super::owner_privacy`]);
//! - a member's answers mark the songs from their own library with `octoPersonal`
//!   ([`super::personal_marks`]).
//!
//! Counts follow what is left: `songCount` and `duration` on the album, playlist or directory
//! that held a removed song, and `albumCount` on the artist that held a removed album. An
//! answer nothing applies to, or that nothing changes, goes out byte for byte as it came.

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::Arc;

use bytes::Bytes;
use octo_core::common::dotnet::eq_ignore_case;
use octo_subsonic::xml::{XElement, XNode};
use serde_json::{Map, Value};

use super::owner_privacy::{PrivacyCache, PrivateView};
use super::rules::{AnswerContext, AnswerRule, AnswerRules};
use super::{kid_filter, owner_privacy, personal_marks};

/// Every endpoint these rules read. Each rule keeps to its own list within it.
pub const ENDPOINTS: &[&str] = &[
    "search",
    "search2",
    "search3",
    "getAlbum",
    "getAlbumList",
    "getAlbumList2",
    "getArtist",
    "getArtists",
    "getIndexes",
    "getMusicDirectory",
    "getMusicFolders",
    "getPlaylist",
    "getPlaylists",
    "getRandomSongs",
    "getSongsByGenre",
    "getSimilarSongs",
    "getSimilarSongs2",
    "getTopSongs",
    "getStarred",
    "getStarred2",
    "getPlayQueue",
    "getPlayQueueByIndex",
    "getNowPlaying",
    "getSong",
];

/// Whether `endpoint` is one of `list`, ignoring case.
pub(crate) fn listed(list: &[&str], endpoint: &str) -> bool {
    list.iter().any(|name| eq_ignore_case(name, endpoint))
}

/// What a node in an answer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A song: `song`, `entry`, `child` or `match` that is not a folder.
    Song,
    /// A folder in a directory listing (`child` or `entry` with `isDir`): an album or an artist.
    Folder,
    Album,
    Artist,
    Playlist,
    /// A music folder, which is one Navidrome library.
    MusicFolder,
}

/// One node as the rules see it.
#[derive(Debug, Clone, Copy)]
pub struct NodeFacts<'a> {
    pub kind: NodeKind,
    pub id: &'a str,
    /// `explicitStatus` as the answer wrote it, when it wrote one.
    pub explicit_status: Option<&'a str>,
    /// A playlist's `owner`.
    pub owner: Option<&'a str>,
}

/// What happens to one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    Drop,
    /// Kept, with `octoPersonal` set.
    Mark,
}

/// Decides each node.
pub type Judge<'a> = dyn Fn(&NodeFacts<'_>) -> Verdict + 'a;

/// The kind a member name holds, before a folder is told from a song.
fn kind_for(name: &str) -> Option<NodeKind> {
    match name {
        "song" | "entry" | "child" | "match" => Some(NodeKind::Song),
        "album" => Some(NodeKind::Album),
        "artist" => Some(NodeKind::Artist),
        "playlist" => Some(NodeKind::Playlist),
        "musicFolder" => Some(NodeKind::MusicFolder),
        _ => None,
    }
}

/// What was taken out of one list, for the counts of the node that holds it.
#[derive(Default)]
struct Removed {
    songs: i64,
    seconds: i64,
    albums: i64,
}

impl Removed {
    fn count(&mut self, kind: NodeKind, seconds: Option<i64>) {
        match kind {
            NodeKind::Song => {
                self.songs += 1;
                self.seconds += seconds.unwrap_or(0);
            }
            NodeKind::Album | NodeKind::Folder => self.albums += 1,
            _ => {}
        }
    }

    fn any(&self) -> bool {
        self.songs > 0 || self.albums > 0
    }

    /// The counts to lower, with how much.
    fn lowered(&self) -> [(&'static str, i64); 3] {
        [
            ("songCount", self.songs),
            ("duration", self.seconds),
            ("albumCount", self.albums),
        ]
    }
}

struct Walk<'a, 'b> {
    judge: &'a Judge<'b>,
    changed: bool,
}

// ---- JSON ----

fn json_text(node: &Map<String, Value>, name: &str) -> Option<String> {
    match node.get(name)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

impl Walk<'_, '_> {
    /// The verdict on one JSON node of `kind`, with the kind it turned out to be.
    fn judge_json(&self, kind: NodeKind, node: &Map<String, Value>) -> (NodeKind, Verdict) {
        let kind = if kind == NodeKind::Song && node.get("isDir").and_then(Value::as_bool) == Some(true) {
            NodeKind::Folder
        } else {
            kind
        };
        let id = json_text(node, "id").unwrap_or_default();
        let explicit = node.get("explicitStatus").and_then(Value::as_str);
        let owner = node.get("owner").and_then(Value::as_str);
        let facts = NodeFacts {
            kind,
            id: &id,
            explicit_status: explicit,
            owner,
        };
        (kind, (self.judge)(&facts))
    }

    /// Applies the verdict on a node in a list. False when it goes.
    fn keep_json(&mut self, kind: NodeKind, node: &mut Map<String, Value>, removed: &mut Removed) -> bool {
        let (kind, verdict) = self.judge_json(kind, node);
        match verdict {
            Verdict::Keep => true,
            Verdict::Mark => {
                self.mark_json(node);
                true
            }
            Verdict::Drop => {
                removed.count(kind, node.get("duration").and_then(Value::as_i64));
                self.changed = true;
                false
            }
        }
    }

    fn mark_json(&mut self, node: &mut Map<String, Value>) {
        if node.get("octoPersonal") != Some(&Value::Bool(true)) {
            node.insert("octoPersonal".into(), Value::Bool(true));
            self.changed = true;
        }
    }

    /// Walks one object. `top` is true for `subsonic-response`, whose members are what the
    /// answer is about (the album of getAlbum, the song of getSong): those are never removed.
    fn object(&mut self, object: &mut Map<String, Value>, top: bool) {
        let mut removed = Removed::default();
        let keys: Vec<String> = object.keys().cloned().collect();
        for key in keys {
            let kind = kind_for(&key);
            let mut gone = false;
            match (kind, object.get_mut(&key)) {
                (Some(kind), Some(Value::Array(items))) => {
                    items.retain_mut(|item| match item {
                        Value::Object(node) => self.keep_json(kind, node, &mut removed),
                        _ => true,
                    });
                }
                (Some(kind), Some(Value::Object(node))) if top => {
                    let (kind, verdict) = self.judge_json(kind, node);
                    if verdict == Verdict::Mark {
                        self.mark_json(node);
                    }
                    if kind != NodeKind::Song {
                        self.object(node, false);
                    }
                }
                (Some(kind), Some(Value::Object(node))) => {
                    gone = !self.keep_json(kind, node, &mut removed);
                }
                (None, Some(Value::Object(node))) => self.object(node, false),
                (None, Some(Value::Array(items))) => {
                    let group = key == "index";
                    items.retain_mut(|item| {
                        let Value::Object(node) = item else {
                            return true;
                        };
                        let before = group.then(|| artists_in_json(node));
                        self.object(node, false);
                        // An index letter whose every artist went goes too.
                        !matches!(before, Some(n) if n > 0 && artists_in_json(node) == 0)
                    });
                }
                _ => {}
            }
            if gone {
                object.shift_remove(&key);
            }
        }
        if removed.any() {
            for (name, by) in removed.lowered() {
                if by > 0
                    && let Some(count) = object.get(name).and_then(Value::as_i64)
                {
                    object.insert(name.into(), Value::from((count - by).max(0)));
                }
            }
        }
    }

    // ---- XML ----

    fn judge_xml(&self, kind: NodeKind, element: &XElement) -> (NodeKind, Verdict) {
        let kind = if kind == NodeKind::Song && element.attribute("isDir") == Some("true") {
            NodeKind::Folder
        } else {
            kind
        };
        let facts = NodeFacts {
            kind,
            id: element.attribute("id").unwrap_or(""),
            explicit_status: element.attribute("explicitStatus"),
            owner: element.attribute("owner"),
        };
        (kind, (self.judge)(&facts))
    }

    fn mark_xml(&mut self, element: &mut XElement) {
        if element.attribute("octoPersonal") != Some("true") {
            element.set_attr("octoPersonal", "true");
            self.changed = true;
        }
    }

    /// Walks one element, as [`Walk::object`] walks an object.
    fn element(&mut self, element: &mut XElement, top: bool) {
        let mut removed = Removed::default();
        let content = std::mem::take(&mut element.content);
        let mut kept = Vec::with_capacity(content.len());
        for node in content {
            let XNode::Element(mut child) = node else {
                kept.push(node);
                continue;
            };
            match kind_for(&child.name) {
                Some(kind) if top => {
                    let (kind, verdict) = self.judge_xml(kind, &child);
                    if verdict == Verdict::Mark {
                        self.mark_xml(&mut child);
                    }
                    if kind != NodeKind::Song {
                        self.element(&mut child, false);
                    }
                }
                Some(kind) => {
                    let (kind, verdict) = self.judge_xml(kind, &child);
                    match verdict {
                        Verdict::Keep => {}
                        Verdict::Mark => self.mark_xml(&mut child),
                        Verdict::Drop => {
                            let seconds = child.attribute("duration").and_then(|d| d.parse().ok());
                            removed.count(kind, seconds);
                            self.changed = true;
                            continue;
                        }
                    }
                }
                None => {
                    let before = (child.name == "index").then(|| artists_in_xml(&child));
                    self.element(&mut child, false);
                    if matches!(before, Some(n) if n > 0 && artists_in_xml(&child) == 0) {
                        continue;
                    }
                }
            }
            kept.push(XNode::Element(child));
        }
        element.content = kept;
        if removed.any() {
            for (name, by) in removed.lowered() {
                if by > 0
                    && let Some(count) = element.attribute(name).and_then(|c| c.parse::<i64>().ok())
                {
                    element.set_attr(name, (count - by).max(0).to_string());
                }
            }
        }
    }
}

fn artists_in_json(node: &Map<String, Value>) -> usize {
    match node.get("artist") {
        Some(Value::Array(items)) => items.len(),
        Some(Value::Object(_)) => 1,
        _ => 0,
    }
}

fn artists_in_xml(element: &XElement) -> usize {
    element.elements().filter(|e| e.name == "artist").count()
}

/// The answer with `judge` applied to every node, in the shape it came in: JSON, JSONP
/// (`callback(...)`) or XML (its declaration kept). None when nothing changed or the answer
/// cannot be read, so the caller sends the bytes it has.
pub fn rewrite_answer(body: &[u8], judge: &Judge<'_>) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(body).ok()?;
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    let mut walk = Walk {
        judge,
        changed: false,
    };
    if trimmed.starts_with('<') {
        let (declaration, rest) = match trimmed.strip_prefix("<?xml") {
            Some(after) => {
                let end = after.find("?>")? + "<?xml".len() + 2;
                let rest = &trimmed[end..];
                let gap = rest.len() - rest.trim_start().len();
                (&trimmed[..end + gap], rest.trim_start())
            }
            None => ("", trimmed),
        };
        let mut root = XElement::parse(rest).ok()?;
        walk.element(&mut root, true);
        if !walk.changed {
            return None;
        }
        return Some(format!("{declaration}{}", root.to_xml_string()).into_bytes());
    }
    let (head, json, tail) = if trimmed.starts_with('{') {
        ("", trimmed, "")
    } else {
        // JSONP: `callback({...});`.
        let open = trimmed.find('(')?;
        let close = trimmed.rfind(')')?;
        if close <= open {
            return None;
        }
        (&trimmed[..=open], &trimmed[open + 1..close], &trimmed[close..])
    };
    let mut root: Value = serde_json::from_str(json).ok()?;
    if let Some(Value::Object(response)) = root.get_mut("subsonic-response") {
        walk.object(response, true);
    }
    if !walk.changed {
        return None;
    }
    let written = serde_json::to_string(&root).ok()?;
    Some(format!("{head}{written}{tail}").into_bytes())
}

/// Whether any song in the answer is one `pick` picks (the answer is not changed).
pub fn any_song(body: &[u8], pick: impl Fn(&NodeFacts<'_>) -> bool) -> bool {
    let found = Cell::new(false);
    let judge = |facts: &NodeFacts<'_>| {
        if facts.kind == NodeKind::Song && pick(facts) {
            found.set(true);
        }
        Verdict::Keep
    };
    let _ = rewrite_answer(body, &judge);
    found.get()
}

/// Registers the rules for every endpoint in [`ENDPOINTS`].
pub fn register(rules: &AnswerRules) {
    let privacy = Arc::new(PrivacyCache::default());
    let rule: AnswerRule =
        Arc::new(move |context: &AnswerContext<'_>, body: Bytes| lists_rule(context, body, &privacy));
    for endpoint in ENDPOINTS {
        rules.register(endpoint, rule.clone());
    }
}

/// The three rules over one answer.
fn lists_rule(context: &AnswerContext<'_>, body: Bytes, privacy: &PrivacyCache) -> Bytes {
    let state = context.state;
    let settings = state.settings.current();
    if !settings.family.enabled {
        return body;
    }
    let caller = context.caller;
    let endpoint = context.endpoint;
    let kid = kid_filter::applies(caller, endpoint).then_some(settings.family.kid_unrated_is_explicit);
    let owner = owner_privacy::applies(caller, endpoint, &settings.family);
    let personal = personal_marks::personal_library(&state.family_store, caller);
    if kid.is_none() && owner.is_none() && personal.is_none() {
        return body;
    }
    let index = &state.library_index;

    if let Some(unrated) = kid
        && eq_ignore_case(endpoint, "getSong")
        && any_song(&body, |facts| {
            kid_filter::song_is_explicit(index, facts.id, facts.explicit_status, unrated)
        })
    {
        return kid_filter::not_found(state, context);
    }

    let view: Option<Arc<PrivateView>> = owner
        .filter(|o| o.libraries)
        .map(|_| privacy.view(index, state.library_scope.personal_library_ids()));
    let members: HashSet<String> = if owner.is_some_and(|o| o.playlists) {
        state
            .family_store
            .members()
            .into_iter()
            .map(|m| m.username.to_lowercase())
            .collect()
    } else {
        HashSet::new()
    };

    let judge = |facts: &NodeFacts<'_>| -> Verdict {
        match facts.kind {
            NodeKind::Song => {
                let indexed = index.song(facts.id);
                if let Some(unrated) = kid
                    && kid_filter::is_explicit(
                        indexed.as_ref().map(|s| s.explicit_status.as_str()),
                        facts.explicit_status,
                        unrated,
                    )
                {
                    return Verdict::Drop;
                }
                let library = indexed.as_ref().and_then(|s| s.library_id);
                if view.as_ref().is_some_and(|v| v.hides_song(library)) {
                    return Verdict::Drop;
                }
                if personal_marks::is_personal(library, personal) {
                    return Verdict::Mark;
                }
                Verdict::Keep
            }
            NodeKind::Playlist => {
                if owner_privacy::hides_playlist(&members, facts.owner) {
                    Verdict::Drop
                } else {
                    Verdict::Keep
                }
            }
            kind => {
                if view.as_ref().is_some_and(|v| v.hides(kind, facts.id)) {
                    Verdict::Drop
                } else {
                    Verdict::Keep
                }
            }
        }
    };
    match rewrite_answer(&body, &judge) {
        Some(rewritten) => Bytes::from(rewritten),
        None => body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn drop_ids<'a>(ids: &'a [&'a str]) -> impl Fn(&NodeFacts<'_>) -> Verdict + 'a {
        move |facts| {
            if ids.contains(&facts.id) {
                Verdict::Drop
            } else {
                Verdict::Keep
            }
        }
    }

    fn json_of(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("JSON")
    }

    #[test]
    fn nothing_changed_sends_the_same_bytes() {
        let body = br#"{"subsonic-response":{"status":"ok","searchResult3":{"song":[{"id":"a"}]}}}"#;
        assert_eq!(rewrite_answer(body, &drop_ids(&[])), None);
        let xml = br#"<?xml version="1.0"?><subsonic-response status="ok"><randomSongs><song id="a"/></randomSongs></subsonic-response>"#;
        assert_eq!(rewrite_answer(xml, &drop_ids(&["zzz"])), None);
    }

    #[test]
    fn json_lists_lose_nodes_and_counts_follow() {
        let body = json!({"subsonic-response": {"status": "ok", "album": {
            "id": "al1", "songCount": 3, "duration": 600,
            "song": [
                {"id": "a", "duration": 200},
                {"id": "b", "duration": 150},
                {"id": "c", "duration": 250}
            ]
        }}})
        .to_string();
        let out = json_of(&rewrite_answer(body.as_bytes(), &drop_ids(&["b"])).expect("changed"));
        let album = &out["subsonic-response"]["album"];
        assert_eq!(album["songCount"], 2);
        assert_eq!(album["duration"], 450);
        let ids: Vec<&str> = album["song"]
            .as_array()
            .expect("list")
            .iter()
            .map(|s| s["id"].as_str().expect("id"))
            .collect();
        assert_eq!(ids, ["a", "c"]);
    }

    #[test]
    fn the_answer_subject_is_never_removed() {
        let body = json!({"subsonic-response": {"status": "ok", "song": {"id": "a"}}}).to_string();
        assert_eq!(rewrite_answer(body.as_bytes(), &drop_ids(&["a"])), None);
    }

    #[test]
    fn marks_are_written_in_json_and_xml() {
        let mark = |facts: &NodeFacts<'_>| {
            if facts.id == "mine" {
                Verdict::Mark
            } else {
                Verdict::Keep
            }
        };
        let body = br#"{"subsonic-response":{"song":{"id":"mine"}}}"#;
        let out = json_of(&rewrite_answer(body, &mark).expect("changed"));
        assert_eq!(out["subsonic-response"]["song"]["octoPersonal"], true);
        let xml = br#"<subsonic-response xmlns="http://subsonic.org/restapi" status="ok"><playlist id="p"><entry id="mine"/><entry id="other"/></playlist></subsonic-response>"#;
        let out = String::from_utf8(rewrite_answer(xml, &mark).expect("changed")).expect("text");
        assert!(
            out.contains(r#"<entry id="mine" octoPersonal="true" />"#),
            "{out}"
        );
        assert!(out.contains(r#"<entry id="other" />"#), "{out}");
    }

    #[test]
    fn jsonp_is_unwrapped_and_wrapped_again() {
        let body = br#"cb({"subsonic-response":{"randomSongs":{"song":[{"id":"a"},{"id":"b"}]}}});"#;
        let out = String::from_utf8(rewrite_answer(body, &drop_ids(&["a"])).expect("changed")).expect("text");
        assert!(out.starts_with("cb({"), "{out}");
        assert!(out.ends_with("});"), "{out}");
        assert!(!out.contains(r#""id":"a""#), "{out}");
        assert!(out.contains(r#""id":"b""#), "{out}");
    }

    #[test]
    fn xml_declaration_is_kept_and_counts_follow() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<subsonic-response status="ok"><artist id="ar" albumCount="2"><album id="x"/><album id="y"/></artist></subsonic-response>"#;
        let out = String::from_utf8(rewrite_answer(xml, &drop_ids(&["x"])).expect("changed")).expect("text");
        assert!(
            out.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<subsonic-response"),
            "{out}"
        );
        assert!(out.contains(r#"albumCount="1""#), "{out}");
        assert!(!out.contains(r#"id="x""#), "{out}");
    }

    #[test]
    fn folders_and_index_letters() {
        let body = json!({"subsonic-response": {"indexes": {"index": [
            {"name": "A", "artist": [{"id": "ar1"}]},
            {"name": "B", "artist": [{"id": "ar2"}, {"id": "ar3"}]}
        ], "child": [{"id": "dir", "isDir": true}, {"id": "s"}]}}})
        .to_string();
        let seen = std::cell::RefCell::new(Vec::new());
        let judge = |facts: &NodeFacts<'_>| {
            seen.borrow_mut().push((facts.id.to_string(), facts.kind));
            if ["ar1", "ar2", "dir"].contains(&facts.id) {
                Verdict::Drop
            } else {
                Verdict::Keep
            }
        };
        let out = json_of(&rewrite_answer(body.as_bytes(), &judge).expect("changed"));
        let indexes = &out["subsonic-response"]["indexes"];
        assert_eq!(indexes["index"].as_array().expect("list").len(), 1);
        assert_eq!(indexes["index"][0]["artist"][0]["id"], "ar3");
        assert_eq!(indexes["child"].as_array().expect("list").len(), 1);
        assert!(seen.borrow().contains(&("dir".into(), NodeKind::Folder)));
        assert!(seen.borrow().contains(&("s".into(), NodeKind::Song)));
    }

    #[test]
    fn songs_are_not_walked_into() {
        // A song's contributors name artists; those are not list items.
        let body = json!({"subsonic-response": {"randomSongs": {"song": [
            {"id": "s", "contributors": [{"role": "composer", "artist": {"id": "ar1"}}]}
        ]}}})
        .to_string();
        assert_eq!(rewrite_answer(body.as_bytes(), &drop_ids(&["ar1"])), None);
    }

    #[test]
    fn music_folder_ids_are_numbers_in_json() {
        let body = json!({"subsonic-response": {"musicFolders": {"musicFolder": [
            {"id": 1, "name": "Music"}, {"id": 3, "name": "Alex's library"}
        ]}}})
        .to_string();
        let out = json_of(&rewrite_answer(body.as_bytes(), &drop_ids(&["3"])).expect("changed"));
        assert_eq!(
            out["subsonic-response"]["musicFolders"]["musicFolder"],
            json!([{"id": 1, "name": "Music"}])
        );
    }

    #[test]
    fn unreadable_answers_are_left_alone() {
        assert_eq!(rewrite_answer(b"not an answer", &drop_ids(&["a"])), None);
        assert_eq!(rewrite_answer(&[0xff, 0xfe], &drop_ids(&["a"])), None);
    }
}
