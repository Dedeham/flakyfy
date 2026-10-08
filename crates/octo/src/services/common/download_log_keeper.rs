//! Port of `Services/Common/DownloadLogKeeper.cs`.

use std::sync::{Arc, Weak};

use tracing::debug;

use super::acquisition_tracker::{AcquisitionState, AcquisitionTracker};
use crate::services::local::DownloadHistoryService;

/// Saves each finished download's log with its entry in the fetched-songs log, so the dashboard
/// can show how an older download went after the live list has forgotten it (three hours). A
/// line that arrives after the end (lyrics, an upgrade's verdict) saves the log again. Only
/// watches: a save that fails is logged and never reaches the download.
pub struct DownloadLogKeeper {
    /// Weak: the tracker holds the keeper's listeners, so a strong reference back would keep both
    /// alive for good.
    tracker: Weak<AcquisitionTracker>,
    history: Arc<DownloadHistoryService>,
}

impl DownloadLogKeeper {
    /// Starts saving the logs of this tracker's finished downloads into this history.
    pub fn attach(
        tracker: &Arc<AcquisitionTracker>,
        history: Arc<DownloadHistoryService>,
    ) -> Arc<DownloadLogKeeper> {
        let keeper = Arc::new(DownloadLogKeeper {
            tracker: Arc::downgrade(tracker),
            history,
        });
        let on_end = keeper.clone();
        tracker.subscribe_ended(Arc::new(move |end| {
            if end.done {
                on_end.save(&end.key);
            }
        }));
        let after_end = keeper.clone();
        tracker.subscribe_logged_after_end(Arc::new(move |key| after_end.save(key)));
        keeper
    }

    /// Saves the log of the row with this key, when it ended in the library.
    pub fn save(&self, key: &str) {
        let Some(tracker) = self.tracker.upgrade() else {
            return;
        };
        let Some(row) = tracker.detail(key, None) else {
            return;
        };
        let Some(events) = row.events.filter(|events| !events.is_empty()) else {
            return;
        };
        if row.state != AcquisitionState::Done {
            return;
        }
        if !self.history.attach_log(key, row.started_at, &events) {
            debug!("No fetched-songs entry for {key} to keep its log with");
        }
    }
}

#[cfg(test)]
mod tests {
    use octo_core::common::Clock;
    use octo_core::models::download::DownloadHistoryEntry;

    use super::*;
    use crate::services::common::acquisition_tracker::acquisition_event_kinds;

    /// `DateTime.UtcNow.ToString("o")`.
    fn now_text() -> String {
        let now = chrono::Utc::now();
        format!(
            "{}.{:07}Z",
            now.format("%Y-%m-%dT%H:%M:%S"),
            now.timestamp_subsec_nanos() / 100
        )
    }

    #[test]
    fn a_finished_download_keeps_its_log_and_a_late_line_saves_it_again() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let history = Arc::new(DownloadHistoryService::new(
            dir.path().join("downloads-history.json"),
        ));
        let tracker = Arc::new(AcquisitionTracker::new(None, Clock::system()));
        DownloadLogKeeper::attach(&tracker, history.clone());
        tracker.begin("soulseek", "abc", Some("abc"), Some("alice"), None, None, None);
        history.record(DownloadHistoryEntry {
            artist: "Air".into(),
            title: "Sexy Boy".into(),
            downloaded_at: now_text(),
            key: Some("soulseek:abc".into()),
            ..Default::default()
        });

        tracker.complete("soulseek", "abc", Some("nd-1"));
        let kinds = |entry: &DownloadHistoryEntry| -> Vec<String> {
            entry.log.iter().flatten().map(|line| line.kind.clone()).collect()
        };
        assert_eq!(
            kinds(&history.get_recent(1)[0]),
            [acquisition_event_kinds::QUEUED, acquisition_event_kinds::DONE]
        );

        tracker.log(
            "soulseek",
            "abc",
            acquisition_event_kinds::LYRICS,
            "Synced lyrics from LRCLIB",
            None,
        );
        assert_eq!(
            kinds(&history.get_recent(1)[0]),
            [
                acquisition_event_kinds::QUEUED,
                acquisition_event_kinds::DONE,
                acquisition_event_kinds::LYRICS
            ]
        );
    }

    #[test]
    fn a_failed_download_keeps_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let history = Arc::new(DownloadHistoryService::new(
            dir.path().join("downloads-history.json"),
        ));
        let tracker = Arc::new(AcquisitionTracker::new(None, Clock::system()));
        DownloadLogKeeper::attach(&tracker, history.clone());
        tracker.begin("soulseek", "abc", Some("abc"), Some("alice"), None, None, None);
        history.record(DownloadHistoryEntry {
            downloaded_at: now_text(),
            key: Some("soulseek:abc".into()),
            ..Default::default()
        });

        tracker.fail("soulseek", "abc", Some("nope"));

        assert!(history.get_recent(1)[0].log.is_none());
    }
}
