//! The audit on the cases found in the library: a studio file on a promo of instrumentals with
//! that promo's release track, a vocal file under "(Instrumentals)", a live take titled plainly
//! beside its studio copy, and stray spaces.

use super::*;

fn identity(recording: &str, kind: RecordingKind, by_length: bool) -> Identity {
    Identity {
        recording_id: recording.into(),
        kind,
        by_length,
        kind_certain: true,
    }
}

fn heard(recording: &str, kind: RecordingKind, by_length: bool, sure: bool) -> HeardAs {
    HeardAs {
        identity: Identity {
            kind_certain: by_length || sure,
            ..identity(recording, kind, by_length)
        },
        sure,
        written_kind: None,
        track_owner: None,
        on_release: None,
    }
}

fn change<'a>(audit: &'a TagAudit, field: &str) -> Option<Option<&'a str>> {
    audit.changes.get(field).map(|value| value.as_deref())
}

fn rules(audit: &TagAudit) -> Vec<AuditRule> {
    audit.findings.iter().map(|finding| finding.rule).collect()
}

fn parade() -> FileTags {
    FileTags {
        title: "Welcome to the Black Parade".into(),
        album: "The Black Parade (instrumentals)".into(),
        recording_id: Some("rec-studio".into()),
        release_track_id: Some("track-on-promo".into()),
        release_id: Some("r-promo".into()),
        release_group_id: Some("g-parade".into()),
    }
}

#[test]
fn a_studio_file_on_a_promo_of_instrumentals_gets_the_plain_album_and_loses_the_promos_ids() {
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.track_owner = Some(("rec-instrumental".into(), RecordingKind::Instrumental));
    studio.on_release = Some(None);
    let found = audit(&parade(), Some(&studio));

    assert_eq!(rules(&found), [AuditRule::Album, AuditRule::Ids]);
    assert!(found.sure);
    assert_eq!(change(&found, "album"), Some(Some("The Black Parade")));
    assert_eq!(change(&found, "title"), None);
    // The recording id already names the audio.
    assert_eq!(change(&found, "recordingId"), None);
    assert_eq!(change(&found, "releaseTrackId"), Some(None));
    assert_eq!(change(&found, "releaseId"), Some(None));
    assert_eq!(change(&found, "releaseGroupId"), Some(None));
    let ids = &found.findings[1].words;
    assert!(
        ids.contains("track-on-promo") && ids.contains("an instrumental"),
        "{ids}"
    );
}

#[test]
fn a_release_that_lists_the_audios_recording_keeps_its_ids_and_names_the_right_track() {
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.track_owner = Some(("rec-instrumental".into(), RecordingKind::Instrumental));
    studio.on_release = Some(Some("track-studio".into()));
    let found = audit(&parade(), Some(&studio));
    assert_eq!(change(&found, "releaseTrackId"), Some(Some("track-studio")));
    assert_eq!(change(&found, "releaseId"), None);
    assert_eq!(change(&found, "releaseGroupId"), None);
}

#[test]
fn when_the_release_is_not_known_only_the_wrong_track_comes_off() {
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.track_owner = Some(("rec-instrumental".into(), RecordingKind::Instrumental));
    let found = audit(&parade(), Some(&studio));
    assert_eq!(change(&found, "releaseTrackId"), Some(None));
    assert_eq!(change(&found, "releaseId"), None);
}

#[test]
fn a_vocal_song_under_instrumentals_gets_its_plain_album_and_its_recording_id() {
    let tags = FileTags {
        title: "So What (feat. Popcaan)".into(),
        album: "A Muse In Her Feelings (Instrumentals)".into(),
        ..Default::default()
    };
    let found = audit(
        &tags,
        Some(&heard("rec-so-what", RecordingKind::Studio, false, true)),
    );
    assert_eq!(rules(&found), [AuditRule::Album]);
    assert_eq!(change(&found, "album"), Some(Some("A Muse In Her Feelings")));
    assert_eq!(change(&found, "recordingId"), Some(Some("rec-so-what")));
    assert_eq!(
        found.findings[0].words,
        "The album 'A Muse In Her Feelings (Instrumentals)' names an instrumental, but the audio is the studio recording."
    );
}

#[test]
fn a_live_take_titled_plainly_says_so_in_its_title() {
    let tags = FileTags {
        title: "Uneasy Hearts Weigh The Most".into(),
        album: "Tree City Sessions 2".into(),
        ..Default::default()
    };
    let found = audit(&tags, Some(&heard("rec-live", RecordingKind::Live, true, true)));
    assert_eq!(rules(&found), [AuditRule::Title]);
    assert_eq!(
        found.findings[0].words,
        "The audio is a live recording, and the title does not say so."
    );
    assert_eq!(
        change(&found, "title"),
        Some(Some("Uneasy Hearts Weigh The Most (Live)"))
    );
    assert_eq!(change(&found, "recordingId"), Some(Some("rec-live")));
}

#[test]
fn an_identity_that_is_not_sure_writes_no_ids_but_still_says_what_it_found() {
    let mut guess = heard("rec-studio", RecordingKind::Studio, false, false);
    guess.track_owner = Some(("rec-live".into(), RecordingKind::Live));
    let found = audit(&parade(), Some(&guess));
    assert!(!found.sure);
    assert_eq!(rules(&found), [AuditRule::Album, AuditRule::Ids]);
    assert_eq!(change(&found, "album"), Some(Some("The Black Parade")));
    for field in ["recordingId", "releaseTrackId", "releaseId", "releaseGroupId"] {
        assert_eq!(change(&found, field), None, "{field}");
    }
}

#[test]
fn another_takes_recording_id_is_replaced_when_the_length_chose_the_audio() {
    let tags = FileTags {
        title: "Welcome to the Black Parade".into(),
        album: "The Black Parade".into(),
        recording_id: Some("rec-instrumental".into()),
        ..Default::default()
    };
    let mut studio = heard("rec-studio", RecordingKind::Studio, true, true);
    studio.written_kind = Some(RecordingKind::Instrumental);
    let found = audit(&tags, Some(&studio));
    assert_eq!(rules(&found), [AuditRule::Ids]);
    assert!(found.findings[0].words.contains("is an instrumental"));
    assert_eq!(change(&found, "recordingId"), Some(Some("rec-studio")));
}

#[test]
fn another_recording_of_the_same_take_is_left_as_it_is() {
    // An album and a compilation often list the same audio under two recordings.
    let tags = FileTags {
        title: "Teardrop".into(),
        album: "Mezzanine".into(),
        recording_id: Some("rec-compilation".into()),
        ..Default::default()
    };
    let mut studio = heard("rec-album", RecordingKind::Studio, false, true);
    studio.written_kind = Some(RecordingKind::Studio);
    let found = audit(&tags, Some(&studio));
    assert!(!found.found());
    assert!(found.changes.is_empty());
}

#[test]
fn stray_spaces_are_found_with_or_without_the_audio() {
    let tags = FileTags {
        title: "Teardrop ".into(),
        album: " Mezzanine".into(),
        ..Default::default()
    };
    let alone = audit(&tags, None);
    assert!(alone.only_spaces());
    assert_eq!(alone.findings.len(), 2);
    assert_eq!(
        alone.findings[0].words,
        "The title 'Teardrop' has stray spaces at its ends."
    );
    assert_eq!(
        alone.findings[1].words,
        "The album 'Mezzanine' has stray spaces at its ends."
    );
    assert_eq!(change(&alone, "title"), Some(Some("Teardrop")));
    assert_eq!(change(&alone, "album"), Some(Some("Mezzanine")));
    assert_eq!(alone.space_changes(&tags), alone.changes);

    let with_audio = audit(&tags, Some(&heard("rec", RecordingKind::Studio, true, true)));
    // The sweep writes only the spaces by itself; the recording id waits for a fix.
    assert!(with_audio.only_spaces());
    assert_eq!(change(&with_audio, "recordingId"), Some(Some("rec")));
    assert_eq!(with_audio.space_changes(&tags).len(), 2);
}

#[test]
fn without_the_audio_an_album_naming_a_take_is_not_a_finding() {
    let tags = FileTags {
        title: "So What".into(),
        album: "A Muse In Her Feelings (Instrumentals)".into(),
        ..Default::default()
    };
    let found = audit(&tags, None);
    assert!(!found.found());
    assert!(found.changes.is_empty());
}

#[test]
fn an_album_with_nothing_plain_left_is_found_but_not_changed() {
    let tags = FileTags {
        title: "So What".into(),
        album: "Instrumentals".into(),
        ..Default::default()
    };
    let found = audit(&tags, Some(&heard("rec", RecordingKind::Studio, true, true)));
    assert_eq!(rules(&found), [AuditRule::Album]);
    assert_eq!(change(&found, "album"), None);
    assert!(found.findings[0].words.ends_with("so it waits for you."));
}

#[test]
fn a_live_copy_beside_the_studio_copy_is_named_and_the_studio_copy_is_not() {
    let copy = |id: &str, title: &str, album: &str, kind| TakeCopy {
        id: id.into(),
        artist: "Dance Gavin Dance".into(),
        title: title.into(),
        album: album.into(),
        kind,
    };
    let found = takes_beside(&[
        copy(
            "studio",
            "Uneasy Hearts Weigh The Most",
            "Dance Gavin Dance",
            RecordingKind::Studio,
        ),
        copy(
            "live",
            "Uneasy Hearts Weigh The Most",
            "Tree City Sessions 2",
            RecordingKind::Live,
        ),
        copy("other", "Lemon Meringue Tie", "Mothership", RecordingKind::Studio),
    ]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "live");
    assert!(found[0].1.contains("a live recording"), "{}", found[0].1);
    assert!(found[0].1.contains("'Dance Gavin Dance'"), "{}", found[0].1);

    // A live copy whose title says so is a different song, not a hidden one.
    let named = takes_beside(&[
        copy(
            "studio",
            "Uneasy Hearts Weigh The Most",
            "Dance Gavin Dance",
            RecordingKind::Studio,
        ),
        copy(
            "live",
            "Uneasy Hearts Weigh The Most (Live)",
            "Tree City Sessions 2",
            RecordingKind::Live,
        ),
    ]);
    assert!(named.is_empty());
}

#[test]
fn rule_names_read_back() {
    for rule in AuditRule::ALL {
        assert_eq!(AuditRule::parse(rule.name()), Some(rule));
        assert!(!rule.label().is_empty());
    }
    assert_eq!(AuditRule::parse("nope"), None);
}
