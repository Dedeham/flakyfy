//! Loading and saving the family's JSON files. A file that cannot be read is never thrown
//! away: it is renamed to `<name>.broken-<unix time>` and the store starts empty, with a
//! warning the Family page can show.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::warn;

use crate::services::state_file;

/// The value in `path` (the default when the file is missing or there is no path), and a
/// warning when the file was there but unreadable and has been kept aside.
pub(crate) fn load<T: DeserializeOwned + Default>(path: Option<&Path>) -> (T, Option<String>) {
    let Some(path) = path else {
        return (T::default(), None);
    };
    let problem = match state_file::read_text(path) {
        Ok(None) => return (T::default(), None),
        Ok(Some(text)) if text.trim().is_empty() => return (T::default(), None),
        Ok(Some(text)) => match serde_json::from_str::<T>(&text) {
            Ok(value) => return (value, None),
            Err(e) => e.to_string(),
        },
        Err(e) => e.to_string(),
    };
    let kept = broken_path(path, chrono::Utc::now().timestamp());
    let message = match std::fs::rename(path, &kept) {
        Ok(()) => format!(
            "{} could not be read ({problem}); it was kept as {} and Family started without it",
            file_name(path),
            file_name(&kept)
        ),
        Err(e) => format!(
            "{} could not be read ({problem}) or kept aside ({e}); Family started without it",
            file_name(path)
        ),
    };
    warn!("FAMILY {message}");
    (T::default(), Some(message))
}

/// Writes `value` to `path` whole (through a temporary file), logging a failure.
pub(crate) fn save<T: Serialize>(path: Option<&Path>, value: &T) {
    let Some(path) = path else { return };
    if let Err(e) = state_file::save_atomic(path, &octo_core::json::to_string(value)) {
        warn!("FAMILY {} could not be written: {e}", file_name(path));
    }
}

/// `<path>.broken-<unix time>`.
pub(crate) fn broken_path(path: &Path, unix_time: i64) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".broken-{unix_time}"));
    PathBuf::from(name)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A path that is set and not blank.
pub(crate) fn clean_path(path: Option<impl AsRef<Path>>) -> Option<PathBuf> {
    path.map(|p| p.as_ref().to_path_buf())
        .filter(|p| !p.as_os_str().to_string_lossy().trim().is_empty())
}
