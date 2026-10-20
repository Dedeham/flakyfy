//! Listening: each person's completed plays, kept on this server for their Listening page
//! (the dashboard's for the admin, the Family page's for members), and the page worked out
//! from them.

pub mod genres;
pub mod listening_log;
pub mod rollups;

use std::sync::Arc;

use serde_json::{Value, json};

pub use genres::AlbumLookup;
pub use listening_log::{ListeningLog, Play, UNKNOWN_APP, UNKNOWN_PLATFORM, platform_of};

use crate::services::last_fm::LastFmService;

/// A completed play waiting for its genre.
pub struct Completed {
    pub user: String,
    pub play: Play,
    /// The song's own genres, first one first.
    pub genres: Vec<String>,
    /// The library album to ask when the song has none.
    pub album: Option<AlbumLookup>,
}

/// Works out the play's genre and writes it down, after the caller has answered: the album
/// and Last.fm are asked only here, never on the way to the reply.
pub fn record_in_background(log: Arc<ListeningLog>, last_fm: Arc<LastFmService>, completed: Completed) {
    tokio::spawn(async move {
        let Completed {
            user,
            mut play,
            genres,
            album,
        } = completed;
        let artist = play.album_artist.clone().unwrap_or_else(|| play.artist.clone());
        play.genre = genres::resolve(&log, &last_fm, &genres, album, &artist).await;
        log.record(&user, play);
    });
}

/// The name of the server's time zone, as the page says it: the `TZ` city ("Los Angeles"), or
/// "UTC".
pub fn time_zone_label() -> String {
    let named = std::env::var("TZ")
        .ok()
        .map(|tz| tz.trim().trim_start_matches(':').to_string());
    if let Some(tz) = named.filter(|tz| !tz.is_empty()) {
        if matches!(
            tz.as_str(),
            "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Universal" | "Zulu"
        ) {
            return "UTC".to_string();
        }
        let city = tz.rsplit('/').next().unwrap_or(&tz).replace('_', " ");
        return city;
    }
    let offset = chrono::Local::now().offset().local_minus_utc();
    if offset == 0 {
        "UTC".to_string()
    } else {
        let sign = if offset < 0 { '-' } else { '+' };
        let minutes = offset.abs() / 60;
        format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
    }
}

/// The Listening page's data: one person's (`user`) or everyone's, for `days`, with the people
/// grid when `with_people`.
pub fn page(log: &ListeningLog, days: i64, user: Option<&str>, with_people: bool) -> Value {
    page_in(log, days, user, with_people, &chrono::Local, time_zone_label())
}

/// [`page`] in a given time zone, for tests.
pub fn page_in<Tz: chrono::TimeZone>(
    log: &ListeningLog,
    days: i64,
    user: Option<&str>,
    with_people: bool,
    tz: &Tz,
    tz_label: String,
) -> Value {
    let now = log.now();
    let settings_on = log.recording();
    let (name, rollup) = match user.map(str::trim).filter(|u| !u.is_empty()) {
        Some(user) => {
            let name = log.name_of(user);
            let plays = log.plays_of(user);
            let rows: Vec<(&str, &Play)> = plays.iter().map(|p| (name.as_str(), p)).collect();
            (Some(name.clone()), rollups::rollup(&rows, now, days, tz))
        }
        None => {
            let names: std::collections::HashMap<String, String> = log.people().into_iter().collect();
            let all = log.all_plays();
            let rows: Vec<(&str, &Play)> = all
                .iter()
                .flat_map(|(key, plays)| {
                    let name = names.get(key).map_or(key.as_str(), String::as_str);
                    plays.iter().map(move |p| (name, p))
                })
                .collect();
            (None, rollups::rollup(&rows, now, days, tz))
        }
    };
    let people: Option<Vec<rollups::Person>> = with_people.then(|| {
        let mut people: Vec<rollups::Person> = log
            .people()
            .into_iter()
            .map(|(key, name)| rollups::person(&name, &log.plays_of(&key), now, days, tz))
            .collect();
        people.sort_by(|a, b| b.hours.total_cmp(&a.hours).then(a.name.cmp(&b.name)));
        people
    });
    json!({
        "range": days,
        "user": name,
        "people": people,
        "recording": settings_on,
        "timeZone": tz_label,
        "rollup": rollup,
    })
}
