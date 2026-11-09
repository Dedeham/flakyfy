//! The list of unsure songs keeps what was heard across a restart, reopens a changed file, and
//! its rules are suggested only by unanimous answers, act only once accepted, and stop when
//! someone disagrees with them.

use chrono::TimeZone;
use serde_json::Value;

use super::*;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 9, 20, 0, 0).unwrap()
}

fn video(id: &str, bits: f64) -> HeardVideo {
    HeardVideo {
        video_id: id.into(),
        title: Some("Song".into()),
        artist: Some("Someone Else".into()),
        album: None,
        seconds: Some(190),
        bit_error: bits,
        offset_seconds: 0.0,
        trusted: false,
        reason: UnsureReason::Name,
        rejected: false,
    }
}

fn song() -> Song {
    Song {
        artist: "Demo Artist".into(),
        title: "Song".into(),
        album: "Demo Album".into(),
        duration: Some(190),
        ..Default::default()
    }
}

/// `count` songs of one bucket, answered `label` by a person.
fn answered(store: &UnsureStore, from: usize, count: usize, bits: f64, label: UnsureLabel) {
    for n in from..from + count {
        store.record(&format!("{n}.flac"), &song(), "1:1", video("v", bits), now());
        let id = store.song_at(&format!("{n}.flac")).unwrap().id;
        store.decide(id, Some(label), None, "bob", now());
    }
}

#[test]
fn a_song_stays_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("youtube-unsure.json");
    let store = UnsureStore::new(Some(path.clone()));
    store.record("a.flac", &song(), "1:1", video("v", 0.31), now());
    store.flush();
    drop(store);

    let store = UnsureStore::new(Some(path));
    let kept = store.song_at("a.flac").expect("kept");
    assert_eq!(kept.videos[0].video_id, "v");
    assert_eq!(kept.artist, "Demo Artist");
}

#[test]
fn a_corrupt_list_is_set_aside_and_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("youtube-unsure.json");
    std::fs::write(&path, "{not json").unwrap();

    let store = UnsureStore::new(Some(path.clone()));

    assert!(store.read(|state| state.songs.is_empty()));
    assert!(!path.exists());
}

#[test]
fn a_changed_file_is_a_new_question() {
    let store = UnsureStore::new(None);
    store.record("a.flac", &song(), "1:1", video("old", 0.31), now());
    let id = store.song_at("a.flac").unwrap().id;
    store.decide(id, Some(UnsureLabel::Right), None, "bob", now());
    assert_eq!(
        store.settled_stamps().get("a.flac").map(String::as_str),
        Some("1:1")
    );

    store.record("a.flac", &song(), "2:2", video("new", 0.4), now());

    let kept = store.song_at("a.flac").unwrap();
    assert!(kept.decision.is_none());
    assert_eq!(kept.videos.len(), 1);
    assert_eq!(kept.videos[0].video_id, "new");
    assert!(store.settled_stamps().is_empty());
}

#[test]
fn an_answer_on_an_unchanged_file_is_not_touched_by_a_later_look() {
    let store = UnsureStore::new(None);
    store.record("a.flac", &song(), "1:1", video("v", 0.31), now());
    let id = store.song_at("a.flac").unwrap().id;
    store.decide(id, Some(UnsureLabel::Version), None, "bob", now());

    store.record("a.flac", &song(), "1:1", video("v", 0.33), now());

    let kept = store.song_at("a.flac").unwrap();
    assert_eq!(kept.decision.unwrap().label, UnsureLabel::Version);
    assert_eq!(kept.videos.len(), 1);
    assert_eq!(kept.videos[0].bit_error, 0.33);
}

#[test]
fn cant_tell_leaves_the_song_open_and_an_answer_can_be_taken_back() {
    let store = UnsureStore::new(None);
    store.record("a.flac", &song(), "1:1", video("v", 0.31), now());
    let id = store.song_at("a.flac").unwrap().id;

    store.decide(id, Some(UnsureLabel::CantTell), None, "bob", now());
    assert!(!store.song(id).unwrap().settled());

    store.decide(id, Some(UnsureLabel::Right), None, "bob", now());
    assert!(store.song(id).unwrap().settled());

    store.decide(id, None, None, "bob", now());
    assert!(store.song(id).unwrap().decision.is_none());
}

#[test]
fn a_video_turned_down_stays_turned_down_and_the_song_stays_open() {
    let store = UnsureStore::new(None);
    store.record("a.flac", &song(), "1:1", video("first", 0.31), now());
    let id = store.song_at("a.flac").unwrap().id;
    store.add_video(id, video("second", 0.47));

    store.decide(id, Some(UnsureLabel::PreviewWrong), Some("first"), "bob", now());
    store.record("a.flac", &song(), "1:1", video("first", 0.3), now());

    let kept = store.song(id).unwrap();
    assert!(kept.decision.is_none());
    assert!(kept.videos[0].rejected);
    assert_eq!(kept.bucket().as_deref(), Some("name|search|45"));
}

#[test]
fn turning_down_a_video_of_no_such_song_is_nothing() {
    let store = UnsureStore::new(None);
    assert!(
        store
            .decide(9, Some(UnsureLabel::Right), None, "bob", now())
            .is_none()
    );
}

#[test]
fn the_cases_in_the_docs_say_when_a_rule_is_suggested() {
    let text = include_str!("../../../../../docs/unsure-rule-cases.json");
    let cases: Value = serde_json::from_str(text).unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let note = case["note"].as_str().unwrap();
        let store = UnsureStore::new(None);
        let reason = match case["reason"].as_str().unwrap() {
            "between" => UnsureReason::Between,
            "length" => UnsureReason::Length,
            _ => UnsureReason::Name,
        };
        let heard = HeardVideo {
            reason,
            trusted: reason == UnsureReason::Between,
            album: case["album"].as_bool().unwrap().then(|| "Demo Album".to_string()),
            ..video("v", case["bits"].as_f64().unwrap())
        };
        let mut n = 0;
        for (label, count) in case["answers"].as_object().unwrap() {
            let label = match label.as_str() {
                "right" => UnsureLabel::Right,
                "wrong" => UnsureLabel::Wrong,
                "version" => UnsureLabel::Version,
                _ => UnsureLabel::CantTell,
            };
            for _ in 0..count.as_u64().unwrap() {
                n += 1;
                let path = format!("{n}.flac");
                store.record(&path, &song(), "1:1", heard.clone(), now());
                let id = store.song_at(&path).unwrap().id;
                store.decide(id, Some(label), None, "bob", now());
            }
        }
        let mut kinds: Vec<String> = store
            .read(suggestions)
            .iter()
            .map(|found| found.kind.name().to_string())
            .collect();
        kinds.sort();
        let expect: Vec<String> = case["expect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|kind| kind.as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, expect, "{note}");
    }
}

#[test]
fn a_suggestion_does_nothing_until_someone_accepts_it() {
    let store = UnsureStore::new(None);
    answered(&store, 0, 10, 0.31, UnsureLabel::Right);

    assert_eq!(
        store.record("new.flac", &song(), "1:1", video("v", 0.31), now()),
        RuleAction::Wait
    );
    assert!(store.song_at("new.flac").unwrap().decision.is_none());

    let id = store.read(suggestions)[0].id.clone();
    assert!(store.accept_rule(&id, now()));

    assert_eq!(
        store.record("next.flac", &song(), "1:1", video("v", 0.31), now()),
        RuleAction::Settled
    );
    let decision = store.song_at("next.flac").unwrap().decision.unwrap();
    assert_eq!(decision.label, UnsureLabel::Right);
    assert_eq!(decision.rule.as_deref(), Some(id.as_str()));
    assert!(store.read(suggestions).is_empty());
}

#[test]
fn a_rule_only_reaches_its_own_bucket() {
    let store = UnsureStore::new(None);
    answered(&store, 0, 10, 0.31, UnsureLabel::Right);
    let id = store.read(suggestions)[0].id.clone();
    store.accept_rule(&id, now());

    assert_eq!(
        store.record("louder.flac", &song(), "1:1", video("v", 0.36), now()),
        RuleAction::Wait
    );
}

#[test]
fn what_a_rule_decided_is_not_an_answer_that_teaches() {
    let store = UnsureStore::new(None);
    answered(&store, 0, 10, 0.31, UnsureLabel::Right);
    let id = store.read(suggestions)[0].id.clone();
    store.accept_rule(&id, now());
    for n in 0..30 {
        store.record(&format!("r{n}.flac"), &song(), "1:1", video("v", 0.31), now());
    }

    let right: usize = store.read(report).iter().map(|row| row.right).sum();

    assert_eq!(right, 10);
}

#[test]
fn disagreeing_with_a_rule_turns_it_off_and_opens_what_it_decided() {
    let store = UnsureStore::new(None);
    answered(&store, 0, 10, 0.31, UnsureLabel::Right);
    let id = store.read(suggestions)[0].id.clone();
    store.accept_rule(&id, now());
    store.record("a.flac", &song(), "1:1", video("v", 0.31), now());
    store.record("b.flac", &song(), "1:1", video("v", 0.31), now());
    let a = store.song_at("a.flac").unwrap().id;

    store.decide(a, Some(UnsureLabel::Wrong), None, "bob", now());

    assert_eq!(store.read(|s| s.rules[0].state), RuleState::Contradicted);
    assert_eq!(store.song(a).unwrap().decision.unwrap().rule, None);
    assert!(store.song_at("b.flac").unwrap().decision.is_none());
    assert_eq!(
        store.record("c.flac", &song(), "1:1", video("v", 0.31), now()),
        RuleAction::Wait
    );
    assert!(store.read(suggestions).is_empty());
}

#[test]
fn a_person_answering_differently_in_the_bucket_turns_the_rule_off_too() {
    let store = UnsureStore::new(None);
    answered(&store, 0, 10, 0.31, UnsureLabel::Right);
    let id = store.read(suggestions)[0].id.clone();
    store.accept_rule(&id, now());
    let first = store.song_at("0.flac").unwrap().id;

    store.decide(first, Some(UnsureLabel::Version), None, "bob", now());

    assert_eq!(store.read(|s| s.rules[0].state), RuleState::Contradicted);
}

#[test]
fn cant_tell_does_not_contradict_a_rule() {
    let store = UnsureStore::new(None);
    answered(&store, 0, 10, 0.31, UnsureLabel::Right);
    let id = store.read(suggestions)[0].id.clone();
    store.accept_rule(&id, now());
    store.record("a.flac", &song(), "1:1", video("v", 0.31), now());
    let a = store.song_at("a.flac").unwrap().id;

    store.decide(a, Some(UnsureLabel::CantTell), None, "bob", now());

    assert_eq!(store.read(|s| s.rules[0].state), RuleState::Active);
}

#[test]
fn a_rule_turned_down_is_not_suggested_again() {
    let store = UnsureStore::new(None);
    answered(&store, 0, 10, 0.31, UnsureLabel::Right);
    let id = store.read(suggestions)[0].id.clone();

    assert!(store.reject_rule(&id, now()));

    assert!(store.read(suggestions).is_empty());
    assert!(!store.accept_rule(&id, now()));
}

#[test]
fn accepting_what_no_longer_stands_does_nothing() {
    let store = UnsureStore::new(None);
    assert!(!store.accept_rule("name|search|30|settle", now()));
    assert!(store.read(|s| s.rules.is_empty()));
}

#[test]
fn replacing_needs_two_videos_all_clearly_apart() {
    let store = UnsureStore::new(None);
    store.record("a.flac", &song(), "1:1", video("one", 0.47), now());
    let id = store.song_at("a.flac").unwrap().id;
    assert_eq!(videos_disagree(&store.song(id).unwrap(), 0.42), None);

    store.add_video(id, video("two", 0.44));
    assert_eq!(videos_disagree(&store.song(id).unwrap(), 0.42), Some(0.44));

    store.add_video(id, video("three", 0.05));
    assert_eq!(videos_disagree(&store.song(id).unwrap(), 0.42), None);

    store.decide(id, Some(UnsureLabel::PreviewWrong), Some("three"), "bob", now());
    assert_eq!(videos_disagree(&store.song(id).unwrap(), 0.42), Some(0.44));
}
