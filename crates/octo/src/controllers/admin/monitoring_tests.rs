use axum::http::{Method, StatusCode};
use chrono::Utc;
use octo_core::monitoring::Play;
use octo_core::settings::{AppSettings, MonitoringSettings};

use super::test_app::{admin_write, app, get, send};
use crate::app::{AppState, TestServices};

fn state(enabled: bool, sign_in: Option<&str>) -> AppState {
    AppState::for_tests_with(
        AppSettings {
            monitoring: MonitoringSettings {
                enabled,
                ..Default::default()
            },
            ..Default::default()
        },
        TestServices {
            sign_in: sign_in.map(str::to_string),
            ..Default::default()
        },
    )
}

fn play(user: &str, seconds: i32) -> Play {
    Play::new(
        user,
        Some(seconds),
        Some("Rock"),
        "Amperfy",
        "Amperfy/1.4 CFNetwork Darwin",
    )
}

#[tokio::test]
async fn monitoring_sums_the_range_asked_for() {
    let state = state(true, None);
    state.listening.record(&play("listener-a", 3600), Utc::now());
    state.listening.record(&play("listener-b", 600), Utc::now());

    let reply = get(&app(state), "/api/admin/monitoring?days=7").await;

    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["enabled"], true);
    assert_eq!(body["retentionDays"], 365);
    let summary = &body["summary"];
    assert_eq!(summary["days"], 7);
    assert_eq!(summary["totals"]["seconds"], 4200);
    assert_eq!(summary["totals"]["listeners"], 2);
    assert_eq!(summary["daily"].as_array().map(Vec::len), Some(7));
    assert_eq!(summary["users"][0]["user"], "listener-a");
    assert_eq!(summary["users"][0]["genres"][0]["name"], "Rock");
    assert_eq!(summary["platforms"][0]["name"], "iOS");
    assert_eq!(summary["apps"][0]["name"], "Amperfy");
}

#[tokio::test]
async fn monitoring_off_records_nothing_and_says_so() {
    let state = state(false, None);
    state.listening.record(&play("listener-a", 3600), Utc::now());

    let body = get(&app(state), "/api/admin/monitoring").await.json();

    assert_eq!(body["enabled"], false);
    assert_eq!(body["summary"]["days"], 30);
    assert_eq!(body["summary"]["totals"]["plays"], 0);
}

#[tokio::test]
async fn monitoring_refuses_a_range_that_is_not_a_number() {
    let reply = get(&app(state(true, None)), "/api/admin/monitoring?days=week").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn monitoring_needs_the_sign_in() {
    let reply = get(&app(state(true, Some("required"))), "/api/admin/monitoring").await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn clearing_needs_the_admin_header_and_forgets_every_play() {
    let state = state(true, None);
    state.listening.record(&play("listener-a", 3600), Utc::now());
    let app = app(state);

    let refused = send(&app, Method::DELETE, "/api/admin/monitoring", &[], None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let kept = get(&app, "/api/admin/monitoring").await.json();
    assert_eq!(kept["summary"]["totals"]["plays"], 1);

    let cleared = admin_write(&app, Method::DELETE, "/api/admin/monitoring", None).await;
    assert_eq!(cleared.status, StatusCode::OK);
    let body = get(&app, "/api/admin/monitoring").await.json();
    assert_eq!(body["summary"]["totals"]["plays"], 0);
}
