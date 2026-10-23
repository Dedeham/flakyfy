//! Each listener's chart country and pinned countries, in `chart-countries.json`. A listener with
//! neither has no entry and follows the server's chart country.

use std::collections::BTreeMap;
use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// One listener's chart choices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerCharts {
    /// Their own chart country, lowercase; empty follows the server.
    #[serde(default)]
    pub country: String,
    /// Countries whose charts they get as well, at most `MAX_PINNED`.
    #[serde(default)]
    pub pinned: Vec<String>,
}

/// Every listener's chart choices, in `chart-countries.json`.
pub struct ChartCountryStore {
    path: Option<PathBuf>,
    users: Mutex<Users>,
    /// The generation last written to the file, held while writing, so a save never writes over
    /// a newer one.
    written: Mutex<u64>,
}

/// The choices, and how many changes they have had.
#[derive(Default)]
struct Users {
    by_name: BTreeMap<String, ListenerCharts>,
    generation: u64,
}

impl ChartCountryStore {
    /// `path`: the file, or none to keep the choices in memory only. A file that cannot be read
    /// is moved aside to `chart-countries.json.broken-<unix time>` and the store starts empty.
    /// Names and countries are read in lowercase, whatever case the file holds.
    pub fn new(path: Option<PathBuf>) -> Self {
        let read: BTreeMap<String, ListenerCharts> = match path.as_ref().map(std::fs::read) {
            Some(Ok(bytes)) => match serde_json::from_slice(&bytes) {
                Ok(read) => read,
                Err(e) => {
                    let path = path.as_ref().expect("read from a path");
                    let aside = broken_name(path);
                    match std::fs::rename(path, &aside) {
                        Ok(()) => warn!(
                            "Could not read the chart countries, starting without them; the file is kept as {}: {e}",
                            aside.display()
                        ),
                        Err(moved) => warn!(
                            "Could not read the chart countries, starting without them: {e} (and could not move the file aside: {moved})"
                        ),
                    }
                    BTreeMap::new()
                }
            },
            _ => BTreeMap::new(),
        };
        let mut by_name: BTreeMap<String, ListenerCharts> = BTreeMap::new();
        for (user, charts) in read {
            let entry = by_name.entry(key(&user)).or_default();
            if entry.country.is_empty() {
                entry.country = charts.country.trim().to_ascii_lowercase();
            }
            for pin in charts.pinned {
                let pin = pin.trim().to_ascii_lowercase();
                if !pin.is_empty() && !entry.pinned.contains(&pin) {
                    entry.pinned.push(pin);
                }
            }
        }
        ChartCountryStore {
            path,
            users: Mutex::new(Users {
                by_name,
                generation: 0,
            }),
            written: Mutex::new(0),
        }
    }

    /// A listener's choices, none when they never chose.
    pub fn get(&self, user: &str) -> ListenerCharts {
        self.users
            .lock()
            .by_name
            .get(&key(user))
            .cloned()
            .unwrap_or_default()
    }

    /// Saves a listener's choices. Choosing nothing removes their entry.
    pub fn set(&self, user: &str, charts: ListenerCharts) {
        let _ = self.update(user, |mine| {
            *mine = charts;
            Ok::<(), ()>(())
        });
    }

    /// Changes a listener's choices in one step, so two changes at once never undo each other.
    /// An error from `change` leaves them as they were. Answers the choices as saved.
    pub fn update<E>(
        &self,
        user: &str,
        change: impl FnOnce(&mut ListenerCharts) -> Result<(), E>,
    ) -> Result<ListenerCharts, E> {
        let (mine, json, generation) = {
            let mut users = self.users.lock();
            let mut mine = users.by_name.get(&key(user)).cloned().unwrap_or_default();
            change(&mut mine)?;
            if mine == ListenerCharts::default() {
                users.by_name.remove(&key(user));
            } else {
                users.by_name.insert(key(user), mine.clone());
            }
            users.generation += 1;
            let json = self
                .path
                .is_some()
                .then(|| serde_json::to_vec_pretty(&users.by_name));
            (mine, json, users.generation)
        };
        if let Some(json) = json {
            self.save(json, generation);
        }
        Ok(mine)
    }

    /// Writes one generation of the choices, unless a newer one was written already. The file is
    /// written outside the choices' lock, so reading them never waits on the disk.
    fn save(&self, json: serde_json::Result<Vec<u8>>, generation: u64) {
        let Some(path) = &self.path else {
            return;
        };
        let mut written = self.written.lock();
        if generation <= *written {
            return;
        }
        match json {
            Ok(json) => {
                let temp = path.with_extension("json.tmp");
                match std::fs::write(&temp, json).and_then(|()| std::fs::rename(&temp, path)) {
                    Ok(()) => *written = generation,
                    Err(e) => warn!("Could not save the chart countries: {e}"),
                }
            }
            Err(e) => warn!("Could not save the chart countries: {e}"),
        }
    }

    /// Every listener with choices, by key.
    pub fn all(&self) -> Vec<(String, ListenerCharts)> {
        self.users
            .lock()
            .by_name
            .iter()
            .map(|(user, charts)| (user.clone(), charts.clone()))
            .collect()
    }

    /// Every country some listener chose or pinned, sorted, no repeats.
    pub fn countries(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .users
            .lock()
            .by_name
            .values()
            .flat_map(|c| std::iter::once(c.country.clone()).chain(c.pinned.iter().cloned()))
            .filter(|c| !c.is_empty())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

fn key(user: &str) -> String {
    user.trim().to_lowercase()
}

/// Where a file that could not be read is kept: beside it, named for when it was moved.
fn broken_name(path: &std::path::Path) -> PathBuf {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".broken-{seconds}"));
    PathBuf::from(name)
}

#[cfg(test)]
#[path = "chart_country_store_tests.rs"]
mod tests;
