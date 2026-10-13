//! getStationSongs (octoStations): a radio station's songs a page at a time. The station's own
//! songs come first; after them the station goes on with new songs of its kind for as long as a
//! client keeps asking, never one the client already queued.

use std::collections::HashSet;

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use octo_core::models::domain::Song;
use octo_core::models::radio::{LastFmRadioStation, LastFmRadioStationKind, LastFmRadioTrack};
use octo_core::soulseek::RoutingKind;

use super::action_helpers::{
    RequestAborted, SubsonicCall, int_param, materialize_tracks, playlist_stations, queue_refresh_if_stale,
    serve_station_songs, suggested_sources,
};
use super::playlists::signed_in;
use crate::app::AppState;
use crate::http::routes::RouteSet;

/// Songs answered when the call does not say how many.
const DEFAULT_COUNT: i32 = 50;

/// The most songs one call answers.
const MOST_SONGS: i32 = 100;

/// How many of the queued ids are read, the newest kept.
const AFTER_READ: usize = 200;

/// How many times the station is asked within one call when what it gave resolves to too few
/// songs (outside songs not found, explicit songs filtered, back-to-back artists dropped).
const ROUNDS: usize = 3;

/// The answer for a station that is not the caller's, not there, or not served.
const GONE: &str = "That station isn't on this server any more.";

pub fn routes() -> RouteSet {
    RouteSet::new().subsonic("getStationSongs", get(get_station_songs).post(get_station_songs))
}

/// Whether octoStations is listed: while radio is on and its stations are served as playlists,
/// the ids getStationSongs takes.
pub fn offered(state: &AppState) -> bool {
    let settings = state.settings.current();
    settings.last_fm.enable_radio && settings.last_fm.expose_radio_as_playlists
}

/// `count` songs (1 to 100, 50 when not sent) of the caller's station `id`, none of them one of
/// the `after` ids (repeated, the songs already queued, oldest first), shaped as getPlaylist
/// lists a station's songs. Error 70 for a station that is not the caller's or not there.
pub async fn get_station_songs(State(state): State<AppState>, req: Request) -> Response {
    let aborted = RequestAborted::new();
    let response = station_songs(&state, req, &aborted).await;
    aborted.answered();
    response
}

async fn station_songs(state: &AppState, req: Request, aborted: &RequestAborted) -> Response {
    let call = match SubsonicCall::read(state, req).await {
        Ok(call) => call,
        Err(response) => return response,
    };
    let format = call.format.as_str();
    let builder = &state.subsonic_response_builder;
    let id = call.param_or("id", "").trim().to_string();
    if id.is_empty() {
        return builder
            .create_error(format, 10, "Required parameter is missing: id")
            .into_response();
    }
    if !signed_in(state, &call, format).await {
        return builder
            .create_error(format, 40, "Wrong username or password")
            .into_response();
    }
    // A listener's stations are theirs alone: the list is the caller's own, as for getPlaylist.
    let username = call.param_or("u", "").to_string();
    let Some(station) = playlist_stations(state, &username)
        .into_iter()
        .find(|station| station.id == id)
    else {
        return builder.create_error(format, 70, GONE).into_response();
    };
    let count = int_param(&call.parameters, "count", DEFAULT_COUNT).clamp(1, MOST_SONGS) as usize;
    let after = queued_ids(call.param("after"));
    let songs = next_songs(state, &call, &station, &username, &after, count, aborted).await;
    queue_refresh_if_stale(state, &username);
    builder
        .create_station_songs_response(format, &station, &songs)
        .into_response()
}

/// The ids the client already queued, oldest first: `after` repeated or comma-separated, the
/// last [`AFTER_READ`] of them.
fn queued_ids(after: Option<&str>) -> Vec<String> {
    let ids: Vec<String> = after
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    let older = ids.len().saturating_sub(AFTER_READ);
    ids.into_iter().skip(older).collect()
}

/// The station's next `count` songs after `after`, resolved as getPlaylist resolves a station
/// and served the same way.
async fn next_songs(
    state: &AppState,
    call: &SubsonicCall,
    station: &LastFmRadioStation,
    username: &str,
    after: &[String],
    count: usize,
    aborted: &RequestAborted,
) -> Vec<Song> {
    let mut skip: HashSet<String> = after.iter().cloned().collect();
    let mut left_out: Vec<LastFmRadioTrack> = after
        .iter()
        .filter_map(|id| queued_track(state, username, station, id))
        .collect();
    let mut previous: Option<(String, String)> = after
        .last()
        .and_then(|id| queued_track(state, username, station, id))
        .map(|track| (track.artist, track.album.unwrap_or_default()));
    let mut songs: Vec<Song> = Vec::new();
    for _ in 0..ROUNDS {
        let wanted = count - songs.len();
        // Discovery Radio asks for half again as many, to stand in for library songs past its
        // share.
        let asked = if station.kind == LastFmRadioStationKind::Discovery {
            wanted + wanted.div_ceil(2)
        } else {
            wanted
        };
        // Songs served before come round again only when this call has found none at all.
        let recommendations = &state.last_fm_radio_recommendations;
        let picks = if songs.is_empty() {
            recommendations
                .next_for_station(username, &station.id, &left_out, asked, &aborted.token)
                .await
        } else {
            recommendations
                .next_new_for_station(username, &station.id, &left_out, asked, &aborted.token)
                .await
        };
        if picks.is_empty() {
            break;
        }
        let (tracks, reserve) = picks.split_at(wanted.min(picks.len()));
        let resolved = materialize_tracks(
            state,
            &call.proxy,
            station,
            tracks,
            reserve,
            &call.parameters,
            &skip,
            previous
                .as_ref()
                .map(|(artist, album)| (artist.as_str(), album.as_str())),
        )
        .await;
        left_out.extend(picks.iter().cloned());
        for song in resolved {
            if songs.len() >= count {
                break;
            }
            if skip.insert(song.id.clone()) {
                songs.push(song);
            }
        }
        if let Some(last) = songs.last() {
            previous = Some((last.artist.clone(), last.album.clone()));
        }
        if songs.len() >= count {
            break;
        }
    }
    let suggested = suggested_sources(&songs);
    let songs = serve_station_songs(state, username, songs, &suggested);
    state.last_fm_radio_recommendations.record_station_served(
        username,
        &station.id,
        songs.iter().map(served_track),
    );
    songs
}

/// A served song as the station history keeps it.
fn served_track(song: &Song) -> LastFmRadioTrack {
    LastFmRadioTrack {
        artist: song.artist.clone(),
        title: song.title.clone(),
        album: (!song.album.trim().is_empty()).then(|| song.album.clone()),
        duration: song.duration,
        resolved_id: Some(song.id.clone()),
        is_local: song.is_local,
        ..Default::default()
    }
}

/// What a queued id stands for: a song a station served this listener, one of this station's
/// tracks, or an outside song's route. None for an id none of them knows; it is still never
/// answered.
fn queued_track(
    state: &AppState,
    username: &str,
    station: &LastFmRadioStation,
    id: &str,
) -> Option<LastFmRadioTrack> {
    if let Some(track) = state.last_fm_radio_recommendations.served_track(username, id) {
        return Some(track);
    }
    if let Some(track) = station
        .tracks
        .iter()
        .chain(&station.reserve)
        .find(|track| track.resolved_id.as_deref() == Some(id))
    {
        return Some(track.clone());
    }
    let route = state.external_id_registry.lookup(id)?.snapshot();
    let (Some(artist), Some(title)) = (route.artist, route.title) else {
        return None;
    };
    (route.kind == RoutingKind::Song && !artist.is_empty() && !title.is_empty()).then(|| LastFmRadioTrack {
        artist,
        title,
        album: route.album,
        duration: route.duration,
        resolved_id: Some(id.to_string()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_ids_read_repeats_and_commas_and_keep_the_newest_two_hundred() {
        assert_eq!(queued_ids(Some("a, b,,c")), ["a", "b", "c"]);
        assert!(queued_ids(None).is_empty());
        let many: Vec<String> = (0..250).map(|i| format!("id{i}")).collect();
        let read = queued_ids(Some(&many.join(",")));
        assert_eq!(read.len(), AFTER_READ);
        assert_eq!(read.first().map(String::as_str), Some("id50"));
        assert_eq!(read.last().map(String::as_str), Some("id249"));
    }
}
