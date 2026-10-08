//! `AdminDownloadsTests.cs` (5d3f096), its Recently removed half: the trash and Put back on the
//! dashboard, behind a Navidrome admin's own sign-in and the library actions gates. The download
//! log and Find songs half is group D's (`admin_downloads_tests.rs`).

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use chrono::{TimeDelta, Utc};
use octo_core::settings::{AppSettings, LibraryAction, LibraryActionSettings};

use crate::app::{AppState, TestServices};
use crate::controllers::admin::test_support_6b1::{Reply, app, send};
use crate::http::pipeline::App;
use crate::services::library::library_action_quarantine::QuarantinedSidecar;
use crate::services::library::{
    LibraryActionEntry, LibraryActionJournal, LibraryActionState, PathSource, QuarantineManifest,
};

/// The dashboard over a temporary music folder, library actions on (or in dry run), and the
/// admin on the allowed list or not.
struct Dashboard {
    state: AppState,
    app: App,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

impl Dashboard {
    fn new(allowed: bool, dry_run: bool) -> Dashboard {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path().join("music");
        std::fs::create_dir_all(&root).expect("the music folder");
        let settings = AppSettings {
            library_actions: LibraryActionSettings {
                enabled: true,
                dry_run,
                allowed_users: vec![if allowed { "admin" } else { "someone-else" }.to_string()],
                quarantine_retention_days: 30,
                ..Default::default()
            },
            ..Default::default()
        };
        let state = AppState::for_tests_with(
            settings,
            TestServices {
                config_dir: Some(dir.path().join("config")),
                raw_settings: vec![("Library:DownloadPath".into(), root.to_string_lossy().into_owned())],
                ..Default::default()
            },
        );
        Dashboard {
            app: app(state.clone()),
            state,
            root,
            _dir: dir,
        }
    }

    async fn call(&self, method: Method, path: &str, body: Option<&str>) -> Reply {
        let token = self.state.browse_sessions.create("admin");
        send(
            &self.app,
            method,
            path,
            &[("X-Octo-Admin", "1"), ("X-Octo-Browse-Token", token.as_str())],
            body,
        )
        .await
    }
}

/// A song alice removed two days ago: its file (and lyrics) in the trash with the manifest, and
/// the removal's journal entry.
fn trashed(root: &Path, id: &str, with_lyrics: bool) -> (LibraryActionEntry, PathBuf, PathBuf) {
    let original = root
        .join("Daft Punk")
        .join("Discovery")
        .join(format!("{id}.flac"));
    let trash_dir = root.join(".octo-trash").join("2026-10-04");
    std::fs::create_dir_all(&trash_dir).expect("the trash");
    let trashed = trash_dir.join(format!("{id}.flac"));
    std::fs::write(&trashed, "audio").expect("written");
    let mut sidecars = Vec::new();
    if with_lyrics {
        let lrc = trash_dir.join(format!("{id}.lrc"));
        std::fs::write(&lrc, "[00:01.00]words").expect("written");
        sidecars.push(QuarantinedSidecar {
            original_path: original.with_extension("lrc").to_string_lossy().into_owned(),
            quarantine_path: lrc.to_string_lossy().into_owned(),
        });
    }
    let manifest = QuarantineManifest {
        original_path: original.to_string_lossy().into_owned(),
        navidrome_id: id.to_string(),
        action: "Delete".to_string(),
        username: "alice".to_string(),
        at_utc: Utc::now(),
        sidecars: Some(sidecars),
        mappings: None,
    };
    let mut manifest_path = trashed.clone().into_os_string();
    manifest_path.push(".octo-action.json");
    std::fs::write(&manifest_path, octo_core::json::to_string(&manifest)).expect("written");
    let entry = LibraryActionEntry {
        key: LibraryActionJournal::make_key(LibraryAction::Delete, id, "1:2"),
        action: LibraryAction::Delete,
        navidrome_id: id.to_string(),
        username: "alice".to_string(),
        title: "Digital Love".to_string(),
        artist: "Daft Punk".to_string(),
        album: "Discovery".to_string(),
        source_path: Some(original.to_string_lossy().into_owned()),
        quarantine_path: Some(trashed.to_string_lossy().into_owned()),
        resolution: Some(PathSource::NativeApi),
        state: LibraryActionState::Applied,
        detail: Some("Removed.".to_string()),
        dry_run: false,
        at_utc: Utc::now() - TimeDelta::days(2),
        history_kept: None,
        revealed_path: None,
        copy: false,
        download_allowed: false,
    };
    (entry, original, trashed)
}

#[tokio::test]
async fn recently_removed_lists_the_trash_and_put_back_restores_the_song_with_its_lyrics() {
    let dash = Dashboard::new(true, false);
    let (entry, original, trashed) = trashed(&dash.root, "nd-1", true);
    dash.state.library_action_journal.record(entry.clone());

    let list = dash.call(Method::GET, "/api/admin/trash", None).await.json();
    assert_eq!(list["keepDays"], 30);
    let songs = list["songs"].as_array().expect("songs");
    assert_eq!(songs.len(), 1);
    let song = &songs[0];
    assert_eq!(song["id"], "nd-1");
    assert_eq!(song["removedBy"], "alice");
    assert_eq!(song["sidecars"], serde_json::json!(["nd-1.lrc"]));
    assert_eq!(song["blocksDownloads"], true);
    assert_eq!(song["key"], entry.key.as_str());
    assert!(song["goneAt"].is_string());

    let restored = dash
        .call(Method::POST, "/api/admin/trash/restore", Some(r#"{"id":"nd-1"}"#))
        .await
        .json();
    assert_eq!(restored["state"], "applied");
    assert!(original.exists());
    assert!(original.with_extension("lrc").exists());
    assert!(!trashed.exists());
    let empty = dash.call(Method::GET, "/api/admin/trash", None).await.json();
    assert_eq!(empty["songs"].as_array().map(Vec::len), Some(0));
    let journaled = dash
        .state
        .library_action_journal
        .recent(10)
        .into_iter()
        .find(|e| e.key == entry.key)
        .expect("the removal");
    assert!(journaled.detail.unwrap_or_default().contains("admin"));
}

#[tokio::test]
async fn an_admin_off_the_allowed_list_sees_why_and_changes_nothing() {
    let dash = Dashboard::new(false, false);
    let (entry, original, trashed) = trashed(&dash.root, "nd-2", false);
    dash.state.library_action_journal.record(entry);

    let list = dash.call(Method::GET, "/api/admin/trash", None).await;
    assert_eq!(list.status, StatusCode::FORBIDDEN);
    assert!(list.text().contains("allowed list"));
    let restore = dash
        .call(Method::POST, "/api/admin/trash/restore", Some(r#"{"id":"nd-2"}"#))
        .await;
    assert_eq!(restore.status, StatusCode::FORBIDDEN);
    assert!(trashed.exists());
    assert!(!original.exists());
}

#[tokio::test]
async fn while_dry_run_is_on_put_back_only_says_what_it_would_do() {
    let dash = Dashboard::new(true, true);
    let (entry, original, trashed) = trashed(&dash.root, "nd-3", false);
    dash.state.library_action_journal.record(entry);

    let restored = dash
        .call(Method::POST, "/api/admin/trash/restore", Some(r#"{"id":"nd-3"}"#))
        .await
        .json();
    assert_eq!(restored["state"], "rehearsed");
    assert!(trashed.exists());
    assert!(!original.exists());
}
