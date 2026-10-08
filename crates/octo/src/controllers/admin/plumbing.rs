//! Plumbing the AdminController actions share: the browse sign-in (`BrowseUser` and its
//! cookie), the secret placeholder, and JSON answers written the way MVC wrote them. Request
//! bodies are read in [`super::json_body`].

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use octo_core::json::dom::Node;
use octo_core::settings::JsonObject;
use serde_json::Value;

use crate::app::AppState;

use crate::middleware::forwarded::RequestScheme;
use crate::services::admin::browse_session_store::BrowseSessionStore;

/// What a saved credential reads as through the admin API: every password, key, token and
/// webhook address in [`super::secrets::SECRET_FIELDS`] and [`super::secrets::SECRET_MAPS`].
/// None goes out in clear, and a save that sends this back keeps what is saved.
pub const SECRET_PLACEHOLDER: &str = "(saved, not shown)";

/// Cookie carrying the browse session. Scoped to /api/admin so it is never sent with the
/// Subsonic traffic Octo proxies.
pub const BROWSE_COOKIE_NAME: &str = "octo_browse";

/// The header a script signs in with instead of the cookie.
pub const BROWSE_TOKEN_HEADER: &str = "X-Octo-Browse-Token";

/// `MaskSecret`: the placeholder for a saved secret, `""` for none.
pub fn mask_secret(value: Option<&str>) -> String {
    match value {
        Some(v) if !v.is_empty() => SECRET_PLACEHOLDER.to_string(),
        _ => String::new(),
    }
}

// ---- the browse sign-in ----------------------------------------------------------------

/// `Request.Cookies[name]`: the first cookie of that name, unescaped as ASP.NET unescaped
/// cookie values.
pub fn request_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for value in headers.get_all(header::COOKIE) {
        let Ok(text) = value.to_str() else { continue };
        for pair in text.split([';', ',']) {
            let Some((key, raw)) = pair.split_once('=') else {
                continue;
            };
            if key.trim() != name {
                continue;
            }
            let raw = raw.trim();
            let raw = raw
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .unwrap_or(raw);
            return Some(octo_core::common::dotnet::unescape_data_string(raw));
        }
    }
    None
}

/// A request header's value as `[FromHeader]` bound a string: the first one, or None.
pub fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// A `[FromQuery] string?` parameter: the first value of the name, matched ignoring case, and
/// an empty value read as missing (`ConvertEmptyStringToNull`).
pub fn query_value(query: Option<&str>, name: &str) -> Option<String> {
    let query = query?;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
}

/// `Request.IsHttps`, after `UseForwardedHeaders` applied `X-Forwarded-Proto`.
pub fn is_https(scheme: Option<&RequestScheme>) -> bool {
    scheme.is_some_and(|s| s.0.eq_ignore_ascii_case("https"))
}

/// The cookie's terms (`BrowseCookieOptions`). Secure only over HTTPS, since this is normally
/// reached over plain HTTP on a LAN and a Secure cookie would simply be dropped there. Its age
/// is set again on every signed-in request, so the browser keeps it exactly as long as the
/// server keeps the session. Written in `SetCookieHeaderValue`'s order.
pub fn browse_cookie(token: &str, https: bool) -> HeaderValue {
    browse_cookie_for(token, https, BrowseSessionStore::TTL)
}

/// `AdminCookie.Options(request, maxAge)`: [`browse_cookie`] with its own age (an hour for a
/// recovery-code session). HttpOnly so the page's script cannot read it, SameSite=Strict so no
/// other site's page sends it, and scoped to /api/admin so it never goes out with the Subsonic
/// traffic Octo relays.
pub fn browse_cookie_for(token: &str, https: bool, max_age: chrono::TimeDelta) -> HeaderValue {
    let max_age = max_age.num_seconds();
    let secure = if https { "; secure" } else { "" };
    let text = format!(
        "{BROWSE_COOKIE_NAME}={}; max-age={max_age}; path=/api/admin{secure}; samesite=strict; httponly",
        escape_cookie_value(token)
    );
    HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static("octo_browse="))
}

/// `Response.Cookies.Delete(name, new CookieOptions { Path = "/api/admin" })`.
pub fn browse_cookie_deleted() -> HeaderValue {
    HeaderValue::from_static("octo_browse=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/api/admin")
}

/// `Uri.EscapeDataString`, which ASP.NET applied to a cookie value it wrote.
fn escape_cookie_value(value: &str) -> String {
    octo_core::common::dotnet::escape_data_string(value)
}

/// The Navidrome admin this request is signed in as (`BrowseUser`), with the cookie to send
/// back. Cookie first (how the admin UI signs in), header second so the endpoints stay usable
/// from curl. A live cookie is renewed: [`BrowseUser::finish`] adds it to the answer. A
/// recovery-code session is not one: it cannot reach the endpoints that touch library files.
pub struct BrowseUser {
    pub user: Option<String>,
    renew: Option<HeaderValue>,
}

impl BrowseUser {
    pub fn check(
        state: &AppState,
        headers: &HeaderMap,
        scheme: Option<&RequestScheme>,
        header_token: Option<&str>,
    ) -> BrowseUser {
        let cookie = request_cookie(headers, BROWSE_COOKIE_NAME);
        // A stale cookie hides a valid header: the cookie wins whenever there is one.
        let user = state
            .browse_sessions
            .navidrome_user_of(cookie.as_deref().or(header_token));
        let renew = match (&user, &cookie) {
            (Some(_), Some(cookie)) => Some(browse_cookie(cookie, is_https(scheme))),
            _ => None,
        };
        BrowseUser { user, renew }
    }

    /// `HasBrowseSession`.
    pub fn signed_in(&self) -> bool {
        self.user.is_some()
    }

    /// The answer with the renewed cookie, when there is one.
    pub fn finish(&self, mut res: Response) -> Response {
        if let Some(cookie) = &self.renew {
            res.headers_mut().append(header::SET_COOKIE, cookie.clone());
        }
        res
    }
}

// ---- JSON answers ----------------------------------------------------------------------

/// A JSON document already written as text, with the content type an MVC result gave it.
pub fn json_text(status: StatusCode, text: String, content_type: &'static str) -> Response {
    let mut res = Response::new(Body::from(text));
    *res.status_mut() = status;
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    res
}

/// `new JsonResult(node)` / `Ok(node)` over a `JsonNode` tree: compact, default escaping,
/// numbers as written.
pub fn json_node(status: StatusCode, node: &Node) -> Response {
    json_text(
        status,
        node.to_json_string(false),
        "application/json; charset=utf-8",
    )
}

/// An MVC `ObjectResult` (`Ok(obj)`, `BadRequest(obj)`, `Accepted(obj)`, `Conflict(obj)`,
/// `StatusCode(n, obj)`, ...). In .NET 9 its output formatter swaps the default encoder for
/// `UnsafeRelaxedJsonEscaping` when none is configured, so `'`, `+` and non-ASCII letters go
/// out as they are, while `new JsonResult(obj)` (written with [`crate::http::error::json_ok`])
/// escapes them. The C# baseline shows both: `browse` and `lastfm/scrobble/disconnect` are not
/// escaped, `settings`, `status` and `config-sources` are.
pub fn object_result(status: StatusCode, body: &impl serde::Serialize) -> Response {
    crate::http::error::json_relaxed_response(status, body, "application/json; charset=utf-8")
}

/// `Ok(obj)`: [`object_result`] with 200.
pub fn ok(body: &impl serde::Serialize) -> Response {
    object_result(StatusCode::OK, body)
}

/// `{"error": message}` with a status, as `BadRequest(new { error })` and its kin wrote it.
pub fn error_json(status: StatusCode, message: &str) -> Response {
    object_result(status, &serde_json::json!({ "error": message }))
}

/// STJ's camelCase naming policy (`JsonNamingPolicy.CamelCase`) for one property name.
pub fn camel_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.is_empty() || !chars[0].is_uppercase() {
        return name.to_string();
    }
    let mut out = chars.clone();
    for i in 0..chars.len() {
        if i == 1 && !chars[i].is_uppercase() {
            break;
        }
        let has_next = i + 1 < chars.len();
        // Stop when the next character is not upper case: the last capital of a run starts
        // the next word ("XMLValue" → "xmlValue").
        if i > 0 && has_next && !chars[i + 1].is_uppercase() {
            if chars[i + 1] == ' ' {
                out[i] = octo_core::common::dotnet::to_lower_char(chars[i]);
            }
            break;
        }
        out[i] = octo_core::common::dotnet::to_lower_char(chars[i]);
    }
    out.into_iter().collect()
}

/// A value serialized with its state-file (PascalCase) names, renamed as an API answer writes
/// it: every property name camelCased, except the keys of the dictionaries named in
/// `dictionaries` (by their camelCased property name), which STJ left alone.
pub fn camel_case_value(value: Value, dictionaries: &[&str]) -> Value {
    fn walk(value: Value, dictionaries: &[&str], is_dictionary: bool) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| {
                        if is_dictionary {
                            (k, walk(v, dictionaries, false))
                        } else {
                            let name = camel_case(&k);
                            let child_is_dictionary = dictionaries.contains(&name.as_str());
                            (name, walk(v, dictionaries, child_is_dictionary))
                        }
                    })
                    .collect(),
            ),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| walk(v, dictionaries, false)).collect())
            }
            other => other,
        }
    }
    walk(value, dictionaries, false)
}

// ---- JsonNode helpers -------------------------------------------------------------------

pub fn n_str(value: &str) -> Node {
    Node::String(value.to_string())
}

pub fn n_num(value: impl std::fmt::Display) -> Node {
    Node::Number(value.to_string())
}

pub fn n_obj<'a>(pairs: impl IntoIterator<Item = (&'a str, Node)>) -> Node {
    Node::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub fn n_str_array<S: AsRef<str>>(items: &[S]) -> Node {
    Node::Array(items.iter().map(|s| n_str(s.as_ref())).collect())
}

/// `KeyOf`: the key in `obj` that matches `name` ignoring case, as configuration keys do.
pub fn key_of(obj: &JsonObject, name: &str) -> Option<String> {
    obj.keys()
        .find(|key| octo_core::common::dotnet::eq_ignore_case(key, name))
        .cloned()
}

/// `Child`: the object under `name` (ignoring case), or None.
pub fn child<'a>(obj: &'a JsonObject, name: &str) -> Option<&'a JsonObject> {
    key_of(obj, name)
        .and_then(|key| obj.get(&key))
        .and_then(Node::as_object)
}

pub fn child_mut<'a>(obj: &'a mut JsonObject, name: &str) -> Option<&'a mut JsonObject> {
    let key = key_of(obj, name)?;
    obj.get_mut(&key).and_then(Node::as_object_mut)
}

/// `Accepted(new { ... })`.
pub fn accepted(body: &impl serde::Serialize) -> Response {
    object_result(StatusCode::ACCEPTED, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_case_follows_the_stj_policy() {
        for (input, expected) in [
            ("Url", "url"),
            ("URLValue", "urlValue"),
            ("ID", "id"),
            ("already", "already"),
            ("StageSeconds", "stageSeconds"),
            ("", ""),
        ] {
            assert_eq!(camel_case(input), expected, "{input}");
        }
    }

    #[test]
    fn camel_case_value_leaves_dictionary_keys_alone() {
        let value = serde_json::json!({"Fields": {"AlbumArtist": {"Value": "x"}}, "Notes": ["A"]});
        let renamed = camel_case_value(value, &["fields"]);
        assert_eq!(
            renamed,
            serde_json::json!({"fields": {"AlbumArtist": {"value": "x"}}, "notes": ["A"]})
        );
    }

    #[test]
    fn cookies_are_read_first_wins_and_unescaped() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; octo_browse=AB%2BC; octo_browse=x"),
        );
        assert_eq!(request_cookie(&headers, "octo_browse").as_deref(), Some("AB+C"));
        assert_eq!(request_cookie(&headers, "missing"), None);
    }

    #[test]
    fn the_cookie_is_written_in_set_cookie_header_value_order() {
        assert_eq!(
            browse_cookie("ABC", false).to_str().unwrap(),
            "octo_browse=ABC; max-age=7776000; path=/api/admin; samesite=strict; httponly"
        );
        assert_eq!(
            browse_cookie("ABC", true).to_str().unwrap(),
            "octo_browse=ABC; max-age=7776000; path=/api/admin; secure; samesite=strict; httponly"
        );
    }
}
