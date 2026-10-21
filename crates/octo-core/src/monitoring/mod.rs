//! The listening log behind the dashboard's Monitoring page: completed plays summed per day and
//! listener, by genre, platform and app. Only totals are kept, never which songs were played.

use std::collections::BTreeMap;

use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::family::device::app_label;

/// The name a play without a genre (or platform) is counted under.
pub const UNKNOWN: &str = "Unknown";

/// Genres listed in the whole summary; each listener lists their top three.
const TOP_GENRES: usize = 10;
const TOP_GENRES_PER_LISTENER: usize = 3;

/// The log as `listening.json` holds it: day (`YYYY-MM-DD`, UTC), then listener.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ListeningLog {
    pub days: BTreeMap<NaiveDate, BTreeMap<String, DayTotals>>,
}

/// One listener's listening on one day.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct DayTotals {
    pub seconds: u64,
    pub plays: u64,
    pub genres: BTreeMap<String, u64>,
    pub platforms: BTreeMap<String, u64>,
    pub apps: BTreeMap<String, u64>,
}

/// One completed play, as it is counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Play {
    pub user: String,
    pub seconds: u64,
    pub genre: String,
    pub platform: String,
    pub app: String,
}

impl Play {
    /// A play from what the scrobble carried: the song's genre and length, the Subsonic `c`
    /// parameter and the User-Agent. A blank genre counts as [`UNKNOWN`].
    pub fn new(
        user: &str,
        seconds: Option<i32>,
        genre: Option<&str>,
        client: &str,
        user_agent: &str,
    ) -> Play {
        let genre = genre.map(str::trim).filter(|g| !g.is_empty()).unwrap_or(UNKNOWN);
        Play {
            user: user.to_string(),
            seconds: seconds.map_or(0, |s| s.max(0) as u64),
            genre: genre.to_string(),
            platform: platform_of(client, user_agent).to_string(),
            // Without the User-Agent the label carries no version, so one app is one row.
            app: app_label(client, ""),
        }
    }
}

/// The platform a player runs on, from its User-Agent, else from apps known to run on one only.
pub fn platform_of(client: &str, user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| ua.contains(w));
    if has(&["android"]) {
        return "Android";
    }
    if has(&["iphone", "ipad", "ipod", "ios"]) {
        return "iOS";
    }
    if has(&["macintosh", "mac os", "macos"]) {
        return "macOS";
    }
    if has(&["windows"]) {
        return "Windows";
    }
    if has(&["linux", "x11"]) {
        return "Linux";
    }
    // Apple's network stack names Darwin, which on a phone player is iOS.
    if has(&["cfnetwork", "darwin"]) {
        return "iOS";
    }
    let client = client.to_ascii_lowercase();
    let starts = |names: &[&str]| names.iter().any(|n| client.starts_with(n));
    if starts(&["amperfy", "play:sub", "isub"]) {
        return "iOS";
    }
    if starts(&["dsub", "ultrasonic", "symfonium", "tempo", "subtracks"]) {
        return "Android";
    }
    UNKNOWN
}

impl ListeningLog {
    /// Counts `play` on `day`.
    pub fn record(&mut self, day: NaiveDate, play: &Play) {
        let totals = self
            .days
            .entry(day)
            .or_default()
            .entry(play.user.clone())
            .or_default();
        totals.seconds += play.seconds;
        totals.plays += 1;
        *totals.genres.entry(play.genre.clone()).or_default() += play.seconds;
        *totals.platforms.entry(play.platform.clone()).or_default() += play.seconds;
        *totals.apps.entry(play.app.clone()).or_default() += play.seconds;
    }

    /// Drops the days before the last `keep_days` up to `today`. True when any went.
    pub fn prune(&mut self, today: NaiveDate, keep_days: u32) -> bool {
        let Some(first) = first_day(today, keep_days) else {
            return false;
        };
        let before = self.days.len();
        self.days = self.days.split_off(&first);
        self.days.len() != before
    }

    /// The listening of the last `days` days up to `today`, or of every day kept when `days` is 0.
    pub fn summarize(&self, today: NaiveDate, days: u32) -> Summary {
        let first = match days {
            0 => self.days.keys().next().copied().unwrap_or(today).min(today),
            n => first_day(today, n).unwrap_or(NaiveDate::MIN),
        };
        let mut users: BTreeMap<&str, DayTotals> = BTreeMap::new();
        let mut daily = Vec::new();
        let mut date = first;
        while date <= today {
            let mut day = DailyTotal {
                date,
                seconds: 0,
                plays: 0,
            };
            for (user, totals) in self.days.get(&date).into_iter().flatten() {
                day.seconds += totals.seconds;
                day.plays += totals.plays;
                add(users.entry(user).or_default(), totals);
            }
            daily.push(day);
            date = match date.succ_opt() {
                Some(next) => next,
                None => break,
            };
        }
        let mut all = DayTotals::default();
        for totals in users.values() {
            add(&mut all, totals);
        }
        let mut users: Vec<UserSummary> = users
            .into_iter()
            .map(|(user, t)| UserSummary {
                user: user.to_string(),
                seconds: t.seconds,
                plays: t.plays,
                genres: ranked(&t.genres, TOP_GENRES_PER_LISTENER),
                platforms: ranked(&t.platforms, usize::MAX),
                apps: ranked(&t.apps, usize::MAX),
            })
            .collect();
        users.sort_by(|a, b| b.seconds.cmp(&a.seconds).then_with(|| a.user.cmp(&b.user)));
        Summary {
            days,
            totals: Totals {
                seconds: all.seconds,
                plays: all.plays,
                listeners: users.len(),
            },
            daily,
            users,
            genres: ranked(&all.genres, TOP_GENRES),
            platforms: ranked(&all.platforms, usize::MAX),
            apps: ranked(&all.apps, usize::MAX),
        }
    }
}

/// The first of the last `days` days up to `today`; none when that is before the calendar starts.
fn first_day(today: NaiveDate, days: u32) -> Option<NaiveDate> {
    today.checked_sub_days(Days::new(u64::from(days.max(1)) - 1))
}

fn add(into: &mut DayTotals, from: &DayTotals) {
    into.seconds += from.seconds;
    into.plays += from.plays;
    for (into, from) in [
        (&mut into.genres, &from.genres),
        (&mut into.platforms, &from.platforms),
        (&mut into.apps, &from.apps),
    ] {
        for (name, seconds) in from {
            *into.entry(name.clone()).or_default() += seconds;
        }
    }
}

/// The `top` names with the most seconds, most first (ties by name).
fn ranked(counts: &BTreeMap<String, u64>, top: usize) -> Vec<Share> {
    let mut shares: Vec<Share> = counts
        .iter()
        .map(|(name, seconds)| Share {
            name: name.clone(),
            seconds: *seconds,
        })
        .collect();
    shares.sort_by(|a, b| b.seconds.cmp(&a.seconds).then_with(|| a.name.cmp(&b.name)));
    shares.truncate(top);
    shares
}

/// What the Monitoring page shows for a range of days.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// The range asked for; 0 is every day kept.
    pub days: u32,
    pub totals: Totals,
    /// Every day of the range, oldest first, days without listening included.
    pub daily: Vec<DailyTotal>,
    /// Listeners, most listening first.
    pub users: Vec<UserSummary>,
    pub genres: Vec<Share>,
    pub platforms: Vec<Share>,
    pub apps: Vec<Share>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub seconds: u64,
    pub plays: u64,
    pub listeners: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyTotal {
    pub date: NaiveDate,
    pub seconds: u64,
    pub plays: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSummary {
    pub user: String,
    pub seconds: u64,
    pub plays: u64,
    /// The listener's top three genres.
    pub genres: Vec<Share>,
    pub platforms: Vec<Share>,
    pub apps: Vec<Share>,
}

/// A name and the seconds listened under it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub name: String,
    pub seconds: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        text.parse().expect("a date")
    }

    fn play(user: &str, seconds: i32, genre: &str, client: &str, ua: &str) -> Play {
        Play::new(user, Some(seconds), Some(genre), client, ua)
    }

    #[test]
    fn platforms_come_from_the_user_agent_then_the_app() {
        assert_eq!(
            platform_of("Symfonium", "Symfonium/12.3.0 (Android 15)"),
            "Android"
        );
        assert_eq!(platform_of("Octo", "Octo/1.6 (Android)"), "Android");
        assert_eq!(
            platform_of("Amperfy", "Amperfy/1.4 CFNetwork/1490 Darwin/23.2.0"),
            "iOS"
        );
        assert_eq!(platform_of("Amperfy", ""), "iOS");
        assert_eq!(
            platform_of(
                "NavidromeUI",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0"
            ),
            "Windows"
        );
        assert_eq!(
            platform_of("feishin", "Mozilla/5.0 (X11; Linux x86_64) Feishin/0.12"),
            "Linux"
        );
        assert_eq!(platform_of("someplayer", "curl/8.5"), UNKNOWN);
    }

    #[test]
    fn a_play_names_its_app_without_a_version_and_a_blank_genre_as_unknown() {
        let play = Play::new(
            "ann",
            Some(200),
            Some("  "),
            "Symfonium",
            "Symfonium/12.3.0 (Android 15)",
        );
        assert_eq!(play.app, "Symfonium");
        assert_eq!(play.genre, UNKNOWN);
        assert_eq!(play.seconds, 200);
        assert_eq!(Play::new("ann", Some(-5), None, "x", "").seconds, 0);
    }

    #[test]
    fn plays_add_up_per_listener_and_the_range_counts_only_its_days() {
        let mut log = ListeningLog::default();
        let android = "Symfonium/12.3.0 (Android 15)";
        log.record(
            day("2026-10-01"),
            &play("ann", 3600, "Rock", "Symfonium", android),
        );
        log.record(day("2026-10-19"), &play("ann", 200, "Jazz", "Symfonium", android));
        log.record(day("2026-10-20"), &play("ann", 300, "Rock", "Symfonium", android));
        log.record(day("2026-10-20"), &play("bob", 1000, "Pop", "Amperfy", ""));

        let week = log.summarize(day("2026-10-20"), 7);
        assert_eq!(week.daily.len(), 7);
        assert_eq!(week.daily[0].date, day("2026-10-14"));
        assert_eq!(week.daily[6].seconds, 1300);
        assert_eq!(week.totals.seconds, 1500);
        assert_eq!(week.totals.plays, 3);
        assert_eq!(week.totals.listeners, 2);
        assert_eq!(week.users[0].user, "bob");
        assert_eq!(week.users[1].genres[0].name, "Rock");
        assert_eq!(week.users[1].platforms[0].name, "Android");
        assert_eq!(week.platforms[0].name, "iOS");
        assert_eq!(week.genres.len(), 3);

        let all = log.summarize(day("2026-10-20"), 0);
        assert_eq!(all.daily.len(), 20);
        assert_eq!(all.totals.seconds, 5100);
        assert_eq!(all.genres[0].name, "Rock");
    }

    #[test]
    fn an_empty_log_summarizes_to_empty_days() {
        let log = ListeningLog::default();
        let all = log.summarize(day("2026-10-20"), 0);
        assert_eq!(all.daily.len(), 1);
        assert_eq!(all.totals.listeners, 0);
        assert!(all.users.is_empty());
    }

    #[test]
    fn prune_keeps_exactly_the_retention_days() {
        let mut log = ListeningLog::default();
        for d in ["2026-10-13", "2026-10-14", "2026-10-20"] {
            log.record(day(d), &play("ann", 60, "Rock", "x", ""));
        }
        assert!(log.prune(day("2026-10-20"), 7));
        assert_eq!(
            log.days.keys().copied().collect::<Vec<_>>(),
            [day("2026-10-14"), day("2026-10-20")]
        );
        assert!(!log.prune(day("2026-10-20"), 7));
    }

    #[test]
    fn the_log_round_trips_through_json() {
        let mut log = ListeningLog::default();
        log.record(day("2026-10-20"), &play("ann", 60, "Rock", "x", ""));
        let text = serde_json::to_string(&log).expect("serialises");
        assert!(text.starts_with("{\"Days\":{\"2026-10-20\":{\"ann\":{\"Seconds\":60"));
        assert_eq!(serde_json::from_str::<ListeningLog>(&text).expect("reads"), log);
    }
}
