//! The dashboard's Listening page: everyone's plays, or one person's, and clearing them.

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;
use tracing::info;

use super::plumbing::{error_json, ok, query_value};
use crate::app::AppState;
use crate::middleware::admin_sign_in_gate::{Caller, caller_of};
use crate::services::admin::{AdminCaller, AdminCallerKind};
use crate::services::listening::{self, rollups};

/// `GET /api/admin/listening?range=7|30|90[&user=]`: everyone's listening with the people
/// grid, or one person's. The Octo app with a listener's own sign-in sees that listener only.
pub async fn get_listening(
    State(state): State<AppState>,
    caller: Caller,
    RawQuery(query): RawQuery,
) -> Response {
    let days = rollups::range_of(query_value(query.as_deref(), "range").as_deref());
    let listener = caller_of(&caller)
        .filter(|caller| caller.kind == AdminCallerKind::Listener)
        .and_then(|caller| caller.user.clone());
    let log = state.listening.clone();
    let everyone = listener.is_none();
    let user = listener.or_else(|| query_value(query.as_deref(), "user"));
    let answer = tokio::task::spawn_blocking(move || {
        let mut page = listening::page(&log, days, user.as_deref(), everyone);
        page["everyone"] = json!(everyone);
        page["keepDays"] = json!(state.settings.current().listening.effective_keep_days());
        page
    })
    .await;
    match answer {
        Ok(page) => ok(&page),
        Err(_) => error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The listening log could not be read.",
        ),
    }
}

/// `POST /api/admin/listening/clear[?user=]`: removes one person's plays, or everyone's.
pub async fn clear_listening(
    State(state): State<AppState>,
    caller: Caller,
    RawQuery(query): RawQuery,
) -> Response {
    if caller_of(&caller).is_some_and(|caller| !caller.is_admin()) {
        return error_json(StatusCode::FORBIDDEN, "That needs a Navidrome admin account.");
    }
    let user = query_value(query.as_deref(), "user");
    let who = AdminCaller::describe(caller_of(&caller));
    let log = state.listening.clone();
    let target = user.clone();
    let removed = tokio::task::spawn_blocking(move || log.clear(target.as_deref()))
        .await
        .unwrap_or(0);
    match user.as_deref() {
        Some(user) => info!("Listening: {who} cleared {user}'s history ({removed} plays)"),
        None => info!("Listening: {who} cleared everyone's history ({removed} plays)"),
    }
    ok(&json!({ "cleared": removed }))
}
