//! Answers to the apps named in `Subsonic:SizeFlagClients` carry a size on every song they
//! should show (see [`octo_subsonic::size_flag`]). Each answer is read once it is finished, so
//! the rule holds for every endpoint, every cached search and every stored list alike. Every
//! other app's answer passes through untouched, as does anything that is not a Subsonic XML
//! or JSON answer: audio, covers, a compressed relay, a HEAD.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, ETAG, USER_AGENT};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use octo_subsonic::size_flag::{Format, flag_sizes};
use tracing::warn;

use crate::app::AppState;

/// The largest answer read; a larger one passes through untouched.
pub const MAX_BODY: usize = 64 * 1024 * 1024;

pub async fn size_flag(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if request.method() == Method::HEAD || !from_a_size_flag_client(&state, &request) {
        return next.run(request).await;
    }
    flag(next.run(request).await).await
}

/// A Subsonic call (under `/rest/`) from an app the setting names, by its `c` parameter or its
/// User-Agent. As everywhere in a Subsonic call, the key matches ignoring case and a repeated
/// key is all its values joined with commas.
fn from_a_size_flag_client(state: &AppState, request: &Request) -> bool {
    if !request.uri().path().to_ascii_lowercase().contains("/rest/") {
        return false;
    }
    let client = request.uri().query().map(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .filter(|(name, value)| name.eq_ignore_ascii_case("c") && !value.is_empty())
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>()
            .join(",")
    });
    let user_agent = request
        .headers()
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok());
    state
        .settings
        .current()
        .subsonic
        .is_size_flag_client(client.as_deref(), user_agent)
}

async fn flag(response: Response) -> Response {
    let Some(format) = answer_format(response.status(), response.headers()) else {
        return response;
    };
    let (mut parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!("An answer could not be read to give its songs a size: {error}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let Some(flagged) = flag_sizes(&bytes, format) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    parts
        .headers
        .insert(CONTENT_LENGTH, HeaderValue::from(flagged.len()));
    parts.headers.remove(ETAG);
    Response::from_parts(parts, Body::from(flagged))
}

/// The answer's shape when it is a plain 200 Subsonic XML or JSON answer small enough to read.
fn answer_format(status: StatusCode, headers: &HeaderMap) -> Option<Format> {
    if status != StatusCode::OK || headers.contains_key(CONTENT_ENCODING) {
        return None;
    }
    let too_large = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_BODY);
    if too_large {
        return None;
    }
    let content_type = headers.get(CONTENT_TYPE)?.to_str().ok()?.to_ascii_lowercase();
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    match essence {
        "application/xml" | "text/xml" => Some(Format::Xml),
        "application/json" | "text/json" => Some(Format::Json),
        _ => None,
    }
}

#[cfg(test)]
#[path = "size_flag_tests.rs"]
mod tests;
