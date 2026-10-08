//! `Controllers/SoulseekSharingController.cs` (`[Route("api/admin/soulseek/sharing")]`): the
//! Sharing card on the dashboard's Soulseek page: what this server gives back to the Soulseek
//! network, read from slskd through Octo's own slskd sign-in, and the switch that turns sharing
//! on and off. Only counts, folder names and states come back; slskd's login stays inside Octo.
//! Behind the same guard as the rest of /api/admin, so the writes need X-Octo-Admin.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use indexmap::IndexMap;
use octo_core::json::dom::Node;
use serde::Deserialize;
use serde_json::json;
use tracing::error;

use super::helpers_6b2::{bind_body, error as error_answer, ok, split, status};
use crate::app::AppState;
use crate::http::routes::RouteSet;

pub fn routes() -> RouteSet {
    RouteSet::new()
        .route("/api/admin/soulseek/sharing", get(get_sharing))
        .route("/api/admin/soulseek/sharing/test-port", post(test_port))
        .route("/api/admin/soulseek/sharing/rescan", post(rescan))
        .route("/api/admin/soulseek/sharing/share", post(share))
}

/// `ShareRequest(bool On)`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ShareRequest {
    on: bool,
}

async fn get_sharing(State(state): State<AppState>) -> Response {
    ok(&state.soulseek_sharing.report(false, None).await)
}

/// Ask Soulseek's port test again now. At most every 30 seconds; sooner returns the last answer.
async fn test_port(State(state): State<AppState>) -> Response {
    ok(&state.soulseek_sharing.report(true, None).await)
}

/// Have slskd look through its shared folders again.
async fn rescan(State(state): State<AppState>) -> Response {
    match state.soulseek_sharing.rescan().await {
        None => status(StatusCode::ACCEPTED, &json!({ "ok": true })),
        Some(refused) => error_answer(StatusCode::CONFLICT, refused),
    }
}

/// The Share my library switch. Saves the choice as Soulseek.ShareLibrary, so the background
/// check keeps slskd to it, then applies it to slskd straight away and answers with the card as
/// it then stands. A choice slskd could not take is still saved, and the card says why.
async fn share(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = split(request).await;
    let request: ShareRequest = match bind_body(&parts, &body) {
        Ok(request) => request,
        Err(rejection) => return *rejection,
    };
    let mut section = IndexMap::new();
    section.insert("ShareLibrary".to_string(), Node::Bool(request.on));
    let mut patch = IndexMap::new();
    patch.insert("Soulseek".to_string(), Node::Object(section));
    if let Err(e) = state.settings_writer.merge(&patch, &[]) {
        error!("Could not save the Soulseek sharing choice: {e}");
        return error_answer(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Octo could not save the choice: {e}"),
        );
    }
    state.soulseek_share_switch.apply(request.on, true).await;
    ok(&state.soulseek_sharing.report(false, Some(request.on)).await)
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode};
    use octo_core::settings::{AppSettings, SoulseekSettings};

    use crate::app::{AppState, TestServices};
    use crate::controllers::admin::test_support_6b1::{app, get, send};

    /// slskd at a port nothing listens on, and the port test off, as the C# tests ran
    /// (`TestEnvironment.TurnOffPortChecks`).
    fn state(config_dir: &std::path::Path) -> AppState {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        let closed = listener.local_addr().expect("an address");
        drop(listener);
        AppState::for_tests_with(
            AppSettings {
                soulseek: SoulseekSettings {
                    base_url: Some(format!("http://{closed}")),
                    username: Some("octo".into()),
                    password: Some("synthetic-slskd-password".into()),
                    check_listen_port: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            TestServices {
                config_dir: Some(config_dir.to_path_buf()),
                ..Default::default()
            },
        )
    }

    // SoulseekSharingTests.Endpoints_WithSlskdOutOfReach: through the real app, with slskd at a
    // closed port: the card says slskd is out of reach and asks nobody about the port, and the
    // rescan is a guarded write.
    #[tokio::test]
    async fn endpoints_with_slskd_out_of_reach() {
        let dir = tempfile::tempdir().expect("a folder");
        let app = app(state(dir.path()));

        let card = get(&app, "/api/admin/soulseek/sharing").await;
        assert_eq!(card.status, StatusCode::OK);
        assert!(!card.text().contains("synthetic-slskd-password"));
        let report = card.json();
        assert_eq!(report["reachable"], false);
        assert_eq!(report["warnings"][0]["code"], "unreachable");
        // The dashboard reads the port's state by name.
        assert_eq!(report["port"]["state"], "Off");

        let unguarded = send(
            &app,
            Method::POST,
            "/api/admin/soulseek/sharing/rescan",
            &[],
            None,
        )
        .await;
        assert_eq!(unguarded.status, StatusCode::FORBIDDEN);

        let rescan = send(
            &app,
            Method::POST,
            "/api/admin/soulseek/sharing/rescan",
            &[("X-Octo-Admin", "1")],
            None,
        )
        .await;
        assert_eq!(rescan.status, StatusCode::CONFLICT);
        assert!(rescan.text().contains("cannot reach slskd"), "{}", rescan.text());
    }

    // SoulseekShareSwitchTests.ShareEndpoint_SavesTheChoice_AndReportsWhatSlskdDid: the switch
    // is saved to settings.json, and with slskd out of reach the answer says so instead of
    // pretending.
    #[tokio::test]
    async fn share_endpoint_saves_the_choice_and_reports_what_slskd_did() {
        let dir = tempfile::tempdir().expect("a folder");
        let app = app(state(dir.path()));
        let saved = || -> serde_json::Value {
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("settings.json")).expect("saved"))
                .expect("JSON")
        };
        let guarded = [("X-Octo-Admin", "1")];

        let on = send(
            &app,
            Method::POST,
            "/api/admin/soulseek/sharing/share",
            &guarded,
            Some(r#"{ "on": true }"#),
        )
        .await;
        assert_eq!(on.status, StatusCode::OK);
        let report = on.json();
        assert_eq!(report["switch"]["on"], true);
        assert!(
            report["switch"]["problem"]
                .as_str()
                .is_some_and(|p| p.contains("cannot reach slskd")),
            "{report}"
        );
        assert_eq!(saved()["Soulseek"]["ShareLibrary"], true);

        let off = send(
            &app,
            Method::POST,
            "/api/admin/soulseek/sharing/share",
            &guarded,
            Some(r#"{ "on": false }"#),
        )
        .await;
        assert_eq!(off.status, StatusCode::OK);
        assert_eq!(off.json()["switch"]["on"], false);
        assert_eq!(saved()["Soulseek"]["ShareLibrary"], false);

        let refused = send(
            &app,
            Method::POST,
            "/api/admin/soulseek/sharing/share",
            &[],
            Some(r#"{ "on": true }"#),
        )
        .await;
        assert_eq!(refused.status, StatusCode::FORBIDDEN);

        // A value that is not a bool is the binder's 400, and nothing is saved.
        let bad = send(
            &app,
            Method::POST,
            "/api/admin/soulseek/sharing/share",
            &guarded,
            Some(r#"{ "on": "maybe" }"#),
        )
        .await;
        assert_eq!(bad.status, StatusCode::BAD_REQUEST);
        assert_eq!(saved()["Soulseek"]["ShareLibrary"], false);
    }
}
