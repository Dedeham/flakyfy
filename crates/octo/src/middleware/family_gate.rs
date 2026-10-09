//! The family gate: one layer every request passes while Family is on. It works out who is
//! asking, signs members in to Navidrome with their hidden passwords, and puts a
//! [`FamilyCaller`] on the request for the handlers and the family rules. With Family off it
//! hands every request on untouched before looking at anything.
//!
//! - **Subsonic (`/rest`)**: a request whose `u` names a member is checked against the
//!   member's device secrets (`p`, plain or `enc:` hex, or `t` with `s`, or a browser token
//!   Octo signed) and rewritten, in the query and in a url-encoded or JSON body, to `u` with a
//!   fresh `s` and `t` made from the hidden password. An API key or a `jwt` that turns out to be
//!   a member's is rewritten the same way. A wrong sign-in is answered with Subsonic error 40
//!   and never reaches Navidrome; twenty wrong ones from one address for one member in 15
//!   minutes and the rest are refused unchecked. The owner and anyone outside the family pass
//!   through exactly as they came.
//! - **Navidrome's web player**: a member signs in on `/auth/login` with their account password
//!   or any device secret. Octo signs in to Navidrome for them and answers with a token of its
//!   own; the real one stays on the member's web device and is swapped in on the way to
//!   Navidrome, and every fresh one Navidrome hands back is swapped for a fresh Octo token.
//! - **Radio streams** name their listener through the stream's session.
//! - **Audio**: every request that hands out audio, a radio stream included, passes the audio
//!   gate ([`audio_gate`]), which may refuse it, sets its quality and tracks its body.
//!
//! When Navidrome refuses a member's rewritten sign-in (someone changed the password there),
//! the gate sets a new hidden password once and tries again.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, OriginalUri, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::{Bytes, BytesMut};
use chrono::Utc;
use futures::StreamExt;
use http_body_util::BodyExt;
use indexmap::IndexMap;
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::family::codes::{constant_time_eq, device_secret, normalize_app_password, subsonic_token};
use octo_core::family::device::{
    Device, DeviceKind, NetworkPlace, client_address, decode_device_name, place_of,
};
use octo_core::family::endpoints::rest_endpoint_name;
use octo_subsonic::subsonic_request_parser::{Parameters, parse_query};
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::app::AppState;
use crate::http::error::json_relaxed_response;
use crate::middleware::admin_guard;
use crate::services::family::audio_gate;
use crate::services::family::device_registry::detected_device_id;
use crate::services::family::member_bridge::{self, DeviceSecret, FailureThrottle, SignIn, Verdict};
use crate::services::family::web_session::{self, WebClaims};
use crate::services::family::{AnswerContext, FamilyCaller, Member, Sighting, passwords, request_rules};
use crate::services::subsonic::IncomingRequest;

/// The largest body the gate reads (Kestrel's default request limit, as the handlers use).
const MAX_BODY: usize = 30_000_000;
/// How much of an answer is read to see whether Navidrome refused the sign-in. Refusals are a
/// few hundred bytes; anything longer is not one.
const PEEK_LIMIT: usize = 8 * 1024;
/// The largest answer an answer rule is given.
const ANSWER_LIMIT: usize = 64 * 1024 * 1024;
/// How long a browser token lasts when Navidrome's own does not say.
const DEFAULT_WEB_SECONDS: i64 = 48 * 3600;

const WRONG_SIGN_IN: &str = "Wrong username or password";
const PAUSED: &str = "This account is paused by the family owner.";
const NEEDS_REPAIR: &str = "This account needs a repair by the family owner.";
const MULTIPART_SIGN_IN: &str = "Octo cannot read a sign-in inside a multipart form. Send it in the address.";
const WEB_DEVICE_NAME: &str = "Navidrome in a browser";

pub async fn family_gate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !state.settings.current().family.enabled {
        return next.run(req).await;
    }
    let path = req.uri().path().to_string();
    if skipped(&path) {
        return next.run(req).await;
    }
    let whereabouts = Whereabouts::of(&state, &req);
    if let Some(token) = radio_token(&path) {
        return radio(&state, req, next, &token, &whereabouts).await;
    }
    if under(&path, "/rest") {
        return rest(state, req, next, whereabouts).await;
    }
    if path.trim_end_matches('/').eq_ignore_ascii_case("/auth/login") && req.method() == Method::POST {
        return web_sign_in(state, req, next, whereabouts).await;
    }
    native(state, req, next, whereabouts).await
}

/// Whether `path` is `prefix` or below it, ignoring case, at a segment boundary.
fn under(path: &str, prefix: &str) -> bool {
    path.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        && (path.len() == prefix.len() || path.as_bytes()[prefix.len()] == b'/')
}

/// Paths with their own sign-in, or none: the dashboard, the family page and its API, static
/// assets, and pairing (which a device does before it has a secret).
fn skipped(path: &str) -> bool {
    admin_guard::is_admin_path(path)
        || under(path, "/admin")
        || under(path, "/family")
        || under(path, "/api/family")
        || under(path, "/assets")
        || rest_endpoint_name(path).is_some_and(|name| name.eq_ignore_ascii_case("octoFamilyPair"))
}

/// The token of a `/radio/stream/{token}` path.
fn radio_token(path: &str) -> Option<String> {
    let prefix = "/radio/stream/";
    let head = path.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let token = path[prefix.len()..].trim_end_matches('/');
    (!token.is_empty() && !token.contains('/')).then(|| token.to_string())
}

/// Where a request comes from: the client address behind trusted proxies, and home or away.
#[derive(Debug, Clone, Copy)]
struct Whereabouts {
    address: IpAddr,
    place: NetworkPlace,
}

impl Whereabouts {
    fn of(state: &AppState, req: &Request) -> Self {
        let settings = state.settings.current();
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip())
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let forwarded: Vec<String> = req
            .headers()
            .get_all("x-forwarded-for")
            .iter()
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
            .collect();
        let forwarded = (!forwarded.is_empty()).then(|| forwarded.join(","));
        let address = client_address(peer, forwarded.as_deref(), &settings.family.trusted_proxy_list());
        Whereabouts {
            address,
            place: place_of(address, &settings.family.home_network_list()),
        }
    }
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).trim().to_string())
        .filter(|value| !value.is_empty())
}

/// What the request says about its device.
fn sighting(headers: &HeaderMap, parameters: &Parameters) -> Sighting {
    Sighting {
        client: parameters.get("c").cloned().unwrap_or_default(),
        user_agent: header_text(headers, "user-agent").unwrap_or_default(),
        octo_device_id: header_text(headers, "x-octo-device-id"),
        name: header_text(headers, "x-octo-device-name")
            .map(|name| decode_device_name(&name))
            .filter(|name| !name.is_empty()),
        navidrome_client_id: header_text(headers, "x-nd-client-unique-id"),
    }
}

/// A member, with their abilities, preference and device.
fn member_caller(
    state: &AppState,
    member: &Member,
    device_id: Option<String>,
    at: &Whereabouts,
) -> FamilyCaller {
    let mut caller = FamilyCaller::member(&member.username, member.preset, member.abilities());
    caller.device_quality = device_id
        .as_deref()
        .map(|id| state.family_store.device_quality(id))
        .unwrap_or_default();
    caller.device_id = device_id;
    caller.place = at.place;
    caller.address = at.address;
    caller.quality = state.family_store.preference(&member.username);
    caller
}

/// The owner or an account outside the family, with the device the request came from.
fn other_caller(state: &AppState, user: &str, seen: &Sighting, at: &Whereabouts) -> FamilyCaller {
    let mut caller = FamilyCaller::unmanaged(user, state.family_store.is_owner(user));
    let device_id = detected_device_id(user, seen);
    caller.device_quality = state.family_store.device_quality(&device_id);
    caller.device_id = Some(device_id);
    caller.place = at.place;
    caller.address = at.address;
    caller.quality = state.family_store.preference(user);
    caller
}

/// The caller for a username, a member or not.
fn caller_for(
    state: &AppState,
    user: &str,
    device_id: Option<String>,
    seen: &Sighting,
    at: &Whereabouts,
) -> FamilyCaller {
    match state.family_store.member(user) {
        Some(member) => member_caller(state, &member, device_id, at),
        None => other_caller(state, user, seen, at),
    }
}

/// The member's devices, each with its secret opened.
fn device_secrets(state: &AppState, user: &str) -> Vec<DeviceSecret> {
    state
        .family_store
        .devices_of(user)
        .into_iter()
        .filter_map(|device| {
            Some(DeviceSecret {
                secret: state.family_vault.open_sealed(&device.secret)?,
                id: device.id,
                kind: device.kind,
            })
        })
        .collect()
}

/// A browser token Octo signed, still good.
fn web_claims(state: &AppState, token: &str) -> Option<WebClaims> {
    let key = state.family_vault.jwt_key()?;
    web_session::verify(&key, token, Utc::now().timestamp())
}

/// How a Subsonic answer is written: `f`, and the `callback` of `jsonp`.
struct Format {
    f: String,
    callback: Option<String>,
}

impl Format {
    fn of(parameters: &Parameters) -> Self {
        Format {
            f: parameters.get("f").map_or("xml", String::as_str).to_string(),
            callback: parameters.get("callback").cloned(),
        }
    }
}

/// A Subsonic error in the format the request asked for (`jsonp` wrapped in its callback).
fn subsonic_error(state: &AppState, format: &Format, code: i32, message: &str) -> Response {
    let builder = &state.subsonic_response_builder;
    if format.f == "jsonp" {
        let reply = builder.create_error("json", code, message);
        let callback = format.callback.as_deref().filter(|c| {
            !c.is_empty()
                && c.chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '$'))
        });
        let Some(callback) = callback else {
            return reply.into_response();
        };
        let body = format!("{callback}({});", reply.text());
        let mut response = Response::new(Body::from(body));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/javascript"),
        );
        return response;
    }
    let f = if format.f == "json" { "json" } else { "xml" };
    builder.create_error(f, code, message).into_response()
}

/// A JSON refusal for Navidrome's native API and its web player's sign-in.
fn native_refusal(status: StatusCode, message: &str) -> Response {
    json_relaxed_response(status, &json!({ "error": message }), "application/json").into_response()
}

/// The request rebuilt from `parts` with a new query (kept on [`OriginalUri`] too, so a
/// handler that relays the path as sent relays the new query) and body.
fn assemble(parts: &Parts, query: Option<&str>, body: Bytes, body_changed: bool) -> Request {
    let mut parts = parts.clone();
    let with_query = |uri: &Uri| -> Option<Uri> {
        let path_and_query = match query {
            Some(query) => format!("{}?{query}", uri.path()),
            None => uri.path().to_string(),
        };
        let mut uri_parts = uri.clone().into_parts();
        uri_parts.path_and_query = Some(path_and_query.parse().ok()?);
        Uri::from_parts(uri_parts).ok()
    };
    if let Some(uri) = with_query(&parts.uri) {
        parts.uri = uri;
    }
    if let Some(OriginalUri(original)) = parts.extensions.get::<OriginalUri>().cloned()
        && let Some(uri) = with_query(&original)
    {
        parts.extensions.insert(OriginalUri(uri));
    }
    if body_changed {
        parts.headers.remove(header::TRANSFER_ENCODING);
        parts
            .headers
            .insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    }
    Request::from_parts(parts, Body::from(body))
}

/// Reads up to [`PEEK_LIMIT`] of a Subsonic answer to see whether it is error 40. Answers the
/// response, whole again, and whether it was.
async fn peek_refusal(response: Response) -> (Response, bool) {
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !(content_type.contains("xml") || content_type.contains("json") || content_type.contains("javascript"))
    {
        return (response, false);
    }
    let (parts, mut body) = response.into_parts();
    let mut seen = BytesMut::new();
    let mut ended = false;
    while seen.len() <= PEEK_LIMIT {
        match body.frame().await {
            None => {
                ended = true;
                break;
            }
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    seen.extend_from_slice(&data);
                }
            }
            Some(Err(_)) => {
                ended = true;
                break;
            }
        }
    }
    let seen = seen.freeze();
    if ended {
        let refused = is_sign_in_refusal(&seen);
        return (Response::from_parts(parts, Body::from(seen)), refused);
    }
    let first = futures::stream::once(async move { Ok::<Bytes, axum::Error>(seen) });
    let rest = first.chain(body.into_data_stream());
    (Response::from_parts(parts, Body::from_stream(rest)), false)
}

/// Whether a Subsonic answer is error 40, in any format.
fn is_sign_in_refusal(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body);
    text.contains("failed") && (text.contains("\"code\":40") || text.contains("code=\"40\""))
}

/// Runs the answer rules registered for `endpoint` over the plain answer.
async fn apply_answer_rules(
    state: &AppState,
    response: Response,
    caller: &FamilyCaller,
    endpoint: &str,
    path: &str,
    parameters: &Parameters,
) -> Response {
    let rules = state.family_answer_rules.for_endpoint(endpoint);
    if rules.is_empty() || response.headers().contains_key(header::CONTENT_ENCODING) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(mut bytes) = axum::body::to_bytes(body, ANSWER_LIMIT).await else {
        warn!("FAMILY an answer to {endpoint} was too large to check, so it was not sent");
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let format = Format::of(parameters);
    let context = AnswerContext {
        state,
        caller,
        endpoint,
        path,
        parameters,
        format: &format.f,
        content_type: content_type.as_deref(),
    };
    for rule in rules {
        bytes = rule(&context, bytes);
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::TRANSFER_ENCODING);
    Response::from_parts(parts, Body::from(bytes))
}

/// The parts of a `/rest` request the gate works from.
struct RestRequest {
    parts: Parts,
    body: Bytes,
    content_type: Option<String>,
    query: Option<String>,
    endpoint: String,
    path: String,
    format: Format,
    seen: Sighting,
    at: Whereabouts,
}

async fn rest(state: AppState, req: Request, next: Next, at: Whereabouts) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let content_type = header_text(&parts.headers, "content-type");
    let query = parts.uri.query().map(str::to_string);
    let incoming = IncomingRequest::new(
        parts.method.clone(),
        parts.headers.clone(),
        query.clone(),
        body.clone(),
    );
    let parameters = incoming.parameters();
    let path = parts.uri.path().to_string();
    let request = RestRequest {
        endpoint: rest_endpoint_name(&path).unwrap_or("").to_string(),
        format: Format::of(&parameters),
        seen: sighting(&parts.headers, &parameters),
        content_type,
        query,
        path,
        parts,
        body,
        at,
    };
    let sign_in = SignIn::read(
        request.query.as_deref(),
        request.content_type.as_deref(),
        &request.body,
    );
    let users = sign_in.named_users();

    if let Some(member) = users.iter().find_map(|user| state.family_store.member(user)) {
        return member_rest(&state, request, next, &sign_in, member, None).await;
    }
    let mut caller = None;
    if users.is_empty() {
        if let Some(api_key) = sign_in.api_keys.first() {
            let mut ask: IndexMap<String, String> = IndexMap::new();
            ask.insert("apiKey".into(), api_key.clone());
            for name in ["v", "c"] {
                if let Some(value) = parameters.get(name) {
                    ask.insert(name.into(), value.clone());
                }
            }
            if let Ok(Some(owner)) = state.request_identity.username(&ask, &state.subsonic_proxy).await {
                if let Some(member) = state.family_store.member(&owner) {
                    return member_rest(&state, request, next, &sign_in, member, Some(None)).await;
                }
                caller = Some(other_caller(&state, &owner, &request.seen, &request.at));
            }
        } else if let Some(jwt) = sign_in.jwts.first() {
            if let Some(claims) = web_claims(&state, jwt)
                && let Some(member) = state.family_store.member(&claims.sub)
                && state
                    .family_store
                    .device(&claims.did)
                    .is_some_and(|d| eq_ignore_case(&d.username, &member.username))
            {
                return member_rest(&state, request, next, &sign_in, member, Some(Some(claims.did))).await;
            }
            if let Some(user) = web_session::unverified_subject(jwt) {
                // Navidrome checks its own token; the gate only notes whose it is.
                caller = Some(caller_for(&state, &user, None, &request.seen, &request.at));
            }
        }
    } else if users.len() == 1 {
        caller = Some(other_caller(&state, users[0], &request.seen, &request.at));
    }
    pass_rest(&state, request, next, caller, &parameters).await
}

/// A `/rest` request that is not a member's: handed on as it came, with its caller.
async fn pass_rest(
    state: &AppState,
    request: RestRequest,
    next: Next,
    caller: Option<FamilyCaller>,
    parameters: &Parameters,
) -> Response {
    let RestRequest {
        mut parts,
        body,
        endpoint,
        path,
        seen,
        ..
    } = request;
    let Some(caller) = caller else {
        return next.run(Request::from_parts(parts, Body::from(body))).await;
    };
    state.family_devices.seen(&caller, &seen);
    if let Some(refused) = request_rules(state, &caller, &parts.method, &endpoint, parameters) {
        return refused;
    }
    let audio = match audio_gate::admit(
        state,
        &caller,
        &parts.method,
        &endpoint,
        parameters,
        &parts.headers,
    )
    .await
    {
        Ok(audio) => audio,
        Err(refused) => return refused,
    };
    let rules = state.family_answer_rules.has(&endpoint);
    if rules {
        parts.headers.remove(header::ACCEPT_ENCODING);
    }
    parts.extensions.insert(caller.clone());
    let req = audio.apply(Request::from_parts(parts, Body::from(body))).await;
    let response = audio.track(state, next.run(req).await);
    if rules {
        apply_answer_rules(state, response, &caller, &endpoint, &path, parameters).await
    } else {
        response
    }
}

/// A member's `/rest` request. `checked` is None when the sign-in still has to be checked, or
/// the device (if any) when an API key or a browser token already named the member.
async fn member_rest(
    state: &AppState,
    request: RestRequest,
    next: Next,
    sign_in: &SignIn,
    member: Member,
    checked: Option<Option<String>>,
) -> Response {
    let user = member.username.clone();
    let key = FailureThrottle::key(&request.at.address, &user);
    let throttle = &state.family_bridge.throttle;
    let refuse = |message: &str| subsonic_error(state, &request.format, 40, message);
    let device_id = match checked {
        Some(device_id) => device_id,
        None => {
            if sign_in.named_users().len() > 1 {
                throttle.failed(&key);
                return refuse(WRONG_SIGN_IN);
            }
            if sign_in.in_multipart {
                return refuse(MULTIPART_SIGN_IN);
            }
            if throttle.blocked(&key) {
                log_refusal(state, &user, &request.at, "too many wrong tries");
                return refuse(WRONG_SIGN_IN);
            }
            let devices = device_secrets(state, &user);
            let web_device = sign_in
                .jwts
                .iter()
                .filter_map(|jwt| web_claims(state, jwt))
                .find(|claims| {
                    eq_ignore_case(&claims.sub, &user) && devices.iter().any(|d| d.id == claims.did)
                })
                .map(|claims| claims.did);
            match member_bridge::verify(&devices, sign_in, web_device.as_deref()) {
                Verdict::Verified { device_id } => Some(device_id),
                Verdict::Missing => {
                    return subsonic_error(state, &request.format, 10, "Required parameter is missing.");
                }
                Verdict::Refused => {
                    throttle.failed(&key);
                    log_refusal(state, &user, &request.at, "a wrong secret");
                    return refuse(WRONG_SIGN_IN);
                }
            }
        }
    };
    if member.suspended {
        return refuse(PAUSED);
    }
    let Some(hidden) = state.family_vault.open_sealed(&member.navidrome_password) else {
        if state.family_bridge.may_log(&format!("open|{user}")) {
            warn!("FAMILY {user}'s hidden Navidrome password cannot be opened (is family.key missing?)");
        }
        return refuse(NEEDS_REPAIR);
    };
    let caller = member_caller(state, &member, device_id, &request.at);
    state.family_devices.seen(&caller, &request.seen);

    let rewrite_with = |hidden: &str| {
        member_bridge::rewrite(
            request.query.as_deref(),
            request.content_type.as_deref(),
            &request.body,
            sign_in.user_in_body_only,
            &user,
            hidden,
        )
    };
    let assemble_from = |rewritten: member_bridge::Rewritten| {
        let body_changed = rewritten.body.is_some();
        let body = rewritten.body.unwrap_or_else(|| request.body.clone());
        let mut req = assemble(&request.parts, rewritten.query.as_deref(), body, body_changed);
        req.extensions_mut().insert(caller.clone());
        req
    };
    let rewritten = rewrite_with(&hidden);
    let parameters = IncomingRequest::new(
        request.parts.method.clone(),
        request.parts.headers.clone(),
        rewritten.query.clone(),
        rewritten.body.clone().unwrap_or_else(|| request.body.clone()),
    )
    .parameters();
    let first = assemble_from(rewritten);
    let method = &request.parts.method;
    if let Some(refused) = request_rules(state, &caller, method, &request.endpoint, &parameters) {
        return refused;
    }
    let audio = match audio_gate::admit(
        state,
        &caller,
        method,
        &request.endpoint,
        &parameters,
        &request.parts.headers,
    )
    .await
    {
        Ok(audio) => audio,
        Err(refused) => return refused,
    };
    let rules = state.family_answer_rules.has(&request.endpoint);
    let strip = |mut req: Request| {
        if rules {
            req.headers_mut().remove(header::ACCEPT_ENCODING);
        }
        req
    };

    let first = audio.apply(strip(first)).await;
    let (mut response, refused) = peek_refusal(next.clone().run(first).await).await;
    if refused {
        match state
            .family_bridge
            .after_refusal(
                &state.navidrome_admin,
                &state.family_store,
                &state.family_vault,
                &user,
                &hidden,
            )
            .await
        {
            Some(retry) => {
                let again = audio.apply(strip(assemble_from(rewrite_with(&retry)))).await;
                let (again, still) = peek_refusal(next.run(again).await).await;
                if still && state.family_bridge.may_log(&format!("repair|{user}")) {
                    warn!("FAMILY Navidrome still refuses {user}'s sign-in after a new hidden password");
                }
                response = again;
            }
            None => {
                if state.family_bridge.may_log(&format!("refused|{user}")) {
                    warn!("FAMILY Navidrome refuses {user}'s hidden password");
                }
            }
        }
    }
    let response = audio.track(state, response);
    if rules {
        apply_answer_rules(
            state,
            response,
            &caller,
            &request.endpoint,
            &request.path,
            &parameters,
        )
        .await
    } else {
        response
    }
}

/// One log line per member a minute about refused sign-ins, naming home or away only.
fn log_refusal(state: &AppState, user: &str, at: &Whereabouts, why: &str) {
    if state.family_bridge.may_log(&format!("sign-in|{user}")) {
        let place = match at.place {
            NetworkPlace::Home => "home",
            NetworkPlace::Away => "away",
        };
        info!("FAMILY sign-in refused for {user} from {place} ({why})");
    }
}

/// A radio stream: its session names the listener.
async fn radio(state: &AppState, mut req: Request, next: Next, token: &str, at: &Whereabouts) -> Response {
    if let Some(session) = state.last_fm_radio_stream_sessions.get(token, None)
        && !session.username.trim().is_empty()
    {
        let seen = sighting(req.headers(), &Parameters::new());
        let mut caller = caller_for(state, &session.username, None, &seen, at);
        state.family_devices.seen(&caller, &seen);
        let audio =
            match audio_gate::admit_radio(state, &mut caller, token, &session.station_id, req.method()) {
                Ok(audio) => audio,
                Err(refused) => return refused,
            };
        req.extensions_mut().insert(caller);
        return audio.track(state, next.run(req).await);
    }
    next.run(req).await
}

/// The bearer tokens and `jwt` values a native request carries.
fn native_tokens(req: &Request) -> Vec<String> {
    let mut tokens = Vec::new();
    for name in ["x-nd-authorization", "authorization"] {
        for value in req.headers().get_all(name) {
            if let Some(token) = value.to_str().ok().and_then(web_session::bearer) {
                tokens.push(token.to_string());
            }
        }
    }
    if let Some(values) = parse_query(req.uri().query().unwrap_or("")).get("jwt") {
        tokens.extend(values.iter().filter(|v| !v.is_empty()).cloned());
    }
    tokens
}

/// Replaces `from` with `to` in the request's bearer headers and `jwt` query values.
fn swap_token(req: Request, from: &str, to: &str) -> Request {
    let (mut parts, body) = req.into_parts();
    for name in [
        header::HeaderName::from_static("x-nd-authorization"),
        header::AUTHORIZATION,
    ] {
        let values: Vec<HeaderValue> = parts.headers.get_all(&name).iter().cloned().collect();
        if values.is_empty() {
            continue;
        }
        parts.headers.remove(&name);
        for value in values {
            let swapped = match value.to_str().ok().and_then(web_session::bearer) {
                Some(token) if token == from => {
                    HeaderValue::from_str(&format!("Bearer {to}")).unwrap_or(value)
                }
                _ => value,
            };
            parts.headers.append(&name, swapped);
        }
    }
    let query = parts.uri.query().map(|query| {
        query
            .split('&')
            .map(|segment| match segment.split_once('=') {
                Some((name, value))
                    if eq_ignore_case(name, "jwt")
                        && octo_subsonic::subsonic_request_parser::unescape_data_string(value) == from =>
                {
                    format!(
                        "{name}={}",
                        octo_subsonic::subsonic_request_parser::escape_data_string(to)
                    )
                }
                _ => segment.to_string(),
            })
            .collect::<Vec<_>>()
            .join("&")
    });
    let original = parts.uri.query().map(str::to_string);
    if query == original {
        return Request::from_parts(parts, body);
    }
    let stub = assemble(&parts, query.as_deref(), Bytes::new(), false);
    let (parts, _) = stub.into_parts();
    Request::from_parts(parts, body)
}

/// A browser token for a member's web device, lasting as long as Navidrome's `real` one.
fn mint_for(
    state: &AppState,
    member: &Member,
    device_id: &str,
    real: &str,
    uid: Option<&str>,
) -> Option<String> {
    let key = state.family_vault.jwt_key()?;
    let now = Utc::now().timestamp();
    let claims = WebClaims {
        iss: "ND".into(),
        sub: member.username.clone(),
        uid: uid
            .filter(|u| !u.is_empty())
            .map_or_else(|| member.navidrome_user_id.clone(), str::to_string),
        adm: false,
        iat: now,
        exp: web_session::unverified_expiry(real).unwrap_or(now + DEFAULT_WEB_SECONDS),
        did: device_id.to_string(),
    };
    Some(web_session::mint(&key, &claims))
}

/// Swaps a fresh Navidrome token in a member's answer for a fresh Octo token, keeping the
/// real one on the device. No real token ever goes back to a member.
fn swap_answer_token(state: &AppState, response: &mut Response, member: &Member, device_id: &str) {
    let name = header::HeaderName::from_static("x-nd-authorization");
    let Some(value) = response.headers().get(&name).cloned() else {
        return;
    };
    response.headers_mut().remove(&name);
    let text = value.to_str().unwrap_or("").trim().to_string();
    let (prefix, real) = match web_session::bearer(&text) {
        Some(token) => ("Bearer ", token.to_string()),
        None => ("", text),
    };
    if real.is_empty() || web_claims(state, &real).is_some() {
        return;
    }
    let sealed = state.family_vault.seal(&real);
    state.family_store.set_device_jwt(device_id, sealed);
    if let Some(octo) = mint_for(state, member, device_id, &real, None)
        && let Ok(value) = HeaderValue::from_str(&format!("{prefix}{octo}"))
    {
        response.headers_mut().insert(name, value);
    }
}

/// Navidrome's native API and anything else that is not `/rest`.
async fn native(state: AppState, req: Request, next: Next, at: Whereabouts) -> Response {
    let tokens = native_tokens(&req);
    if tokens.is_empty() {
        return next.run(req).await;
    }
    let seen = sighting(req.headers(), &Parameters::new());
    let path = req.uri().path().to_string();
    let octo = tokens
        .iter()
        .find_map(|token| web_claims(&state, token).map(|claims| (token.clone(), claims)));
    let Some((octo_token, claims)) = octo else {
        // A Navidrome token: Navidrome checks it; the gate only notes whose it is.
        let Some(user) = tokens.iter().find_map(|t| web_session::unverified_subject(t)) else {
            return next.run(req).await;
        };
        let device = state.family_store.member(&user).and_then(|member| {
            state
                .family_store
                .devices_of(&member.username)
                .into_iter()
                .filter(|d| d.kind == DeviceKind::NavidromeWeb)
                .max_by_key(|d| d.last_seen.unwrap_or(d.created))
                .map(|d| d.id)
        });
        let caller = caller_for(&state, &user, device, &seen, &at);
        return pass_native(&state, req, next, caller, &seen, &path).await;
    };
    let member = state.family_store.member(&claims.sub);
    let device = state.family_store.device(&claims.did);
    let (Some(member), Some(device)) = (member, device) else {
        return native_refusal(StatusCode::UNAUTHORIZED, "Sign in again.");
    };
    if !eq_ignore_case(&device.username, &member.username) || device.kind != DeviceKind::NavidromeWeb {
        return native_refusal(StatusCode::UNAUTHORIZED, "Sign in again.");
    }
    if member.suspended {
        return native_refusal(StatusCode::UNAUTHORIZED, PAUSED);
    }
    let Some(real) = device
        .navidrome_jwt
        .as_deref()
        .and_then(|sealed| state.family_vault.open_sealed(sealed))
    else {
        return native_refusal(StatusCode::UNAUTHORIZED, "Sign in again.");
    };
    let req = swap_token(req, &octo_token, &real);
    let caller = member_caller(&state, &member, Some(device.id.clone()), &at);
    let mut response = pass_native(&state, req, next, caller, &seen, &path).await;
    swap_answer_token(&state, &mut response, &member, &device.id);
    response
}

async fn pass_native(
    state: &AppState,
    mut req: Request,
    next: Next,
    caller: FamilyCaller,
    seen: &Sighting,
    path: &str,
) -> Response {
    state.family_devices.seen(&caller, seen);
    let parameters = IncomingRequest::new(
        req.method().clone(),
        HeaderMap::new(),
        req.uri().query().map(str::to_string),
        Bytes::new(),
    )
    .parameters();
    if let Some(refused) = request_rules(state, &caller, req.method(), path, &parameters) {
        return refused;
    }
    req.extensions_mut().insert(caller);
    next.run(req).await
}

/// Whether `password` is the member's account password or one of their device secrets.
/// Answers the device whose secret it was, if it was one.
async fn web_password_ok(state: &AppState, member: &Member, password: &str) -> (bool, Option<DeviceSecret>) {
    for device in device_secrets(state, &member.username) {
        let exact = constant_time_eq(password.as_bytes(), device.secret.as_bytes());
        let typed = device.kind == DeviceKind::SubsonicApp
            && constant_time_eq(
                normalize_app_password(password).as_bytes(),
                normalize_app_password(&device.secret).as_bytes(),
            );
        if exact || typed {
            return (true, Some(device));
        }
    }
    if member.account_password.is_empty() {
        return (false, None);
    }
    let (password, hash) = (password.to_string(), member.account_password.clone());
    let ok = tokio::task::spawn_blocking(move || passwords::verify(&password, &hash))
        .await
        .unwrap_or(false);
    (ok, None)
}

/// Navidrome's own sign-in for a member, with the hidden password.
async fn navidrome_login(
    state: &AppState,
    headers: &HeaderMap,
    user: &str,
    hidden: &str,
) -> Option<(u16, Bytes)> {
    let mut sent = HeaderMap::new();
    sent.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    for name in ["user-agent", "x-nd-client-unique-id"] {
        if let Some(value) = headers.get(name) {
            sent.insert(header::HeaderName::from_static(name), value.clone());
        }
    }
    let body = json!({ "username": user, "password": hidden }).to_string();
    let incoming = IncomingRequest::new(Method::POST, sent, None, Bytes::from(body));
    let relay = state.subsonic_proxy.with_request(Arc::new(incoming));
    match relay
        .relay_raw("auth/login", std::iter::empty::<(&str, &str)>())
        .await
    {
        Ok(raw) => Some((raw.status, raw.body)),
        Err(e) => {
            warn!(
                "FAMILY could not reach Navidrome to sign {user} in: {}",
                e.type_name()
            );
            None
        }
    }
}

/// `POST /auth/login` from Navidrome's web player. Members sign in here; everyone else goes
/// on to Navidrome as before.
async fn web_sign_in(state: AppState, req: Request, next: Next, at: Whereabouts) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_BODY).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let sent: Option<Value> = serde_json::from_slice(&body).ok();
    let text = |name: &str| {
        sent.as_ref()
            .and_then(|v| v.get(name))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let (Some(username), Some(password)) = (text("username"), text("password")) else {
        return next.run(Request::from_parts(parts, Body::from(body))).await;
    };
    let Some(member) = state.family_store.member(username.trim()) else {
        return next.run(Request::from_parts(parts, Body::from(body))).await;
    };
    let user = member.username.clone();
    let key = FailureThrottle::key(&at.address, &user);
    let throttle = &state.family_bridge.throttle;
    if throttle.blocked(&key) {
        log_refusal(&state, &user, &at, "too many wrong tries");
        return native_refusal(StatusCode::UNAUTHORIZED, "Invalid username or password");
    }
    let (ok, matched) = web_password_ok(&state, &member, &password).await;
    if !ok {
        throttle.failed(&key);
        log_refusal(&state, &user, &at, "a wrong password");
        return native_refusal(StatusCode::UNAUTHORIZED, "Invalid username or password");
    }
    if member.suspended {
        return native_refusal(StatusCode::UNAUTHORIZED, PAUSED);
    }
    if state.family_vault.jwt_key().is_none() {
        return native_refusal(StatusCode::UNAUTHORIZED, NEEDS_REPAIR);
    }
    let Some(hidden) = state.family_vault.open_sealed(&member.navidrome_password) else {
        return native_refusal(StatusCode::UNAUTHORIZED, NEEDS_REPAIR);
    };
    let Some(mut answer) = navidrome_login(&state, &parts.headers, &user, &hidden).await else {
        return native_refusal(StatusCode::BAD_GATEWAY, "Navidrome is not answering.");
    };
    if answer.0 == 401
        && let Some(retry) = state
            .family_bridge
            .after_refusal(
                &state.navidrome_admin,
                &state.family_store,
                &state.family_vault,
                &user,
                &hidden,
            )
            .await
        && let Some(again) = navidrome_login(&state, &parts.headers, &user, &retry).await
    {
        answer = again;
    }
    let (status, body) = answer;
    let navidrome: Option<Value> = serde_json::from_slice(&body).ok();
    let real = navidrome
        .as_ref()
        .and_then(|v| v.get("token"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let (200, Some(Value::Object(mut fields)), Some(real)) = (status, navidrome, real) else {
        if status == 401 && state.family_bridge.may_log(&format!("web|{user}")) {
            warn!("FAMILY Navidrome refuses {user}'s hidden password for the web player");
        }
        return native_refusal(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            "Navidrome did not sign this account in.",
        );
    };
    let sealed_real = state.family_vault.seal(&real);
    let device = web_device(&state, &member, matched.as_ref(), sealed_real);
    let Some(secret) = state.family_vault.open_sealed(&device.secret) else {
        return native_refusal(StatusCode::UNAUTHORIZED, NEEDS_REPAIR);
    };
    let uid = fields.get("id").and_then(Value::as_str).map(str::to_string);
    let Some(octo) = mint_for(&state, &member, &device.id, &real, uid.as_deref()) else {
        return native_refusal(StatusCode::UNAUTHORIZED, NEEDS_REPAIR);
    };
    let salt = hex::encode(rand::random::<[u8; 6]>());
    fields.insert("token".into(), Value::String(octo));
    fields.insert(
        "subsonicToken".into(),
        Value::String(subsonic_token(&secret, &salt)),
    );
    fields.insert("subsonicSalt".into(), Value::String(salt));
    let seen = Sighting {
        client: "NavidromeUI".into(),
        ..sighting(&parts.headers, &Parameters::new())
    };
    let caller = member_caller(&state, &member, Some(device.id.clone()), &at);
    state.family_devices.seen(&caller, &seen);
    json_relaxed_response(StatusCode::OK, &Value::Object(fields), "application/json").into_response()
}

/// The member's web device to keep this sign-in on: the one whose secret they typed, else
/// the one they used last, else a new one. Its real Navidrome token becomes `sealed_real`.
fn web_device(
    state: &AppState,
    member: &Member,
    matched: Option<&DeviceSecret>,
    sealed_real: Option<String>,
) -> Device {
    let store = &state.family_store;
    let existing = matched
        .filter(|d| d.kind == DeviceKind::NavidromeWeb)
        .and_then(|d| store.device(&d.id))
        .or_else(|| {
            store
                .devices_of(&member.username)
                .into_iter()
                .filter(|d| d.kind == DeviceKind::NavidromeWeb)
                .max_by_key(|d| d.last_seen.unwrap_or(d.created))
        });
    if let Some(mut device) = existing {
        store.set_device_jwt(&device.id, sealed_real.clone());
        device.navidrome_jwt = sealed_real;
        return device;
    }
    let device = Device {
        id: format!("d_{}", &device_secret()[..16]),
        username: member.username.clone(),
        name: WEB_DEVICE_NAME.to_string(),
        kind: DeviceKind::NavidromeWeb,
        secret: state.family_vault.seal(&device_secret()).unwrap_or_default(),
        created: Utc::now(),
        last_seen: None,
        app: String::new(),
        place: NetworkPlace::Home,
        seen_apps: Vec::new(),
        navidrome_jwt: sealed_real,
        quality: Default::default(),
    };
    store.add_device(device.clone());
    info!("FAMILY web player signed in for {}", member.username);
    device
}

#[cfg(test)]
#[path = "family_gate_tests.rs"]
mod tests;
