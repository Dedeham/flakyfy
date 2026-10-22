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
    users: Mutex<BTreeMap<String, ListenerCharts>>,
}

impl ChartCountryStore {
    /// `path`: the file, or none to keep the choices in memory only. A file that cannot be read
    /// starts the store empty and stays as it is until the next save.
    pub fn new(path: Option<PathBuf>) -> Self {
        let users = match path.as_ref().map(std::fs::read) {
            Some(Ok(bytes)) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!("Could not read the chart countries, starting without them: {e}");
                BTreeMap::new()
            }),
            _ => BTreeMap::new(),
        };
        ChartCountryStore {
            path,
            users: Mutex::new(users),
        }
    }

    /// A listener's choices, none when they never chose.
    pub fn get(&self, user: &str) -> ListenerCharts {
        self.users.lock().get(&key(user)).cloned().unwrap_or_default()
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
        let mut users = self.users.lock();
        let mut mine = users.get(&key(user)).cloned().unwrap_or_default();
        change(&mut mine)?;
        if mine == ListenerCharts::default() {
            users.remove(&key(user));
        } else {
            users.insert(key(user), mine.clone());
        }
        self.save(&users);
        Ok(mine)
    }

    fn save(&self, users: &BTreeMap<String, ListenerCharts>) {
        let Some(path) = &self.path else {
            return;
        };
        // Written under the lock, so a save never overwrites a newer one.
        match serde_json::to_vec_pretty(users) {
            Ok(json) => {
                let temp = path.with_extension("json.tmp");
                if let Err(e) = std::fs::write(&temp, json).and_then(|()| std::fs::rename(&temp, path)) {
                    warn!("Could not save the chart countries: {e}");
                }
            }
            Err(e) => warn!("Could not save the chart countries: {e}"),
        }
    }

    /// Every listener with choices, by key.
    pub fn all(&self) -> Vec<(String, ListenerCharts)> {
        self.users
            .lock()
            .iter()
            .map(|(user, charts)| (user.clone(), charts.clone()))
            .collect()
    }

    /// Every country some listener chose or pinned, sorted, no repeats.
    pub fn countries(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .users
            .lock()
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

#[cfg(test)]
#[path = "chart_country_store_tests.rs"]
mod tests;
