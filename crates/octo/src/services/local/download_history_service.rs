//! Port of `Services/Local/DownloadHistoryService.cs`, the store behind `downloads-history.json`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, TimeDelta, Utc};
use octo_core::models::download::acquisition_event::acquisition_event_kinds;
use octo_core::models::download::{AcquisitionEvent, DownloadHistoryEntry};
use parking_lot::Mutex;
use tracing::warn;

use crate::services::state_file;

/// Persistent, bounded log of songs Octo has fetched. Written to a JSON file next
/// to the settings file so it survives restarts and container recreation (the
/// config dir is a host bind-mount). Newest entries first; capped so it can't grow
/// without limit. Best-effort: a write failure never breaks a download.
pub struct DownloadHistoryService {
    path: PathBuf,
    /// Loaded on first use.
    cache: Mutex<Option<Vec<DownloadHistoryEntry>>>,
    /// [`Self::MAX_LOG_BYTES`], smaller in tests.
    log_room: AtomicI64,
}

impl DownloadHistoryService {
    pub const MAX_ENTRIES: usize = 500;

    /// Lines kept per saved log. The first line stays, and the newest ones after it.
    pub const MAX_LOG_LINES: usize = 80;

    /// Copies kept on one saved line: the best ones, which are what a search line is read for.
    pub const MAX_SAVED_CANDIDATES: usize = 12;

    /// Longest text, detail or file name kept on a saved line.
    pub const MAX_SAVED_TEXT: usize = 300;

    /// Room for every saved log together. Past it, the oldest entries let go of their logs (the
    /// entries themselves stay), so the file stays small enough to read whole on every change.
    pub const MAX_LOG_BYTES: i64 = 2 * 1024 * 1024;

    pub fn new(path: impl AsRef<Path>) -> Self {
        DownloadHistoryService {
            path: path.as_ref().to_path_buf(),
            cache: Mutex::new(None),
            log_room: AtomicI64::new(Self::MAX_LOG_BYTES),
        }
    }

    /// The room for every saved log together (the C# `LogRoom`, set by tests).
    pub fn set_log_room(&self, bytes: i64) {
        self.log_room.store(bytes, Ordering::Relaxed);
    }

    // ---- A download's log, kept with its entry (the dashboard's Fetched songs) ----------------

    /// Saves a download's log with the entry it made: the newest entry with this key written
    /// since the run began, so a log never lands on an older fetch of the same song. Called
    /// again when a line arrives after the end (lyrics, an upgrade's verdict), and each time the
    /// whole log is saved anew. False when there is no such entry.
    pub fn attach_log(&self, key: &str, started_utc: DateTime<Utc>, events: &[AcquisitionEvent]) -> bool {
        if octo_core::common::dotnet::is_blank(key) || events.is_empty() {
            return false;
        }
        let mut cache = self.cache.lock();
        let list = self.load_locked(&mut cache);
        let since = started_utc - TimeDelta::seconds(1);
        let Some(entry) = list
            .iter_mut()
            .find(|e| e.key.as_deref() == Some(key) && Self::written_at(e).is_some_and(|at| at >= since))
        else {
            return false;
        };
        entry.log = Some(Self::trimmed(events));
        self.keep_logs_within_room(list);
        self.save_locked(list);
        true
    }

    /// The entry written at this moment for this key, as listed (DownloadedAt), or None.
    pub fn find(&self, key: &str, downloaded_at: Option<&str>) -> Option<DownloadHistoryEntry> {
        if octo_core::common::dotnet::is_blank(key) {
            return None;
        }
        let mut cache = self.cache.lock();
        let list = self.load_locked(&mut cache);
        list.iter()
            .find(|e| e.key.as_deref() == Some(key) && downloaded_at.is_none_or(|at| e.downloaded_at == at))
            .cloned()
    }

    /// When an entry was written, from its DownloadedAt; None when that does not read.
    pub fn written_at(entry: &DownloadHistoryEntry) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(entry.downloaded_at.trim())
            .ok()
            .map(|at| at.with_timezone(&Utc))
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(entry.downloaded_at.trim(), "%Y-%m-%dT%H:%M:%S%.f")
                    .ok()
                    .map(|at| at.and_utc())
            })
    }

    /// A log as it is saved: at most [`Self::MAX_LOG_LINES`] lines (the first, a line saying how
    /// many were left out, and the newest), each line's words and copies capped.
    pub fn trimmed(events: &[AcquisitionEvent]) -> Vec<AcquisitionEvent> {
        fn cap(text: Option<&str>) -> Option<String> {
            let text = text?;
            if octo_core::common::dotnet::utf16_len(text) <= DownloadHistoryService::MAX_SAVED_TEXT {
                return Some(text.to_string());
            }
            Some(format!(
                "{}...",
                utf16_prefix(text, DownloadHistoryService::MAX_SAVED_TEXT - 3).trim_end()
            ))
        }
        fn line(event: &AcquisitionEvent) -> AcquisitionEvent {
            AcquisitionEvent {
                text: cap(Some(&event.text)).unwrap_or_default(),
                detail: cap(event.detail.as_deref()),
                candidates: event
                    .candidates
                    .as_ref()
                    .filter(|copies| !copies.is_empty())
                    .map(|copies| {
                        copies
                            .iter()
                            .take(DownloadHistoryService::MAX_SAVED_CANDIDATES)
                            .map(|copy| {
                                let mut copy = copy.clone();
                                copy.file = cap(copy.file.as_deref());
                                copy.folder = cap(copy.folder.as_deref());
                                copy.note = cap(copy.note.as_deref());
                                copy
                            })
                            .collect()
                    }),
                ..event.clone()
            }
        }

        if events.len() <= Self::MAX_LOG_LINES {
            return events.iter().map(line).collect();
        }
        let left = events.len() - (Self::MAX_LOG_LINES - 1);
        let tail = &events[events.len() - (Self::MAX_LOG_LINES - 2)..];
        let mut kept = vec![line(&events[0])];
        kept.push(AcquisitionEvent {
            at: tail[0].at,
            kind: acquisition_event_kinds::NOTE.to_string(),
            text: format!(
                "{left} {} left out to keep the saved log short",
                if left == 1 { "line" } else { "lines" }
            ),
            detail: None,
            candidates: None,
        });
        kept.extend(tail.iter().map(line));
        kept
    }

    /// The newest entries keep their logs; once they fill the room, older ones let go. Caller
    /// holds the lock.
    fn keep_logs_within_room(&self, list: &mut [DownloadHistoryEntry]) {
        let room = self.log_room.load(Ordering::Relaxed);
        let mut used: i64 = 0;
        let mut full = false;
        for entry in list.iter_mut() {
            let Some(log) = &entry.log else {
                continue;
            };
            if !full {
                let size = octo_core::json::to_string_without_nulls(log).len() as i64;
                if used + size <= room {
                    used += size;
                    continue;
                }
                full = true;
            }
            entry.log = None;
        }
    }

    /// Append a fetched-song entry (newest first) and persist.
    pub fn record(&self, entry: DownloadHistoryEntry) {
        let mut cache = self.cache.lock();
        let list = self.load_locked(&mut cache);
        list.insert(0, entry);
        list.truncate(Self::MAX_ENTRIES);
        self.save_locked(list);
    }

    /// The most recent entries, newest first. The C# default was 200.
    pub fn get_recent(&self, limit: i32) -> Vec<DownloadHistoryEntry> {
        let mut cache = self.cache.lock();
        let list = self.load_locked(&mut cache);
        list.iter().take(limit.max(0) as usize).cloned().collect()
    }

    fn load_locked<'a>(
        &self,
        cache: &'a mut Option<Vec<DownloadHistoryEntry>>,
    ) -> &'a mut Vec<DownloadHistoryEntry> {
        cache.get_or_insert_with(|| {
            let loaded = (|| -> anyhow::Result<Vec<DownloadHistoryEntry>> {
                let Some(json) = state_file::read_text(&self.path)? else {
                    return Ok(Vec::new());
                };
                if json.trim().is_empty() {
                    return Ok(Vec::new());
                }
                Ok(serde_json::from_str::<Option<Vec<DownloadHistoryEntry>>>(&json)?.unwrap_or_default())
            })();
            loaded.unwrap_or_else(|e| {
                warn!("Download history load failed ({e}); starting fresh");
                Vec::new()
            })
        })
    }

    fn save_locked(&self, list: &[DownloadHistoryEntry]) {
        // Written with WhenWritingNull, as the C# FileOptions are: a saved log's copies are
        // mostly missing figures, and the file has to stay small enough to read whole.
        if let Err(e) = state_file::save_atomic(&self.path, &octo_core::json::to_string_without_nulls(list)) {
            warn!("Download history save failed: {e}");
        }
    }
}

/// The longest prefix of at most `units` UTF-16 code units (`text[..units]`), never splitting a
/// character.
fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut count = 0;
    for (index, c) in text.char_indices() {
        count += c.len_utf16();
        if count > units {
            return &text[..index];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/rust-migration/fixtures/state/downloads-history.json"
    );

    fn entry(title: &str) -> DownloadHistoryEntry {
        DownloadHistoryEntry {
            artist: "A".into(),
            title: title.into(),
            downloaded_at: "2026-10-02T09:01:44.0807210Z".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_record_goes_first_and_the_rest_of_the_fixture_is_written_back_unchanged() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("downloads-history.json");
        let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
        std::fs::write(&path, &text).expect("writes");

        let service = DownloadHistoryService::new(&path);
        let before = service.get_recent(200).len();
        service.record(entry("New"));

        let written = std::fs::read_to_string(&path).expect("reads");
        let expected_tail = &text.trim_end_matches('\n')[1..];
        assert!(
            written.ends_with(expected_tail),
            "the old entries keep their bytes"
        );
        assert!(written.starts_with(r#"[{"Artist":"A","Title":"New","#));
        assert_eq!(service.get_recent(200).len(), before + 1);
        assert_eq!(service.get_recent(1)[0].title, "New");
        assert!(service.get_recent(-5).is_empty());
    }

    #[test]
    fn the_log_is_capped_at_500_newest_first() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config").join("downloads-history.json");
        let service = DownloadHistoryService::new(&path);
        for i in 0..505 {
            service.record(entry(&i.to_string()));
        }
        let reopened = DownloadHistoryService::new(&path);
        let all = reopened.get_recent(1000);
        assert_eq!(all.len(), 500);
        assert_eq!(all[0].title, "504");
        assert_eq!(all[499].title, "5");
    }

    fn line(kind: &str, text: &str) -> AcquisitionEvent {
        AcquisitionEvent {
            at: chrono::Utc::now(),
            kind: kind.into(),
            text: text.into(),
            detail: None,
            candidates: None,
        }
    }

    /// Rust-only (7602787 has no C# test): a saved log keeps its first line, says how many it
    /// left out, keeps the newest, and caps each text.
    #[test]
    fn a_saved_log_is_trimmed() {
        let mut events: Vec<AcquisitionEvent> =
            (0..100).map(|i| line("note", &format!("line {i}"))).collect();
        events[99].text = "x".repeat(400);
        let kept = DownloadHistoryService::trimmed(&events);
        assert_eq!(kept.len(), DownloadHistoryService::MAX_LOG_LINES);
        assert_eq!(kept[0].text, "line 0");
        assert_eq!(kept[1].text, "21 lines left out to keep the saved log short");
        assert_eq!(kept[2].text, "line 22");
        let last = &kept[kept.len() - 1].text;
        assert_eq!(last.chars().count(), DownloadHistoryService::MAX_SAVED_TEXT);
        assert!(last.ends_with("..."));
    }

    /// Rust-only: a log lands on the entry with its key written since the run began, and past
    /// the room the oldest entries let go of theirs.
    #[test]
    fn a_log_is_kept_with_its_entry_within_the_room() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let service = DownloadHistoryService::new(dir.path().join("downloads-history.json"));
        let now = chrono::Utc::now();
        let written = |minutes: i64| {
            (now - chrono::TimeDelta::minutes(minutes))
                .format("%Y-%m-%dT%H:%M:%S%.6fZ")
                .to_string()
        };
        for (title, key, minutes) in [("old", "soulseek:a", 30), ("new", "soulseek:b", 1)] {
            service.record(DownloadHistoryEntry {
                key: Some(key.into()),
                downloaded_at: written(minutes),
                ..entry(title)
            });
        }

        assert!(!service.attach_log(
            "soulseek:a",
            now - chrono::TimeDelta::minutes(5),
            &[line("done", "In your library")]
        ));
        assert!(service.attach_log(
            "soulseek:a",
            now - chrono::TimeDelta::hours(1),
            &[line("done", "In your library")]
        ));
        assert!(service.attach_log(
            "soulseek:b",
            now - chrono::TimeDelta::minutes(2),
            &[line("done", "In your library")]
        ));
        assert!(service.find("soulseek:a", None).and_then(|e| e.log).is_some());

        service.set_log_room(100);
        assert!(service.attach_log(
            "soulseek:b",
            now - chrono::TimeDelta::minutes(2),
            &[line("done", "In your library")]
        ));
        assert!(service.find("soulseek:b", None).and_then(|e| e.log).is_some());
        assert!(service.find("soulseek:a", None).and_then(|e| e.log).is_none());
        let reopened = DownloadHistoryService::new(dir.path().join("downloads-history.json"));
        assert!(reopened.find("soulseek:a", None).is_some_and(|e| e.log.is_none()));
    }

    #[test]
    fn a_missing_blank_or_corrupt_file_starts_empty_and_is_overwritten() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("downloads-history.json");
        assert!(DownloadHistoryService::new(&path).get_recent(10).is_empty());
        std::fs::write(&path, "  \n").expect("writes");
        assert!(DownloadHistoryService::new(&path).get_recent(10).is_empty());
        std::fs::write(&path, "[{\"Artist\":").expect("writes");
        let service = DownloadHistoryService::new(&path);
        assert!(service.get_recent(10).is_empty());
        service.record(entry("x"));
        assert_eq!(DownloadHistoryService::new(&path).get_recent(10).len(), 1);
    }
}
