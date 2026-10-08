//! `Services/Imports/TrickleQueue.cs`'s file, `imports-trickle.json`: the missing songs the
//! trickle is working through, one job per song per person however many lists hold it, with who
//! has paused theirs.

use std::path::Path;

use chrono::{DateTime, TimeDelta, Utc};
use octo_core::json::datetime;
use serde::{Deserialize, Serialize};

use super::TrickleJob;
use super::import_models::import_track_states;
use crate::services::state_file::null_as_default;

/// A song that could not be found is tried again after two weeks, since what Soulseek has
/// changes.
pub const RETRY_NOT_FOUND_AFTER: TimeDelta = TimeDelta::days(14);

/// A fetched job is kept a week, then pruned.
pub const KEEP_DONE: TimeDelta = TimeDelta::days(7);

/// `TrickleQueue.FileShape`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TrickleFile {
    #[serde(deserialize_with = "null_as_default")]
    pub jobs: Vec<TrickleJob>,
    /// The people who paused their trickle.
    #[serde(deserialize_with = "null_as_default")]
    pub paused: Vec<String>,
    #[serde(with = "datetime::utc_option")]
    pub last_start_utc: Option<DateTime<Utc>>,
}

/// The file as the queue starts from it: a job running when Octo stopped is queued again (the
/// library check it starts with catches one that arrived meanwhile).
pub fn load(path: &Path) -> TrickleFile {
    let mut file: TrickleFile = super::load_or_default(path, "the trickle queue");
    for job in &mut file.jobs {
        if job.state == import_track_states::DOWNLOADING {
            job.state = import_track_states::QUEUED.to_string();
            job.detail = Some("Octo restarted while this was running; it goes again.".to_string());
        }
    }
    file
}

pub fn save(path: &Path, file: &TrickleFile) -> bool {
    super::save(path, file, "the trickle queue")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/rust-migration/fixtures/state/imports-trickle.json"
    );

    #[test]
    fn the_fixture_round_trips_byte_for_byte() {
        let text = std::fs::read_to_string(FIXTURE).expect("the fixture is in the repo");
        let file: TrickleFile = serde_json::from_str(&text).expect("the fixture reads");
        assert_eq!(file.paused, ["anh"]);
        assert_eq!(octo_core::json::to_string(&file), text.trim_end_matches('\n'));
    }

    /// A job running when Octo stopped is queued again on load.
    #[test]
    fn a_running_job_is_queued_again_on_load() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("imports-trickle.json");
        let mut file: TrickleFile =
            serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("the fixture")).expect("reads");
        file.jobs[0].state = import_track_states::DOWNLOADING.to_string();
        assert!(save(&path, &file));
        let loaded = load(&path);
        assert_eq!(loaded.jobs[0].state, import_track_states::QUEUED);
        assert_eq!(
            loaded.jobs[0].detail.as_deref(),
            Some("Octo restarted while this was running; it goes again.")
        );
        assert_eq!(loaded.jobs[1], file.jobs[1]);
    }
}
