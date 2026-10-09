//! `octoFamily` v1: Octo Family for the apps, on `/rest`, signed like any Subsonic call. The
//! family gate has already signed members in; the owner and accounts outside the family are
//! checked with Navidrome here. Every answer is a normal `subsonic-response` with
//! `"type":"octo"`, JSON or XML (XML writes scalars as attributes and lists as elements).
//! While Family is off these calls reach Navidrome as any unknown call would.
//!
//! `octoFamilyPair` is open: an app pairs with a member's username and 6-digit code before it
//! has a secret. Wrong codes count per address, against the member's live code (five burn it),
//! and for everyone together.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::family::abilities::RequestQuality;
use octo_core::family::device::{DeviceKind, NetworkPlace, client_origin};
use octo_core::family::request::RequestKind;
use octo_subsonic::subsonic_response_builder::{
    SUBSONIC_NAMESPACE, SUBSONIC_VERSION, SubsonicReply, json_shape_to_xml,
};
use octo_subsonic::xml::XElement;
use serde_json::{Value, json};

use super::caller_checks::{SubsonicCall, is_successful_subsonic_response};
use crate::app::AppState;
use crate::controllers::family::account::{device_kind, quality_choice, quality_mode, set_preference};
use crate::controllers::family::manage::request_state;
use crate::http::routes::RouteSet;
use crate::middleware::forwarded::RequestScheme;
use crate::services::family::family_service::{Actor, FamilyError, NewRequest, quality_json};
use crate::services::family::{FamilyCaller, FamilyService, family_caller};

pub fn routes() -> RouteSet {
    RouteSet::new()
        .subsonic("getFamily", get(get_family).post(get_family))
        .subsonic(
            "setFamilyQuality",
            get(set_family_quality).post(set_family_quality),
        )
        .subsonic(
            "setFamilyDeviceQuality",
            get(set_family_device_quality).post(set_family_device_quality),
        )
        .subsonic(
            "getFamilyRequests",
            get(get_family_requests).post(get_family_requests),
        )
        .subsonic("requestCopy", get(request_copy).post(request_copy))
        .subsonic(
            "cancelFamilyRequest",
            get(cancel_family_request).post(cancel_family_request),
        )
        .subsonic(
            "decideFamilyRequest",
            get(decide_family_request).post(decide_family_request),
        )
        .subsonic(
            "getFamilyDevices",
            get(get_family_devices).post(get_family_devices),
        )
        .subsonic(
            "signOutFamilyDevice",
            get(sign_out_family_device).post(sign_out_family_device),
        )
        .subsonic("addFamilyDevice", get(add_family_device).post(add_family_device))
        .subsonic(
            "removeFromMyLibrary",
            get(remove_from_my_library).post(remove_from_my_library),
        )
        .subsonic(
            "createFamilyPlaylist",
            get(create_family_playlist).post(create_family_playlist),
        )
        .subsonic("octoFamilyPair", get(octo_family_pair).post(octo_family_pair))
}

/// The extension's name and versions, as `getOpenSubsonicExtensions` lists it.
pub const FAMILY_EXTENSION: (&str, i32) = ("octoFamily", 1);

/// Adds `octoFamily` to a `getOpenSubsonicExtensions` answer (an ok one; a failed or unreadable
/// answer goes out as it came).
pub fn with_family_extension(reply: SubsonicReply, format: &str) -> SubsonicReply {
    let (name, version) = FAMILY_EXTENSION;
    if format.eq_ignore_ascii_case("json") {
        let Some(mut value) = reply.json_value() else {
            return reply;
        };
        let Some(envelope) = value.get_mut("subsonic-response").and_then(Value::as_object_mut) else {
            return reply;
        };
        if envelope.get("status").and_then(Value::as_str) != Some("ok") {
            return reply;
        }
        let list = envelope
            .entry("openSubsonicExtensions")
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(list) = list.as_array_mut() else {
            return reply;
        };
        if !list
            .iter()
            .any(|item| item.get("name").and_then(Value::as_str) == Some(name))
        {
            list.push(json!({ "name": name, "versions": [version] }));
        }
        return SubsonicReply::json(&value);
    }
    let Ok(mut response) = XElement::parse(&reply.text()) else {
        return reply;
    };
    if response.attribute("status") != Some("ok") {
        return reply;
    }
    let ns = response.namespace.clone();
    let listed = response
        .elements_named(ns.as_deref(), "openSubsonicExtensions")
        .any(|e| e.attribute("name") == Some(name));
    if !listed {
        response.push(
            XElement::in_namespace(ns.as_deref(), "openSubsonicExtensions")
                .attr("name", name)
                .child(XElement::in_namespace(ns.as_deref(), "versions").text(version.to_string())),
        );
    }
    SubsonicReply::content(response.to_xml_string(), &reply.content_type)
}

/// `json` for `json` and `jsonp`, else `xml`.
fn format_of(call: &SubsonicCall) -> String {
    match call.format().to_ascii_lowercase().as_str() {
        "json" | "jsonp" => "json".to_string(),
        _ => "xml".to_string(),
    }
}

/// An ok answer of Octo's own carrying `key` (or nothing).
pub fn octo_reply(state: &AppState, format: &str, key: Option<(&str, &Value)>) -> Response {
    if format == "json" {
        let mut body = json!({
            "status": "ok",
            "version": SUBSONIC_VERSION,
            "type": "octo",
            "openSubsonic": true,
        });
        if let (Some((key, value)), Value::Object(map)) = (key, &mut body) {
            map.insert(key.to_string(), value.clone());
        }
        return state
            .subsonic_response_builder
            .create_json_response(body)
            .into_response();
    }
    let mut envelope = XElement::ns(SUBSONIC_NAMESPACE, "subsonic-response")
        .attr("status", "ok")
        .attr("version", SUBSONIC_VERSION)
        .attr("type", "octo")
        .attr("openSubsonic", "true");
    if let Some((key, value)) = key {
        envelope.push(json_shape_to_xml(Some(SUBSONIC_NAMESPACE), key, value));
    }
    SubsonicReply::xml(&envelope).into_response()
}

fn error(state: &AppState, format: &str, code: i32, message: &str) -> Response {
    state
        .subsonic_response_builder
        .create_error(format, code, message)
        .into_response()
}

fn family_failure(state: &AppState, format: &str, e: &FamilyError) -> Response {
    error(state, format, e.subsonic_code(), e.message())
}

/// One `octoFamily` call: its parameters, its caller and the format to answer in.
struct FamilyCall {
    call: SubsonicCall,
    caller: FamilyCaller,
    format: String,
    origin: String,
}

impl FamilyCall {
    fn actor(&self) -> Actor {
        Actor {
            user: self.caller.user.clone(),
            role: self.caller.role,
        }
    }

    fn param(&self, key: &str) -> &str {
        self.call.param(key).trim()
    }

    fn flag(&self, key: &str) -> bool {
        eq_ignore_case(self.param(key), "true") || self.param(key) == "1"
    }
}

/// The address the client reached Octo by: `scheme://host`.
fn origin_of(req: &Request) -> String {
    let https = req
        .extensions()
        .get::<RequestScheme>()
        .is_some_and(|s| s.0.eq_ignore_ascii_case("https"));
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or("localhost")
        .to_string();
    format!("{}://{host}", if https { "https" } else { "http" })
}

/// Starts a call: hands it to the catch-all while Family is off, and checks who is asking.
#[allow(clippy::result_large_err)] // the error is the answer itself, sent at once
async fn begin(state: &AppState, req: Request) -> Result<FamilyCall, Response> {
    if !state.settings.current().family.enabled {
        return Err(crate::http::catch_all::catch_all(State(state.clone()), req).await);
    }
    let caller = family_caller(req.extensions()).cloned();
    let origin = origin_of(&req);
    let call = SubsonicCall::read(state, req).await?;
    let format = format_of(&call);
    let Some(mut caller) = caller else {
        return Err(if call.param_opt("u").is_none_or(|u| u.trim().is_empty()) {
            error(state, &format, 10, "Required parameter is missing: u")
        } else {
            error(state, &format, 40, "Wrong username or password")
        });
    };
    if caller.managed && !caller.checked {
        // A member's sign-in is always checked by the gate; one that was not is no sign-in.
        return Err(error(state, &format, 40, "Wrong username or password"));
    }
    if !caller.managed {
        // The gate passes the owner and accounts outside the family on as they came, so
        // Navidrome says whether the sign-in is right.
        let mut auth = call.parameters.clone();
        auth.insert("f".into(), "json".into());
        let Some(check) = call.proxy.relay_safe("rest/ping", &auth).await else {
            return Err(error(
                state,
                &format,
                0,
                "Octo can't reach Navidrome to check who is asking",
            ));
        };
        if !is_successful_subsonic_response(&check.body, "json") {
            return Err(error(state, &format, 40, "Wrong username or password"));
        }
        if !caller.checked {
            // The sign-in checks out now, so the device it came from is noted before the call
            // reads the device list.
            caller.checked = true;
            let seen = crate::middleware::family_gate::sighting(call.headers(), &call.parameters);
            state.family_devices.seen(&caller, &seen);
        }
    }
    Ok(FamilyCall {
        call,
        caller,
        format,
        origin,
    })
}

macro_rules! started {
    ($state:expr, $req:expr) => {
        match begin(&$state, $req).await {
            Ok(call) => call,
            Err(res) => return res,
        }
    };
}

/// Whether the caller manages the family from where they are now.
fn manages_here(state: &AppState, call: &FamilyCall) -> bool {
    FamilyService::may_manage(state, &call.actor()) && may_act_from_here(state, call)
}

fn approves_here(state: &AppState, call: &FamilyCall) -> bool {
    FamilyService::may_approve(state, &call.actor()) && may_act_from_here(state, call)
}

fn may_act_from_here(state: &AppState, call: &FamilyCall) -> bool {
    call.caller.place == NetworkPlace::Home || state.settings.current().family.manage_from_away
}

/// `getFamily`.
pub async fn get_family(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let me = state.family.me_json(
        &state,
        &call.caller.user,
        call.caller.device_id.as_deref(),
        call.caller.place,
    );
    let mut family = json!({ "me": me });
    if manages_here(&state, &call)
        && let Some(manager) = state.family.manager_json(&state, &call.actor())
        && let Value::Object(map) = &mut family
    {
        map.insert("manager".into(), manager);
    }
    octo_reply(&state, &call.format, Some(("family", &family)))
}

/// `setFamilyQuality`: the account's home and away quality (either may be left out).
pub async fn set_family_quality(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let (home, away) = match (
        quality_choice(call.call.param_opt("home")),
        quality_choice(call.call.param_opt("away")),
    ) {
        (Ok(home), Ok(away)) => (home, away),
        (Err(words), _) | (_, Err(words)) => return error(&state, &call.format, 10, words),
    };
    let preference = set_preference(&state, &call.caller.user, home, away);
    let quality = quality_json(
        &preference,
        &FamilyService::abilities_of(&state, &call.caller.user),
    );
    octo_reply(&state, &call.format, Some(("quality", &quality)))
}

/// `setFamilyDeviceQuality`: `id` (own device), `mode` (`Account` or `App`).
pub async fn set_family_device_quality(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let id = call.param("id");
    if id.is_empty() {
        return error(&state, &call.format, 10, "Required parameter is missing: id");
    }
    let Some(mode) = quality_mode(call.param("mode")) else {
        return error(&state, &call.format, 10, "Choose mode Account or App.");
    };
    match state
        .family
        .set_device(&state, &call.caller.user, id, Some(mode), None)
    {
        Ok(device) => {
            let device = state
                .family
                .device_json(&state, &device, call.caller.device_id.as_deref());
            octo_reply(&state, &call.format, Some(("device", &device)))
        }
        Err(e) => family_failure(&state, &call.format, &e),
    }
}

/// `getFamilyRequests`: the caller's own, or everyone's (`all`, approvers).
pub async fn get_family_requests(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let all = call.flag("all");
    if all && !approves_here(&state, &call) {
        return error(
            &state,
            &call.format,
            50,
            "Only people who approve requests can see everyone's.",
        );
    }
    let filter = match call.param("state") {
        "" => None,
        text => match request_state(text) {
            Some(found) => Some(found),
            None => return error(&state, &call.format, 10, "Unknown request state."),
        },
    };
    let user = (!all).then(|| call.caller.user.clone());
    let list: Vec<Value> = FamilyService::requests(&state, user.as_deref(), filter)
        .iter()
        .map(|r| FamilyService::request_json(&state, r))
        .collect();
    octo_reply(
        &state,
        &call.format,
        Some(("familyRequests", &json!({ "request": list }))),
    )
}

/// Reads a request kind by name, any case.
fn request_kind(text: &str) -> Option<RequestKind> {
    serde_json::from_value(Value::String(
        ["Song", "Album", "Upgrade", "Import", "PlaylistAdd"]
            .into_iter()
            .find(|k| k.eq_ignore_ascii_case(text.trim()))?
            .to_string(),
    ))
    .ok()
}

/// `requestCopy`: `id`, `quality` (default Best), `kind` (optional).
pub async fn request_copy(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    if !call.caller.managed {
        return error(&state, &call.format, 50, "Requests are for family members.");
    }
    let id = call.param("id");
    if id.is_empty() {
        return error(&state, &call.format, 10, "Required parameter is missing: id");
    }
    let quality = match call.param("quality") {
        "" => RequestQuality::Best,
        text => match RequestQuality::parse(text) {
            Some(quality) => quality,
            None => return error(&state, &call.format, 10, "Choose quality Best, Flac or Mp3."),
        },
    };
    let kind = match call.param("kind") {
        "" => None,
        text => match request_kind(text) {
            Some(kind) => Some(kind),
            None => return error(&state, &call.format, 10, "Unknown request kind."),
        },
    };
    let hooks = state.family.hooks(&state);
    match hooks
        .requests
        .create(NewRequest {
            username: call.caller.user.clone(),
            target: id.to_string(),
            quality,
            kind,
        })
        .await
    {
        Ok(made) => {
            let value = FamilyService::request_json(&state, &made);
            octo_reply(&state, &call.format, Some(("familyRequest", &value)))
        }
        Err(e) => family_failure(&state, &call.format, &e),
    }
}

/// `cancelFamilyRequest`: the requester's own pending request.
pub async fn cancel_family_request(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let id = call.param("id");
    if id.is_empty() {
        return error(&state, &call.format, 10, "Required parameter is missing: id");
    }
    match state
        .family
        .hooks(&state)
        .requests
        .cancel(id, &call.caller.user)
        .await
    {
        Ok(request) => {
            let value = FamilyService::request_json(&state, &request);
            octo_reply(&state, &call.format, Some(("familyRequest", &value)))
        }
        Err(e) => family_failure(&state, &call.format, &e),
    }
}

/// `decideFamilyRequest`: `id`, `approve`, `note`; needs Approve requests.
pub async fn decide_family_request(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    if !approves_here(&state, &call) {
        return error(
            &state,
            &call.format,
            50,
            "Only people who approve requests can do that.",
        );
    }
    let id = call.param("id");
    if id.is_empty() || call.param("approve").is_empty() {
        return error(
            &state,
            &call.format,
            10,
            "Required parameter is missing: id or approve",
        );
    }
    let approve = call.flag("approve");
    match state
        .family
        .hooks(&state)
        .requests
        .decide(id, approve, &call.caller.user, call.param("note"))
        .await
    {
        Ok(request) => {
            let value = FamilyService::request_json(&state, &request);
            octo_reply(&state, &call.format, Some(("familyRequest", &value)))
        }
        Err(e) => family_failure(&state, &call.format, &e),
    }
}

/// `getFamilyDevices`: the caller's own, or everyone's (`all`, managers).
pub async fn get_family_devices(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let all = call.flag("all");
    if all && !manages_here(&state, &call) {
        return error(
            &state,
            &call.format,
            50,
            "Only people who manage the family can see everyone's devices.",
        );
    }
    let devices = if all {
        FamilyService::all_devices(&state)
    } else {
        FamilyService::devices_of(&state, &call.caller.user)
    };
    let list: Vec<Value> = devices
        .iter()
        .map(|d| {
            state
                .family
                .device_json(&state, d, call.caller.device_id.as_deref())
        })
        .collect();
    octo_reply(
        &state,
        &call.format,
        Some(("familyDevices", &json!({ "device": list }))),
    )
}

/// `signOutFamilyDevice`: own devices, or any member's for managers.
pub async fn sign_out_family_device(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let id = call.param("id");
    if id.is_empty() {
        return error(&state, &call.format, 10, "Required parameter is missing: id");
    }
    let manager = manages_here(&state, &call);
    match state.family.sign_out_device(&state, &call.actor(), id, manager) {
        Ok(()) => octo_reply(&state, &call.format, None),
        Err(e) => family_failure(&state, &call.format, &e),
    }
}

/// `addFamilyDevice`: `name`, `kind` (`OctoApp` gives a pairing code, `SubsonicApp` an app
/// password), and `username` for a manager adding a device for any member.
pub async fn add_family_device(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let Some(kind) = device_kind(call.param("kind")) else {
        return error(&state, &call.format, 10, "Choose kind OctoApp or SubsonicApp.");
    };
    let user = match call.param("username") {
        "" => call.caller.user.clone(),
        other if eq_ignore_case(other, &call.caller.user) => call.caller.user.clone(),
        other => {
            if !manages_here(&state, &call) {
                return error(
                    &state,
                    &call.format,
                    50,
                    "Only people who manage the family can add a device for someone else.",
                );
            }
            match state.family_store.member(other) {
                Some(member) => member.username,
                None => {
                    return error(
                        &state,
                        &call.format,
                        70,
                        &format!("{other} is not in the family."),
                    );
                }
            }
        }
    };
    let base = FamilyService::public_base(&state, &call.origin);
    match state
        .family
        .add_device(&state, &user, call.param("name"), kind, &base)
    {
        Ok(added) => octo_reply(&state, &call.format, Some(("familyDeviceAdded", &json!(added)))),
        Err(e) => family_failure(&state, &call.format, &e),
    }
}

/// `removeFromMyLibrary`: `id`, a song in the caller's own library.
pub async fn remove_from_my_library(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    if !call.caller.managed {
        return error(
            &state,
            &call.format,
            50,
            "Only family members have a library of their own.",
        );
    }
    let id = call.param("id");
    if id.is_empty() {
        return error(&state, &call.format, 10, "Required parameter is missing: id");
    }
    match state
        .family
        .hooks(&state)
        .my_library
        .remove(&call.caller.user, id)
        .await
    {
        Ok(()) => octo_reply(&state, &call.format, None),
        Err(e) => family_failure(&state, &call.format, &e),
    }
}

/// `createFamilyPlaylist`: `name`, `approveAdditions` (managers); answers the playlist node.
pub async fn create_family_playlist(State(state): State<AppState>, req: Request) -> Response {
    let call = started!(state, req);
    let name = call.param("name");
    if name.is_empty() {
        return error(&state, &call.format, 10, "Required parameter is missing: name");
    }
    if !FamilyService::abilities_of(&state, &call.caller.user).family_playlists_edit {
        return error(
            &state,
            &call.format,
            50,
            "Family playlists cannot be changed from this account.",
        );
    }
    let approve = call.flag("approveAdditions");
    if approve && !manages_here(&state, &call) {
        return error(
            &state,
            &call.format,
            50,
            "Only people who manage the family can ask for approval of additions.",
        );
    }
    match state
        .family
        .hooks(&state)
        .playlists
        .create(name, &call.caller.user, approve)
        .await
    {
        Ok(playlist) => {
            let node = json!({
                "id": playlist.id,
                "name": playlist.name,
                "comment": "",
                "owner": "Family",
                "public": true,
                "songCount": playlist.song_count,
                "duration": 0,
                "created": playlist.created,
                "changed": playlist.created,
                "entry": [],
            });
            octo_reply(&state, &call.format, Some(("playlist", &node)))
        }
        Err(e) => family_failure(&state, &call.format, &e),
    }
}

/// The client address of a request the gate did not look at.
fn address_of(state: &AppState, req: &Request) -> IpAddr {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let forwarded: Vec<String> = req
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .collect();
    let forwarded = (!forwarded.is_empty()).then(|| forwarded.join(","));
    let settings = state.settings.current();
    client_origin(
        peer,
        forwarded.as_deref(),
        &settings.family.trusted_proxy_list(),
        &settings.family.home_network_list(),
    )
    .address
}

/// `octoFamilyPair` (open, GET or form POST): `username`, `code`, `deviceName`, `platform`.
/// `kind=SubsonicApp` gives an app password for another app instead of an Octo app's secret.
pub async fn octo_family_pair(State(state): State<AppState>, req: Request) -> Response {
    if !state.settings.current().family.enabled {
        return crate::http::catch_all::catch_all(State(state.clone()), req).await;
    }
    let address = address_of(&state, &req);
    let origin = origin_of(&req);
    let call = match SubsonicCall::read(&state, req).await {
        Ok(call) => call,
        Err(res) => return res,
    };
    let format = format_of(&call);
    let (username, code) = (call.param("username").trim(), call.param("code").trim());
    if username.is_empty() || code.is_empty() {
        return error(
            &state,
            &format,
            10,
            "Required parameter is missing: username or code",
        );
    }
    let kind = match call.param("kind").trim() {
        "" => DeviceKind::OctoApp,
        text => device_kind(text).unwrap_or(DeviceKind::OctoApp),
    };
    let device_name = call.param("deviceName").to_string();
    let base = FamilyService::public_base(&state, &origin);
    match state.family.pair(
        &state,
        &address.to_string(),
        username,
        code,
        &device_name,
        call.param("platform"),
        kind,
        &base,
    ) {
        Ok(paired) => octo_reply(&state, &format, Some(("familyPair", &json!(paired)))),
        Err(FamilyError::Refused(message)) => error(&state, &format, 40, &message),
        Err(e) => family_failure(&state, &format, &e),
    }
}
