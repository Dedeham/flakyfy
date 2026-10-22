//! The YouTube check's list: the songs it heard as another song or could not tell, worst first,
//! with how far each is from its preview, and what can be done with the ones picked: replace them
//! through Wrong song, keep them as they are, or stop listing them. Nothing here runs by itself.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use octo_core::settings::LibraryAction;
use serde::Deserialize;
use serde_json::{Value, json};

use super::health::{navidrome_user_of, not_signed_in};
use super::json_body::{FromBody, Kind, Member, Shape};
use super::plumbing::{accepted, ok};
use super::request_binding::{bind_body, error, query, split};
use crate::app::AppState;
use crate::http::routes::RouteSet;
use crate::services::library::library_youtube_check::{SongFilter, SongRow};
use crate::services::library::notice_queue::NoticeState;

pub fn routes() -> RouteSet {
    RouteSet::new()
        .route("/api/admin/youtube-check/songs", get(list))
        .route("/api/admin/youtube-check/songs/replace", post(replace))
        .route("/api/admin/youtube-check/songs/keep", post(keep))
        .route("/api/admin/youtube-check/songs/dismiss", post(dismiss))
}

/// `{ "ids": [...] }`: the songs a request is about, by the ids the list gave.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SongsRequest {
    ids: Option<Vec<String>>,
}

impl FromBody for SongsRequest {
    const SHAPE: &'static Shape = &Shape {
        name: "Octo.Controllers.YoutubeCheckSongs+SongsRequest",
        by_constructor: true,
        members: &[Member {
            name: "ids",
            kind: Kind::TextList,
        }],
    };
}

async fn bound(request: Request) -> Result<(axum::http::request::Parts, SongsRequest), Box<Response>> {
    let (parts, body) = split(request).await;
    let bound: SongsRequest = bind_body(&parts, &body)?;
    Ok((parts, bound))
}

/// The video of a preview on YouTube Music, for listening to what a song was held against. Only
/// an id of the shape YouTube uses is turned into an address.
pub(super) fn reference_url(video_id: &str) -> Option<String> {
    let plain = (6..=20).contains(&video_id.len())
        && video_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| format!("https://music.youtube.com/watch?v={video_id}"))
}

fn row_json(row: &SongRow) -> Value {
    let result = &row.result;
    json!({
        "id": row.id,
        "artist": result.artist,
        "title": result.title,
        "bitError": result.bit_error,
        "percentDiffer": (result.bit_error * 100.0).round(),
        "heard": if result.clearly_wrong { "wrong" } else { "unclear" },
        "trusted": result.trusted,
        "referenceUrl": reference_url(&result.video_id),
    })
}

fn count_text(count: usize) -> String {
    if count == 1 {
        "1 song".to_string()
    } else {
        format!("{count} songs")
    }
}

/// `GET /api/admin/youtube-check/songs?filter=wrong|unclear|all&offset=&limit=`: a page of the
/// list, the most different first, and how many songs each filter holds.
async fn list(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    if navidrome_user_of(&state, &parts).is_none() {
        return not_signed_in();
    }
    let filter = SongFilter::parse(query(&parts.uri, "filter").as_deref());
    let number = |name: &str| query(&parts.uri, name).and_then(|value| value.trim().parse::<usize>().ok());
    let offset = number("offset").unwrap_or(0);
    let limit = number("limit").unwrap_or(50).clamp(1, 200);
    let page = state.library_youtube_check.songs(filter, offset, limit).await;
    ok(&json!({
        "songs": page.rows.iter().map(row_json).collect::<Vec<_>>(),
        "matching": page.matching,
        "wrong": page.wrong,
        "unclear": page.unclear,
        "offset": offset,
        "limit": limit,
    }))
}

/// Replaces the picked songs through Wrong song, one after the other: the file goes to Recently
/// removed and the right one is looked for, held to the download checks. A song anyone starred
/// or rated is left out and said so.
async fn replace(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    let Some(user) = navidrome_user_of(&state, &parts) else {
        return not_signed_in();
    };
    let can = state.library_health.abilities(&user);
    if !can.edit {
        return error(
            StatusCode::FORBIDDEN,
            can.why.as_deref().unwrap_or("Octo may not change files for you."),
        );
    }
    let actions = state.settings.current().library_actions.clone();
    let refusal = if !actions.enabled {
        Some("Library actions are off. Turn them on to replace songs.".to_string())
    } else if !actions.is_allowed(Some(&user)) {
        Some(format!("{user} is not on the library actions allowlist."))
    } else if !actions
        .effective_actions()
        .into_iter()
        .any(|entry| entry.action == LibraryAction::WrongSong && entry.enabled)
    {
        Some("The Wrong song action is off under Library actions. Turn it on to replace songs.".to_string())
    } else {
        None
    };
    if let Some(refusal) = refusal {
        return error(StatusCode::BAD_REQUEST, &refusal);
    }
    let ids = body.ids.unwrap_or_default();
    if ids.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Say which songs to replace.");
    }
    match state.library_youtube_check.replace_selected(&user, &ids).await {
        Ok(report) => {
            let skipped: Vec<Value> = report
                .skipped
                .iter()
                .map(|(id, why)| json!({ "id": id, "why": why }))
                .collect();
            let mut words = if report.queued == 0 {
                "Nothing was replaced.".to_string()
            } else if actions.dry_run {
                format!(
                    "Dry run: {} would be replaced. Nothing is changed.",
                    count_text(report.queued)
                )
            } else {
                format!(
                    "Looking for the right version of {}. Each one shows in Downloads; the old file goes to Recently removed.",
                    count_text(report.queued)
                )
            };
            if !skipped.is_empty() {
                words.push_str(&format!(" {} left out.", count_text(skipped.len())));
            }
            accepted(&json!({
                "ok": true,
                "queued": report.queued,
                "skipped": skipped,
                "dryRun": actions.dry_run,
                "words": words,
            }))
        }
        Err(why) => error(StatusCode::CONFLICT, why),
    }
}

async fn settle(state: AppState, request: Request, how: NoticeState, done: &str) -> Response {
    let (parts, body) = match bound(request).await {
        Ok(bound) => bound,
        Err(answer) => return *answer,
    };
    if navidrome_user_of(&state, &parts).is_none() {
        return not_signed_in();
    }
    let ids = body.ids.unwrap_or_default();
    if ids.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Say which songs.");
    }
    let settled = state.library_youtube_check.settle(&ids, how);
    ok(&json!({
        "ok": true,
        "settled": settled,
        "words": format!("{} {done}.", count_text(settled)),
    }))
}

/// Keeps the picked songs as they are: off the list, and their Review question answered.
async fn keep(State(state): State<AppState>, request: Request) -> Response {
    settle(state, request, NoticeState::Kept, "kept").await
}

/// Takes the picked songs off the list and stops asking about them. Nothing else changes.
async fn dismiss(State(state): State<AppState>, request: Request) -> Response {
    settle(state, request, NoticeState::Dismissed, "dismissed").await
}
