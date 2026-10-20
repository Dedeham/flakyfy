//! `ListeningStore`: completed plays, summed per day and listener, in `listening.json`. Plays
//! are counted only while `Monitoring:Enabled` is on; days past `Monitoring:RetentionDays` are
//! dropped as new ones come. Changes reach the file on the "ListeningStore" worker's timer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use octo_core::monitoring::{ListeningLog, Play, Summary};
use octo_core::settings::SettingsStore;
use parking_lot::Mutex;
use tracing::warn;

use crate::services::state_file;

pub struct ListeningStore {
    path: Option<PathBuf>,
    settings: Arc<SettingsStore>,
    log: Mutex<ListeningLog>,
    dirty: AtomicBool,
    /// Held from the snapshot to the file move, so an older snapshot never lands last.
    save_lock: Mutex<()>,
}

impl ListeningStore {
    /// The store over `path` (none keeps it in memory). A file that cannot be read starts the
    /// log empty, with a warning.
    pub fn new(path: Option<PathBuf>, settings: Arc<SettingsStore>) -> Self {
        let log = path.as_deref().map(load).unwrap_or_default();
        ListeningStore {
            path,
            settings,
            log: Mutex::new(log),
            dirty: AtomicBool::new(false),
            save_lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether plays are being recorded.
    pub fn enabled(&self) -> bool {
        self.settings.current().monitoring.enabled
    }

    /// Counts `play` on the UTC day of `at`, while Monitoring is on.
    pub fn record(&self, play: &Play, at: DateTime<Utc>) {
        let monitoring = self.settings.current().monitoring.clone();
        if !monitoring.enabled {
            return;
        }
        let today = at.date_naive();
        let mut log = self.log.lock();
        log.record(today, play);
        log.prune(today, monitoring.effective_retention_days());
        self.dirty.store(true, Ordering::Release);
    }

    /// The last `days` days up to `today` (0: every day kept).
    pub fn summary(&self, today: DateTime<Utc>, days: u32) -> Summary {
        let keep = self.settings.current().monitoring.effective_retention_days();
        let mut log = self.log.lock();
        // A shorter retention set since the last play takes effect here too.
        if log.prune(today.date_naive(), keep) {
            self.dirty.store(true, Ordering::Release);
        }
        log.summarize(today.date_naive(), days)
    }

    /// Forgets every play, on disk too.
    pub fn clear(&self) {
        *self.log.lock() = ListeningLog::default();
        self.dirty.store(true, Ordering::Release);
        self.flush_if_dirty();
    }

    /// Writes the log when it changed since the last write; a failed write is tried again next time.
    pub fn flush_if_dirty(&self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        let _saving = self.save_lock.lock();
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return;
        }
        let text = octo_core::json::to_string(&*self.log.lock());
        if let Err(e) = state_file::save_atomic(path, &text) {
            warn!("Could not save {}: {e}", path.display());
            self.dirty.store(true, Ordering::Release);
        }
    }
}

fn load(path: &Path) -> ListeningLog {
    let read = || -> anyhow::Result<ListeningLog> {
        match state_file::read_text(path)? {
            Some(text) if !text.trim().is_empty() => Ok(serde_json::from_str(&text)?),
            _ => Ok(ListeningLog::default()),
        }
    };
    read().unwrap_or_else(|e| {
        warn!(
            "{} could not be read; the listening log starts empty: {e}",
            path.display()
        );
        ListeningLog::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use octo_core::settings::{AppSettings, MonitoringSettings};

    fn settings(enabled: bool) -> Arc<SettingsStore> {
        Arc::new(SettingsStore::for_tests(AppSettings {
            monitoring: MonitoringSettings {
                enabled,
                retention_days: 30,
            },
            ..Default::default()
        }))
    }

    fn play() -> Play {
        Play::new(
            "ann",
            Some(180),
            Some("Rock"),
            "Symfonium",
            "Symfonium/12 (Android 15)",
        )
    }

    #[test]
    fn nothing_is_recorded_while_monitoring_is_off() {
        let store = ListeningStore::new(None, settings(false));
        store.record(&play(), Utc::now());
        assert_eq!(store.summary(Utc::now(), 0).totals.plays, 0);
    }

    #[test]
    fn plays_reach_the_file_and_load_again() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("listening.json");
        let store = ListeningStore::new(Some(path.clone()), settings(true));
        store.record(&play(), Utc::now());
        store.flush_if_dirty();
        let again = ListeningStore::new(Some(path), settings(true));
        let summary = again.summary(Utc::now(), 7);
        assert_eq!(summary.totals.seconds, 180);
        assert_eq!(summary.users[0].apps[0].name, "Symfonium");
        assert_eq!(summary.users[0].platforms[0].name, "Android");
    }

    #[test]
    fn clear_forgets_on_disk_too() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("listening.json");
        let store = ListeningStore::new(Some(path.clone()), settings(true));
        store.record(&play(), Utc::now());
        store.clear();
        let again = ListeningStore::new(Some(path), settings(true));
        assert_eq!(again.summary(Utc::now(), 0).totals.plays, 0);
    }

    #[test]
    fn an_unreadable_file_starts_empty() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("listening.json");
        std::fs::write(&path, "{not json").expect("written");
        let store = ListeningStore::new(Some(path), settings(true));
        assert_eq!(store.summary(Utc::now(), 0).totals.plays, 0);
    }
}
