//! The logo the dashboard, the family pages and the badge on outside covers show. The picture
//! itself is served at its usual address (`static_files`), which needs no sign-in, because the
//! sign-in pages show it. Changing it needs the dashboard's sign-in, as every `/api/admin` call.

use axum::extract::multipart::MultipartError;
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use flakyfy_media::cover::custom_logo::{MAX_UPLOAD_BYTES, normalize};
use serde_json::json;
use tracing::warn;

use super::plumbing::{error_json, ok};
use crate::app::AppState;
use crate::http::error::{AppError, AppResult};
use crate::http::routes::RouteSet;

/// Room for the form around the largest file taken.
const FORM_OVERHEAD: usize = 64 * 1024;
const TOO_LARGE: &str = "That picture is over 2 MB. Choose a smaller one.";
const NOT_A_PICTURE: &str = "Choose a PNG, JPEG, WebP or GIF picture, at most 4096 pixels wide and tall.";
const NOT_SAVED: &str = "The logo could not be saved. Check that Flakyfy can write to its config folder.";

pub fn routes() -> RouteSet {
    RouteSet::new().route(
        "/api/admin/branding/logo",
        get(status)
            .post(upload)
            .delete(reset)
            .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + FORM_OVERHEAD)),
    )
}

async fn status(State(state): State<AppState>) -> Response {
    ok(&json!({ "custom": state.cover_art_service.has_custom_logo() }))
}

fn unreadable_form(error: &MultipartError) -> Response {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return error_json(StatusCode::PAYLOAD_TOO_LARGE, TOO_LARGE);
    }
    error_json(StatusCode::BAD_REQUEST, NOT_A_PICTURE)
}

async fn upload(State(state): State<AppState>, request: Request) -> AppResult {
    let Ok(mut form) = Multipart::from_request(request, &state).await else {
        return Ok(error_json(StatusCode::BAD_REQUEST, NOT_A_PICTURE));
    };
    let mut file = None;
    loop {
        let field = match form.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return Ok(unreadable_form(&error)),
        };
        if file.is_some() || !field.name().is_some_and(|n| n.eq_ignore_ascii_case("file")) {
            continue;
        }
        match field.bytes().await {
            Ok(bytes) => file = Some(bytes),
            Err(error) => return Ok(unreadable_form(&error)),
        }
    }
    let Some(file) = file else {
        return Ok(error_json(StatusCode::BAD_REQUEST, NOT_A_PICTURE));
    };
    if file.len() > MAX_UPLOAD_BYTES {
        return Ok(error_json(StatusCode::PAYLOAD_TOO_LARGE, TOO_LARGE));
    }
    let covers = state.cover_art_service.clone();
    // Decoding a picture is slow work, so it stays off the async threads.
    let answer = tokio::task::spawn_blocking(move || {
        let Some(png) = normalize(&file) else {
            return error_json(StatusCode::BAD_REQUEST, NOT_A_PICTURE);
        };
        match covers.save_custom_logo(&png) {
            Ok(()) => ok(&json!({ "custom": true })),
            Err(error) => {
                warn!(error = %error, "Could not save the custom logo");
                error_json(StatusCode::INTERNAL_SERVER_ERROR, NOT_SAVED)
            }
        }
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
    Ok(answer)
}

async fn reset(State(state): State<AppState>) -> AppResult {
    let covers = state.cover_art_service.clone();
    let answer = tokio::task::spawn_blocking(move || match covers.remove_custom_logo() {
        Ok(()) => ok(&json!({ "custom": false })),
        Err(error) => {
            warn!(error = %error, "Could not remove the custom logo");
            error_json(StatusCode::INTERNAL_SERVER_ERROR, NOT_SAVED)
        }
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))?;
    Ok(answer)
}

#[cfg(test)]
#[path = "branding_tests.rs"]
mod tests;
