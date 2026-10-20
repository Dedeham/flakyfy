//! The dashboard's Monitoring page: `GET /api/admin/monitoring?days=N` sums the listening log
//! over the last N days (0: every day kept, the default 30), and `DELETE` forgets it. Behind the
//! sign-in gate like every admin route; the delete needs the admin header like every write.

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use chrono::Utc;
use octo_core::settings::monitoring::MAX_RETENTION_DAYS;
use serde_json::json;
use tracing::info;

use super::plumbing::{error_json, ok, query_value};
use crate::app::AppState;
use crate::http::routes::RouteSet;

/// The range when none is asked for.
const DEFAULT_DAYS: u32 = 30;

pub fn routes() -> RouteSet {
    RouteSet::new().route(
        "/api/admin/monitoring",
        get(get_monitoring).delete(clear_monitoring),
    )
}

pub async fn get_monitoring(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let days = match query_value(query.as_deref(), "days") {
        None => DEFAULT_DAYS,
        Some(text) => match text.trim().parse::<u32>() {
            Ok(days) => days.min(MAX_RETENTION_DAYS),
            Err(_) => return error_json(StatusCode::BAD_REQUEST, "days must be a whole number."),
        },
    };
    let monitoring = state.settings.current().monitoring.clone();
    let summary = state.listening.summary(Utc::now(), days);
    ok(&json!({
        "enabled": monitoring.enabled,
        "retentionDays": monitoring.effective_retention_days(),
        "summary": summary,
    }))
}

pub async fn clear_monitoring(State(state): State<AppState>) -> Response {
    state.listening.clear();
    info!("MONITORING the listening log was cleared");
    ok(&json!({ "ok": true }))
}
