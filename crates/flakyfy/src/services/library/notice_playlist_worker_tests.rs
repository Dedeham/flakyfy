//! The reconcile decision for one person's notice playlist, driven directly. And what goes back
//! to AcoustID when a person keeps a track.

use std::sync::atomic::{AtomicI32, Ordering};

use chrono::{TimeDelta, Utc};
use flakyfy_core::models::domain::Song;

use super::*;

static ORDER: AtomicI32 = AtomicI32::new(0);

fn entry(id: &str, state: NoticeState, group: Option<&str>, minutes_ago: i64) -> NoticeEntry {
    NoticeEntry {
        key: format!("k-{id}"),
        username: "alice".into(),
        navidrome_id: Some(id.to_string()),
        state,
        group_key: group.map(str::to_string),
        order: ORDER.fetch_add(1, Ordering::SeqCst) + 1,
        created_utc: Utc::now() - TimeDelta::minutes(minutes_ago),
        ..Default::default()
    }
}

fn present(ids: &[&str]) -> HashSet<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

fn added(plan: &NoticePlan) -> Vec<&str> {
    plan.add
        .iter()
        .filter_map(|e| e.navidrome_id.as_deref())
        .collect()
}

#[test]
fn plan_a_queued_track_gone_from_the_playlist_is_a_dismissal() {
    let plan = NoticeReconcile::plan(
        &[
            entry("a", NoticeState::Queued, None, 0),
            entry("b", NoticeState::Queued, None, 0),
        ],
        &present(&["b"]),
        10,
    );

    assert_eq!(plan.dismiss, ["k-a"]);
    assert!(plan.remove.is_empty());
    assert!(plan.add.is_empty());
}

/// A restart between adding a track and recording it must not add it twice.
#[test]
fn plan_a_waiting_track_already_in_the_playlist_is_adopted_not_added_again() {
    let plan = NoticeReconcile::plan(&[entry("a", NoticeState::Waiting, None, 0)], &present(&["a"]), 10);

    assert_eq!(plan.adopt, ["k-a"]);
    assert!(plan.add.is_empty());
}

#[test]
fn plan_a_settled_track_still_listed_is_taken_off() {
    for state in [
        NoticeState::Kept,
        NoticeState::Acted,
        NoticeState::Expired,
        NoticeState::Dismissed,
    ] {
        let plan = NoticeReconcile::plan(&[entry("a", state, None, 0)], &present(&["a"]), 10);

        assert_eq!(plan.remove, ["a"], "{state:?}");
        assert!(plan.dismiss.is_empty(), "{state:?}");
    }
}

#[test]
fn plan_fills_only_the_room_left_oldest_first() {
    let entries = [
        entry("asked", NoticeState::Queued, None, 0),
        entry("new", NoticeState::Waiting, None, 1),
        entry("old", NoticeState::Waiting, None, 30),
        entry("older", NoticeState::Waiting, None, 60),
    ];

    let plan = NoticeReconcile::plan(&entries, &present(&["asked"]), 3);

    assert_eq!(added(&plan), ["older", "old"]);
}

#[test]
fn plan_a_full_playlist_adds_nothing() {
    let plan = NoticeReconcile::plan(
        &[
            entry("asked", NoticeState::Queued, None, 0),
            entry("new", NoticeState::Waiting, None, 0),
        ],
        &present(&["asked"]),
        1,
    );

    assert!(plan.add.is_empty());
}

#[test]
fn plan_a_waiting_track_without_a_navidrome_id_waits() {
    let waiting = NoticeEntry {
        navidrome_id: None,
        ..entry("a", NoticeState::Waiting, None, 0)
    };

    assert!(
        NoticeReconcile::plan(&[waiting], &present(&[]), 10)
            .add
            .is_empty()
    );
}

/// A duplicate pair only makes sense together: it goes in whole or waits.
#[test]
fn plan_a_group_that_does_not_fit_waits_while_a_single_track_takes_the_room() {
    let entries = [
        entry("a1", NoticeState::Waiting, Some("g"), 60),
        entry("a2", NoticeState::Waiting, Some("g"), 60),
        entry("single", NoticeState::Waiting, None, 1),
    ];

    let plan = NoticeReconcile::plan(&entries, &present(&[]), 1);

    assert_eq!(added(&plan), ["single"]);
}

#[test]
fn plan_a_group_that_fits_goes_in_whole_in_its_order() {
    let entries = [
        entry("a1", NoticeState::Waiting, Some("g"), 0),
        entry("a2", NoticeState::Waiting, Some("g"), 0),
    ];

    let plan = NoticeReconcile::plan(&entries, &present(&[]), 2);

    assert_eq!(added(&plan), ["a1", "a2"]);
}

// ---- What goes back to AcoustID ---------------------------------------------------------

#[test]
fn may_submit_needs_consent_both_keys_and_a_real_run() {
    let cases = [
        (true, false, "app", "user", true),
        (false, false, "app", "user", false),
        (true, true, "app", "user", false),
        (true, false, "", "user", false),
        (true, false, "app", "", false),
    ];
    for (consent, dry_run, app_key, user_key, expected) in cases {
        let settings = LibraryActionSettings {
            dry_run,
            ..Default::default()
        };
        let soulseek = SoulseekSettings {
            submit_confirmed_fingerprints: consent,
            acoust_id_api_key: app_key.into(),
            acoust_id_user_api_key: user_key.into(),
            ..Default::default()
        };
        assert_eq!(
            NoticePlaylistWorker::may_submit(&settings, &soulseek),
            expected,
            "{consent} {dry_run} {app_key:?} {user_key:?}"
        );
    }
}

/// A track AcoustID confidently named as something else was kept for some reason other than
/// "AcoustID is missing this", and a shortened fingerprint must never land beside the standard
/// ones.
#[test]
fn submittable_only_what_acoust_id_could_not_place_at_the_standard_length() {
    let cases = [
        (InconclusiveReason::NoEntry, 120, true),
        (InconclusiveReason::BelowThreshold, 120, true),
        (InconclusiveReason::SourceDisagreed, 120, false),
        (InconclusiveReason::NoEntry, 60, false),
    ];
    for (cause, seconds, expected) in cases {
        let entry = NoticeEntry {
            cause,
            fingerprint: Some("AQADtEqk".into()),
            duration_seconds: 330,
            ..Default::default()
        };
        let soulseek = SoulseekSettings {
            fingerprint_seconds: seconds,
            ..Default::default()
        };
        assert_eq!(
            NoticePlaylistWorker::submittable(&entry, &soulseek),
            expected,
            "{cause:?} {seconds}"
        );
    }
}

#[test]
fn submittable_without_a_length_or_a_fingerprint_is_not() {
    let soulseek = SoulseekSettings {
        fingerprint_seconds: 120,
        ..Default::default()
    };

    assert!(!NoticePlaylistWorker::submittable(
        &NoticeEntry {
            cause: InconclusiveReason::NoEntry,
            fingerprint: Some("AQADtEqk".into()),
            ..Default::default()
        },
        &soulseek
    ));
    assert!(!NoticePlaylistWorker::submittable(
        &NoticeEntry {
            cause: InconclusiveReason::NoEntry,
            duration_seconds: 330,
            ..Default::default()
        },
        &soulseek
    ));
}

fn hand_over_settings(allowed: &[&str]) -> LibraryActionSettings {
    LibraryActionSettings {
        review_enabled: true,
        allowed_users: allowed.iter().map(|user| user.to_string()).collect(),
        ..Default::default()
    }
}

fn review_of(settings: &LibraryActionSettings, owners: &[&str]) -> Vec<PlaylistRow> {
    owners
        .iter()
        .map(|owner| PlaylistRow {
            id: format!("p-{owner}"),
            name: settings.notice_title(NoticeKind::Review),
            owner: owner.to_string(),
        })
        .collect()
}

fn question(queue: &NoticeQueue, user: &str, file: &str) {
    assert!(queue.add_library_question(user, &format!("/music/{file}"), &Song::default(), "why"));
}

fn open_for(queue: &NoticeQueue, user: &str) -> Vec<NoticeEntry> {
    let mut open = queue.for_user(user, NoticeKind::Review);
    open.retain(NoticeEntry::is_open);
    open
}

#[test]
fn a_question_for_someone_without_a_review_playlist_goes_to_the_first_allowed_person() {
    let queue = NoticeQueue::new();
    question(&queue, "service", "a.flac");
    let settings = hand_over_settings(&["alice", "service"]);

    NoticeReconcile::hand_over(&queue, &settings, &review_of(&settings, &["alice"]));

    let moved = open_for(&queue, "alice");
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].key, NoticeQueue::review_key("alice", "/music/a.flac"));
    assert_eq!(moved[0].state, NoticeState::Waiting);
    assert!(queue.for_user("service", NoticeKind::Review).is_empty());
    assert_eq!(
        queue.open_count(NoticeOrigin::LibrarySweep),
        1,
        "moved, not copied"
    );
}

#[test]
fn a_question_someone_could_see_stays_with_them() {
    let queue = NoticeQueue::new();
    question(&queue, "bob", "a.flac");
    let settings = hand_over_settings(&["alice", "bob"]);

    NoticeReconcile::hand_over(&queue, &settings, &review_of(&settings, &["alice", "bob"]));

    assert_eq!(open_for(&queue, "bob").len(), 1);
    assert!(open_for(&queue, "alice").is_empty());
}

#[test]
fn a_question_already_shown_to_someone_off_the_allowlist_is_asked_again() {
    let queue = NoticeQueue::new();
    question(&queue, "former", "a.flac");
    queue.mark_queued(&[NoticeQueue::review_key("former", "/music/a.flac")]);
    let settings = hand_over_settings(&["alice"]);

    NoticeReconcile::hand_over(&queue, &settings, &review_of(&settings, &["alice", "former"]));

    assert_eq!(open_for(&queue, "alice")[0].state, NoticeState::Waiting);
    assert!(open_for(&queue, "former").is_empty());
}

#[test]
fn a_file_already_asked_about_closes_the_stranded_copy() {
    let queue = NoticeQueue::new();
    question(&queue, "service", "a.flac");
    question(&queue, "alice", "a.flac");
    let settings = hand_over_settings(&["alice", "service"]);

    NoticeReconcile::hand_over(&queue, &settings, &review_of(&settings, &["alice"]));

    assert_eq!(open_for(&queue, "alice").len(), 1);
    let closed = queue.for_user("service", NoticeKind::Review);
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].state, NoticeState::Dismissed);
}

#[test]
fn nothing_moves_when_the_person_asked_has_no_playlist_and_the_pause_says_so() {
    let queue = NoticeQueue::new();
    question(&queue, "service", "a.flac");
    let settings = hand_over_settings(&["alice", "service"]);

    NoticeReconcile::hand_over(&queue, &settings, &review_of(&settings, &["bob"]));

    assert_eq!(open_for(&queue, "service").len(), 1);
    assert!(queue.pause_reason(50, "alice").contains("no Review playlist"));

    NoticeReconcile::hand_over(&queue, &settings, &review_of(&settings, &["alice"]));

    assert!(
        queue
            .pause_reason(50, "alice")
            .contains("alice's Review playlist")
    );
}

#[test]
fn a_failed_read_of_the_playlists_moves_nothing() {
    let queue = NoticeQueue::new();
    question(&queue, "service", "a.flac");
    let settings = hand_over_settings(&["alice", "service"]);

    NoticeReconcile::hand_over(&queue, &settings, &[]);

    assert_eq!(open_for(&queue, "service").len(), 1);
    assert!(
        queue
            .pause_reason(50, "alice")
            .contains("Review playlist. Answer")
    );
}
