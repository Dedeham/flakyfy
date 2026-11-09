//! The songs the YouTube check could not settle: the list, each song's file and video to hear them
//! side by side, a person's answers, and the rules those answers suggest.
//!
//! Session-gated like the notices, because these name files. An answer, a further look and a rule
//! need a Navidrome admin's own sign-in, on the allowed list. Nothing here replaces a file: an
//! answer of "wrong" only raises a Review question, where Replace now keeps its own gates.

use std::path::Path as FilePath;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use flakyfy_core::models::domain::Song;
use serde::Deserialize;
use serde_json::{Value, json};

use super::health::{navidrome_user_of, not_signed_in};
use super::json_body::{FromBody, Shape, text_record};
use super::request_binding::{bind_body, browse_user, error, ok, query, sign_in, split};
use crate::app::AppState;
use crate::controllers::subsonic::media::serve_path;
use crate::http::routes::RouteSet;
use crate::services::library::notice_queue::{NoticeOrigin, NoticeQueue, NoticeState};
use crate::services::library::unsure_songs::{UnsureLabel, UnsureSong, UnsureState, report, suggestions};

/// What a Review question raised by a person's answer says.
pub const HEARD_AS_WRONG: &str = "You heard this as another song";

pub fn routes() -> RouteSet {
    RouteSet::new()
        .route("/api/admin/youtube-check/unsure", get(list))
        .route("/api/admin/youtube-check/unsure/export", get(export))
        .route("/api/admin/youtube-check/unsure/rules/accept", post(accept_rule))
        .route("/api/admin/youtube-check/unsure/rules/reject", post(reject_rule))
        .route("/api/admin/youtube-check/unsure/{id}/audio", get(audio))
        .route("/api/admin/youtube-check/unsure/{id}/preview", get(preview))
        .route("/api/admin/youtube-check/unsure/{id}/decide", post(decide))
        .route(
            "/api/admin/youtube-check/unsure/{id}/look-again",
            post(look_again),
        )
}

fn no_such_song() -> Response {
    error(StatusCode::NOT_FOUND, "That song is not in the list.")
}

fn overview(state: &UnsureState) -> Value {
    json!({
        "songs": state.songs,
        "report": report(state),
        "suggestions": suggestions(state),
        "rules": state.rules,
    })
}

async fn list(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    let browse = browse_user(&state, &parts);
    if !browse.signed_in() {
        return browse.finish(sign_in());
    }
    let body = state.library_youtube_check.unsure().read(overview);
    browse.finish(ok(&body))
}

/// The whole list as a file, for replaying the answers on a bench.
async fn export(State(state): State<AppState>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    let browse = browse_user(&state, &parts);
    if !browse.signed_in() {
        return browse.finish(sign_in());
    }
    let body = state.library_youtube_check.unsure().read(overview);
    let mut response = ok(&body);
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"unsure-songs.json\""),
    );
    browse.finish(response)
}

/// The song's file, only when it really lies inside the music folder.
async fn audio(State(state): State<AppState>, Path(id): Path<u64>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    let browse = browse_user(&state, &parts);
    if !browse.signed_in() {
        return browse.finish(sign_in());
    }
    let check = &state.library_youtube_check;
    let Some(song) = check.unsure().song(id) else {
        return browse.finish(no_such_song());
    };
    let full = check.full_path_of(&song.path);
    let root = check.full_path_of("");
    let (real, real_root) = (
        tokio::fs::canonicalize(&full).await,
        tokio::fs::canonicalize(&root).await,
    );
    let (Ok(real), Ok(real_root)) = (real, real_root) else {
        return browse.finish(no_such_song());
    };
    if !real.starts_with(&real_root) || !FilePath::new(&real).is_file() {
        return browse.finish(no_such_song());
    }
    let answer = match serve_path(&parts.headers, &real.to_string_lossy()).await {
        Ok(response) => response,
        Err(_) => no_such_song(),
    };
    browse.finish(answer)
}

/// One of the song's own videos, through the shim. Never any other, so this is no open relay.
async fn preview(State(state): State<AppState>, Path(id): Path<u64>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    let browse = browse_user(&state, &parts);
    if !browse.signed_in() {
        return browse.finish(sign_in());
    }
    let Some(song) = state.library_youtube_check.unsure().song(id) else {
        return browse.finish(no_such_song());
    };
    let video = query(&parts.uri, "video").unwrap_or_default();
    if !song.videos.iter().any(|known| known.video_id == video) {
        return browse.finish(error(
            StatusCode::BAD_REQUEST,
            "That video is not one of this song's.",
        ));
    }
    let range = parts.headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let Some(stream) = state.you_tube_resolver.open_stream(&video, range).await else {
        return browse.finish(error(StatusCode::BAD_GATEWAY, "YouTube did not send that video."));
    };
    let mut response = Response::new(Body::from_stream(stream.response.bytes_stream()));
    *response.status_mut() = StatusCode::from_u16(stream.status_code).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = response.headers_mut();
    if let Ok(kind) = HeaderValue::from_str(&stream.content_type) {
        headers.insert(header::CONTENT_TYPE, kind);
    }
    if let Some(length) = stream.content_length {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    }
    if let Some(range) = stream.content_range.and_then(|r| HeaderValue::from_str(&r).ok()) {
        headers.insert(header::CONTENT_RANGE, range);
    }
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    browse.finish(response)
}

/// The person answering: signed in with Navidrome and on the allowed list, or the refusal.
fn answering_user(state: &AppState, parts: &axum::http::request::Parts) -> Result<String, Box<Response>> {
    let Some(user) = navidrome_user_of(state, parts) else {
        return Err(Box::new(not_signed_in()));
    };
    if !state.settings.current().library_actions.is_allowed(Some(&user)) {
        return Err(Box::new(error(
            StatusCode::FORBIDDEN,
            format!("{user} is not on the library actions allowlist."),
        )));
    }
    Ok(user)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DecideRequest {
    label: Option<String>,
    video_id: Option<String>,
}

impl FromBody for DecideRequest {
    const SHAPE: &'static Shape = text_record!("UnsureDecideRequest", "label", "videoId");
}

fn label_of(text: &str) -> Option<UnsureLabel> {
    match text {
        "right" => Some(UnsureLabel::Right),
        "wrong" => Some(UnsureLabel::Wrong),
        "version" => Some(UnsureLabel::Version),
        "preview_wrong" => Some(UnsureLabel::PreviewWrong),
        "cant_tell" => Some(UnsureLabel::CantTell),
        _ => None,
    }
}

/// A person's answer, or with no label one taken back.
async fn decide(State(state): State<AppState>, Path(id): Path<u64>, request: Request) -> Response {
    let (parts, body) = split(request).await;
    let request: DecideRequest = match bind_body(&parts, &body) {
        Ok(request) => request,
        Err(answer) => return *answer,
    };
    let user = match answering_user(&state, &parts) {
        Ok(user) => user,
        Err(answer) => return *answer,
    };
    let label = match request.label.as_deref().filter(|text| !text.is_empty()) {
        None => None,
        Some(text) => match label_of(text) {
            Some(label) => Some(label),
            None => return error(StatusCode::BAD_REQUEST, "That is not an answer Flakyfy knows."),
        },
    };
    let check = state.library_youtube_check.clone();
    let now = chrono::Utc::now();
    let Some(song) = check
        .unsure()
        .decide(id, label, request.video_id.as_deref(), &user, now)
    else {
        return no_such_song();
    };
    let full = check.full_path_of(&song.path);
    match label {
        Some(UnsureLabel::Right) => close_question(&state, &user, &full),
        Some(UnsureLabel::Wrong) => {
            state
                .notice_queue
                .add_library_question(&user, &full, &song_of(&song), HEARD_AS_WRONG);
        }
        Some(UnsureLabel::PreviewWrong) => {
            tokio::spawn(async move {
                check.look_again(id).await;
            });
        }
        _ => {}
    }
    ok(&song)
}

/// A person heard the song and it is right: an open question about it from a library check is
/// answered too.
fn close_question(state: &AppState, user: &str, full: &str) {
    let key = NoticeQueue::review_key(user, full);
    if state
        .notice_queue
        .open_review(&key)
        .is_some_and(|entry| entry.origin == NoticeOrigin::LibrarySweep)
    {
        state.notice_queue.resolve(&key, NoticeState::Kept);
    }
}

fn song_of(song: &UnsureSong) -> Song {
    Song {
        artist: song.artist.clone(),
        title: song.title.clone(),
        album: song.album.clone(),
        duration: song.seconds,
        ..Default::default()
    }
}

/// Compares the song with more of its videos, and only adds what they sound like.
async fn look_again(State(state): State<AppState>, Path(id): Path<u64>, request: Request) -> Response {
    let (parts, _) = request.into_parts();
    if let Err(answer) = answering_user(&state, &parts) {
        return *answer;
    }
    let check = &state.library_youtube_check;
    let Some(found) = check.look_again(id).await else {
        return no_such_song();
    };
    ok(&json!({ "found": found, "song": check.unsure().song(id) }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RuleRequest {
    id: Option<String>,
}

impl FromBody for RuleRequest {
    const SHAPE: &'static Shape = text_record!("UnsureRuleRequest", "id");
}

async fn accept_rule(State(state): State<AppState>, request: Request) -> Response {
    rule_answer(&state, request, true).await
}

async fn reject_rule(State(state): State<AppState>, request: Request) -> Response {
    rule_answer(&state, request, false).await
}

async fn rule_answer(state: &AppState, request: Request, accept: bool) -> Response {
    let (parts, body) = split(request).await;
    let request: RuleRequest = match bind_body(&parts, &body) {
        Ok(request) => request,
        Err(answer) => return *answer,
    };
    if let Err(answer) = answering_user(state, &parts) {
        return *answer;
    }
    let id = request.id.unwrap_or_default();
    let unsure = state.library_youtube_check.unsure();
    let now = chrono::Utc::now();
    let done = if accept {
        unsure.accept_rule(&id, now)
    } else {
        unsure.reject_rule(&id, now)
    };
    if !done {
        return error(StatusCode::CONFLICT, "That suggestion no longer stands.");
    }
    ok(&unsure.read(overview))
}

#[cfg(test)]
#[path = "unsure_tests.rs"]
mod tests;
