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

use super::helpers_6b1::is_json_content_type;
use super::helpers_6b2::{bind_body, error as error_answer, ok, split, status};
use crate::app::AppState;
use crate::http::error::validation_problem;
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
        Err(rejection) => {
            if is_json_content_type(&parts.headers)
                && let Some((path, line, byte)) = wrong_typed_on(&body)
            {
                let message = format!(
                    "The JSON value could not be converted to Octo.Controllers.SoulseekSharingController+ShareRequest. Path: $.{path} | LineNumber: {line} | BytePositionInLine: {byte}."
                );
                return validation_problem(&[
                    ("request", &["The request field is required."]),
                    (&format!("$.{path}"), &[message.as_str()]),
                ]);
            }
            return *rejection;
        }
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

/// The `on` member of a valid JSON object body when it is not a bool, which System.Text.Json
/// refused naming the record and the member: the member's name as sent, and the line and the
/// byte in it where the reader stood (after a plain value, after the opening bracket of an
/// object or a list). None for any other body, which the shared binder answers.
fn wrong_typed_on(body: &[u8]) -> Option<(String, usize, usize)> {
    let text = std::str::from_utf8(body).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let offset = body.len() - text.len();
    serde_json::from_str::<serde_json::Value>(text)
        .ok()?
        .as_object()?;
    let s = text.as_bytes();
    let mut i = skip_ws(s, 0);
    if s.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    let mut found = None;
    loop {
        i = skip_ws(s, i);
        if s.get(i) == Some(&b'}') {
            break;
        }
        let key_end = string_end(s, i)?;
        let key: String = serde_json::from_slice(&s[i..key_end]).ok()?;
        i = skip_ws(s, key_end);
        i = skip_ws(s, i + 1); // the ':'
        let start = i;
        let end = value_end(s, start)?;
        if key.eq_ignore_ascii_case("on") {
            let stopped = if matches!(s[start], b'{' | b'[') {
                start + 1
            } else {
                end
            };
            found = Some((key, s[start] == b't' || s[start] == b'f', stopped));
        }
        i = skip_ws(s, end);
        if s.get(i) == Some(&b',') {
            i += 1;
        }
    }
    let (key, is_bool, stopped) = found?;
    if is_bool {
        return None;
    }
    let before = &body[..offset + stopped];
    let line = before.iter().filter(|b| **b == b'\n').count();
    let line_start = before.iter().rposition(|b| *b == b'\n').map_or(0, |p| p + 1);
    Some((key, line, before.len() - line_start))
}

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while matches!(s.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        i += 1;
    }
    i
}

/// The index after the string starting at `i`.
fn string_end(s: &[u8], i: usize) -> Option<usize> {
    if s.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    while let Some(&b) = s.get(j) {
        match b {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// The index after the value starting at `i` (the body is known to be valid JSON).
fn value_end(s: &[u8], i: usize) -> Option<usize> {
    match s.get(i)? {
        b'"' => string_end(s, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while let Some(&b) = s.get(j) {
                match b {
                    b'"' => {
                        j = string_end(s, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while matches!(s.get(j), Some(b) if !matches!(b, b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n'))
            {
                j += 1;
            }
            Some(j)
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode};
    use octo_core::settings::{AppSettings, SoulseekSettings};

    use super::wrong_typed_on;
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
        let errors = bad.json()["errors"].clone();
        assert_eq!(
            errors.as_object().map(|e| e.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["request".to_string(), "$.on".to_string()])
        );
        assert_eq!(
            errors["$.on"][0],
            "The JSON value could not be converted to Octo.Controllers.SoulseekSharingController+ShareRequest. Path: $.on | LineNumber: 0 | BytePositionInLine: 15."
        );
        assert_eq!(saved()["Soulseek"]["ShareLibrary"], false);
    }

    /// Where System.Text.Json stopped on a member it could not read as a bool, as the C# answer
    /// for the body {"on": "maybe"} said (LineNumber 0, BytePositionInLine 14).
    #[test]
    fn a_wrong_typed_on_is_placed_where_the_reader_stopped() {
        let cases: [(&str, Option<(&str, usize, usize)>); 8] = [
            (r#"{"on": "maybe"}"#, Some(("on", 0, 14))),
            (r#"{"x":[1,{"on":2}],"On":12}"#, Some(("On", 0, 25))),
            ("{\n  \"on\": null\n}", Some(("on", 1, 12))),
            (r#"{"on": {"a": 1}}"#, Some(("on", 0, 8))),
            (r#"{"on": true}"#, None),
            (r#"{"off": 1}"#, None),
            ("[1]", None),
            ("{not json", None),
        ];
        for (body, expected) in cases {
            assert_eq!(
                wrong_typed_on(body.as_bytes()),
                expected.map(|(k, l, b)| (k.to_string(), l, b)),
                "{body}"
            );
        }
    }
}
