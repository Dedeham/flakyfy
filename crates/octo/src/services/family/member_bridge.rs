//! Members sign in to Octo with their device secrets; Octo signs in to Navidrome for them with
//! a hidden password they never see. This reads the sign-in a Subsonic request carries,
//! checks it against the member's devices, and rewrites the request to the hidden password.
//! It also keeps the failure throttle and repairs a hidden password Navidrome stopped taking.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use chrono::{DateTime, TimeDelta, Utc};
use octo_core::common::Clock;
use octo_core::common::dotnet::eq_ignore_case;
use octo_core::family::codes::{constant_time_eq, device_secret, normalize_app_password, subsonic_token};
use octo_core::family::device::DeviceKind;
use octo_subsonic::subsonic_request_parser::{
    StringValuesCollection, escape_data_string, has_form_content_type, parse_query, read_form,
    unescape_data_string,
};
use parking_lot::Mutex;
use serde_json::Value;
use tracing::{info, warn};

use super::family_store::FamilyStore;
use super::navidrome_admin::NavidromeAdmin;
use super::vault::FamilyVault;

/// Every key a Subsonic sign-in can use.
pub const SIGN_IN_KEYS: [&str; 6] = ["u", "p", "t", "s", "apiKey", "jwt"];

fn is_sign_in_key(key: &str) -> bool {
    SIGN_IN_KEYS.iter().any(|k| eq_ignore_case(k, key))
}

fn media_type(content_type: Option<&str>) -> String {
    content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn is_url_encoded(content_type: Option<&str>) -> bool {
    media_type(content_type) == "application/x-www-form-urlencoded"
}

fn is_json(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|c| c.contains("application/json"))
}

/// The sign-in a request carries, from its query, a form body (url-encoded or multipart) and
/// a JSON body, each key matched ignoring case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignIn {
    pub users: Vec<String>,
    pub passwords: Vec<String>,
    pub tokens: Vec<String>,
    pub salts: Vec<String>,
    pub api_keys: Vec<String>,
    pub jwts: Vec<String>,
    /// Some sign-in key sits in a multipart body, which Octo cannot rewrite.
    pub in_multipart: bool,
    /// `u` came in a url-encoded body and not in the query.
    pub user_in_body_only: bool,
}

impl SignIn {
    pub fn read(query: Option<&str>, content_type: Option<&str>, body: &[u8]) -> SignIn {
        let mut sign_in = SignIn::default();
        let query = parse_query(query.unwrap_or(""));
        sign_in.take(&query);
        let user_in_query = query.get("u").is_some();
        if has_form_content_type(content_type) {
            if let Ok(form) = read_form(content_type.unwrap_or(""), body) {
                if is_url_encoded(content_type) {
                    sign_in.user_in_body_only = !user_in_query && form.get("u").is_some();
                    sign_in.take(&form);
                } else {
                    sign_in.in_multipart = form.iter().any(|(key, _)| is_sign_in_key(key));
                    sign_in.take(&form);
                }
            }
        } else if is_json(content_type)
            && let Some(pairs) = octo_subsonic::subsonic_request_parser::json_parameters(body)
        {
            let mut json = StringValuesCollection::new();
            for (key, value) in pairs {
                json.append(key, value);
            }
            sign_in.take(&json);
        }
        sign_in
    }

    fn take(&mut self, values: &StringValuesCollection) {
        let add = |key: &str, into: &mut Vec<String>| {
            if let Some(found) = values.get(key) {
                into.extend(
                    found
                        .iter()
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty()),
                );
            }
        };
        add("u", &mut self.users);
        add("p", &mut self.passwords);
        add("t", &mut self.tokens);
        add("s", &mut self.salts);
        add("apiKey", &mut self.api_keys);
        add("jwt", &mut self.jwts);
    }

    /// The names in `u`, each once (ignoring case).
    pub fn named_users(&self) -> Vec<&str> {
        distinct(&self.users)
    }
}

fn distinct(values: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for value in values {
        if !out.iter().any(|known| eq_ignore_case(known, value)) {
            out.push(value);
        }
    }
    out
}

fn distinct_exact(values: &[String]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for value in values {
        if !out.contains(&value.as_str()) {
            out.push(value);
        }
    }
    out
}

/// One of a member's devices with its secret opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSecret {
    pub id: String,
    pub kind: DeviceKind,
    pub secret: String,
}

/// What checking a member's sign-in came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// One of the member's devices signed this request.
    Verified { device_id: String },
    /// A sign-in was there and it was wrong.
    Refused,
    /// No password, token or web token at all.
    Missing,
}

/// The ways an app password may have been typed: as shown, without dashes, in lower case.
fn spellings(device: &DeviceSecret) -> Vec<String> {
    let mut out = vec![device.secret.clone()];
    if device.kind == DeviceKind::SubsonicApp {
        let plain = normalize_app_password(&device.secret);
        for spelling in [plain.clone(), plain.to_lowercase(), device.secret.to_lowercase()] {
            if !out.contains(&spelling) {
                out.push(spelling);
            }
        }
    }
    out
}

/// Checks a member's sign-in against their devices: `p` (plain or `enc:` hex) equal to a
/// device secret, or `t` equal to `md5(secret + s)` for one of them, or a web token Octo
/// signed for one of their devices (`web_device`). App passwords match however they were
/// typed. Every comparison takes the same time wherever the values differ.
pub fn verify(devices: &[DeviceSecret], sign_in: &SignIn, web_device: Option<&str>) -> Verdict {
    let passwords = distinct_exact(&sign_in.passwords);
    if !passwords.is_empty() {
        if passwords.len() > 1 {
            return Verdict::Refused;
        }
        let mut password = passwords[0].to_string();
        if let Some(encoded) = password.strip_prefix("enc:")
            && let Ok(bytes) = hex::decode(encoded)
        {
            password = String::from_utf8_lossy(&bytes).into_owned();
        }
        let mut found = None;
        for device in devices {
            let exact = constant_time_eq(password.as_bytes(), device.secret.as_bytes());
            let typed = device.kind == DeviceKind::SubsonicApp
                && constant_time_eq(
                    normalize_app_password(&password).as_bytes(),
                    normalize_app_password(&device.secret).as_bytes(),
                );
            if (exact || typed) && found.is_none() {
                found = Some(device.id.clone());
            }
        }
        return found.map_or(Verdict::Refused, |device_id| Verdict::Verified { device_id });
    }
    let tokens = distinct_exact(&sign_in.tokens);
    let salts = distinct_exact(&sign_in.salts);
    if !tokens.is_empty() || !salts.is_empty() {
        if tokens.len() != 1 || salts.len() != 1 {
            return Verdict::Refused;
        }
        let token = tokens[0].to_ascii_lowercase();
        let mut found = None;
        for device in devices {
            for spelling in spellings(device) {
                let expected = subsonic_token(&spelling, salts[0]);
                if constant_time_eq(expected.as_bytes(), token.as_bytes()) && found.is_none() {
                    found = Some(device.id.clone());
                }
            }
        }
        return found.map_or(Verdict::Refused, |device_id| Verdict::Verified { device_id });
    }
    if let Some(device_id) = web_device {
        return Verdict::Verified {
            device_id: device_id.to_string(),
        };
    }
    if !sign_in.jwts.is_empty() {
        return Verdict::Refused;
    }
    Verdict::Missing
}

/// Whether one `name=value` segment of a query or form is a sign-in key.
fn segment_is_sign_in(segment: &str) -> bool {
    let name = segment.split_once('=').map_or(segment, |(name, _)| name);
    is_sign_in_key(&unescape_data_string(&name.replace('+', " ")))
}

/// `encoded` (a query or url-encoded body) without its sign-in keys. Every other segment stays
/// exactly as written, repeated keys and their order included.
pub fn strip_sign_in(encoded: &str) -> String {
    encoded
        .split('&')
        .filter(|segment| !segment.is_empty() && !segment_is_sign_in(segment))
        .collect::<Vec<_>>()
        .join("&")
}

/// `u`, `t` and `s` for `user` with a new salt, url-encoded.
pub fn signed_as(user: &str, hidden: &str) -> String {
    let salt = hex::encode(rand::random::<[u8; 6]>());
    format!(
        "u={}&t={}&s={salt}",
        escape_data_string(user),
        subsonic_token(hidden, &salt)
    )
}

fn joined(head: String, tail: &str) -> String {
    match (head.is_empty(), tail.is_empty()) {
        (true, _) => tail.to_string(),
        (false, true) => head,
        (false, false) => format!("{head}&{tail}"),
    }
}

/// A request's query and body after the rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewritten {
    pub query: Option<String>,
    /// The new body, when it changed.
    pub body: Option<Bytes>,
}

/// Takes every sign-in key out of the query, a url-encoded body and a JSON body, and signs the
/// request in as `user` with `hidden` (a fresh salt each time). The new sign-in goes where the
/// client put `u`: the body when it sent `u` only there, else the query.
pub fn rewrite(
    query: Option<&str>,
    content_type: Option<&str>,
    body: &[u8],
    user_in_body_only: bool,
    user: &str,
    hidden: &str,
) -> Rewritten {
    let signed = signed_as(user, hidden);
    let kept_query = strip_sign_in(query.unwrap_or(""));
    let mut new_body = None;
    let mut query_out = kept_query.clone();
    if is_url_encoded(content_type) && !body.is_empty() {
        let text = String::from_utf8_lossy(body);
        let kept = strip_sign_in(&text);
        if user_in_body_only {
            new_body = Some(Bytes::from(joined(kept, &signed)));
        } else {
            if kept != text {
                new_body = Some(Bytes::from(kept));
            }
            query_out = joined(kept_query, &signed);
        }
    } else {
        if is_json(content_type)
            && let Ok(Value::Object(mut object)) = serde_json::from_slice::<Value>(body)
        {
            let before = object.len();
            object.retain(|key, _| !is_sign_in_key(key));
            if object.len() != before {
                new_body = Some(Bytes::from(Value::Object(object).to_string()));
            }
        }
        query_out = joined(kept_query, &signed);
    }
    Rewritten {
        query: (!query_out.is_empty()).then_some(query_out),
        body: new_body,
    }
}

/// Wrong sign-ins allowed per address and member before more are refused unchecked.
pub const MAX_FAILURES: usize = 20;
/// How long a wrong sign-in counts.
pub const FAILURE_WINDOW: TimeDelta = TimeDelta::minutes(15);
const MAX_KEYS: usize = 10_000;

/// Counts wrong member sign-ins per client address and member.
pub struct FailureThrottle {
    failures: Mutex<HashMap<String, VecDeque<DateTime<Utc>>>>,
    clock: Clock,
}

impl FailureThrottle {
    pub fn new(clock: Clock) -> Self {
        FailureThrottle {
            failures: Mutex::new(HashMap::new()),
            clock,
        }
    }

    /// The key for one address and member.
    pub fn key(address: &std::net::IpAddr, user: &str) -> String {
        format!("{address}|{}", user.to_lowercase())
    }

    /// Whether this key has used up its tries.
    pub fn blocked(&self, key: &str) -> bool {
        let now = self.clock.now();
        let mut failures = self.failures.lock();
        let Some(tries) = failures.get_mut(key) else {
            return false;
        };
        while tries.front().is_some_and(|first| now - *first >= FAILURE_WINDOW) {
            tries.pop_front();
        }
        tries.len() >= MAX_FAILURES
    }

    /// One more wrong sign-in for this key.
    pub fn failed(&self, key: &str) {
        let now = self.clock.now();
        let mut failures = self.failures.lock();
        if failures.len() >= MAX_KEYS && !failures.contains_key(key) {
            failures.retain(|_, tries| tries.back().is_some_and(|last| now - *last < FAILURE_WINDOW));
        }
        failures.entry(key.to_string()).or_default().push_back(now);
    }
}

/// How soon a member's hidden password may be set again after a repair.
pub const REPAIR_EVERY: Duration = Duration::from_secs(60);
const LOG_EVERY: Duration = Duration::from_secs(60);

/// The throttle, the repairs under way and the refusals logged lately.
pub struct MemberBridge {
    pub throttle: FailureThrottle,
    repairs: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<Instant>>>>>,
    logged: Mutex<HashMap<String, Instant>>,
}

impl Default for MemberBridge {
    fn default() -> Self {
        Self::new(Clock::system())
    }
}

impl MemberBridge {
    pub fn new(clock: Clock) -> Self {
        MemberBridge {
            throttle: FailureThrottle::new(clock),
            repairs: Mutex::new(HashMap::new()),
            logged: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a line about `key` may be logged now (once a minute per key).
    pub fn may_log(&self, key: &str) -> bool {
        let mut logged = self.logged.lock();
        let now = Instant::now();
        if logged
            .get(key)
            .is_some_and(|at| now.duration_since(*at) < LOG_EVERY)
        {
            return false;
        }
        if logged.len() > MAX_KEYS {
            logged.retain(|_, at| now.duration_since(*at) < LOG_EVERY);
        }
        logged.insert(key.to_string(), now);
        true
    }

    fn repair_lock(&self, user: &str) -> Arc<tokio::sync::Mutex<Option<Instant>>> {
        self.repairs
            .lock()
            .entry(user.to_lowercase())
            .or_default()
            .clone()
    }

    /// Navidrome refused a member's request signed with `used`. Answers the hidden password to
    /// try once more with: the current one when it changed meanwhile, or a new one set
    /// through Octo's admin sign-in. None when there is nothing to retry with (a repair ran
    /// for this member less than [`REPAIR_EVERY`] ago, or it failed).
    pub async fn after_refusal(
        &self,
        admin: &NavidromeAdmin,
        store: &FamilyStore,
        vault: &FamilyVault,
        user: &str,
        used: &str,
    ) -> Option<String> {
        let lock = self.repair_lock(user);
        let mut last = lock.lock().await;
        let member = store.member(user)?;
        let current = vault.open_sealed(&member.navidrome_password);
        if let Some(current) = current.as_ref()
            && current != used
        {
            return Some(current.clone());
        }
        if last.is_some_and(|at| at.elapsed() < REPAIR_EVERY) {
            return None;
        }
        *last = Some(Instant::now());
        match repair(admin, store, vault, user).await {
            Ok(hidden) => {
                info!("FAMILY Navidrome refused {user}'s hidden password, so Octo set a new one");
                Some(hidden)
            }
            Err(e) => {
                warn!(
                    "FAMILY Navidrome refused {user}'s hidden password, and Octo could not set a new one: {e}"
                );
                None
            }
        }
    }
}

/// Sets a new hidden password for a member in Navidrome and keeps it (sealed). Answers it.
pub async fn repair(
    admin: &NavidromeAdmin,
    store: &FamilyStore,
    vault: &FamilyVault,
    user: &str,
) -> Result<String, String> {
    let mut member = store
        .member(user)
        .ok_or_else(|| format!("{user} is not in the family"))?;
    if member.navidrome_user_id.trim().is_empty() {
        return Err(format!("{user} has no Navidrome account on record"));
    }
    let hidden = device_secret();
    let sealed = vault
        .seal(&hidden)
        .ok_or_else(|| "family.key is missing".to_string())?;
    admin
        .set_password(&member.navidrome_user_id, &hidden)
        .await
        .map_err(|e| e.to_string())?;
    member.navidrome_password = sealed;
    store.upsert_member(member);
    Ok(hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<DeviceSecret> {
        vec![
            DeviceSecret {
                id: "d_app".into(),
                kind: DeviceKind::OctoApp,
                secret: "Zx9-secret_value".into(),
            },
            DeviceSecret {
                id: "d_sub".into(),
                kind: DeviceKind::SubsonicApp,
                secret: "ABCD-EFGH-JKMN-PQRS".into(),
            },
        ]
    }

    fn sign_in(query: &str) -> SignIn {
        SignIn::read(Some(query), None, b"")
    }

    fn verified(id: &str) -> Verdict {
        Verdict::Verified { device_id: id.into() }
    }

    #[test]
    fn token_from_any_device_verifies() {
        let t = subsonic_token("Zx9-secret_value", "c19b2d");
        assert_eq!(
            verify(&devices(), &sign_in(&format!("u=alex&t={t}&s=c19b2d")), None),
            verified("d_app")
        );
        let t = subsonic_token("ABCD-EFGH-JKMN-PQRS", "aa");
        assert_eq!(
            verify(
                &devices(),
                &sign_in(&format!("u=alex&t={}&s=aa", t.to_uppercase())),
                None
            ),
            verified("d_sub")
        );
        assert_eq!(
            verify(&devices(), &sign_in("u=alex&t=0000&s=aa"), None),
            Verdict::Refused
        );
    }

    #[test]
    fn plain_and_enc_passwords_verify() {
        assert_eq!(
            verify(&devices(), &sign_in("u=alex&p=Zx9-secret_value"), None),
            verified("d_app")
        );
        let encoded = format!("enc:{}", hex::encode("Zx9-secret_value"));
        assert_eq!(
            verify(&devices(), &sign_in(&format!("u=alex&p={encoded}")), None),
            verified("d_app")
        );
        // Device secrets are exact: case matters for them.
        assert_eq!(
            verify(&devices(), &sign_in("u=alex&p=zx9-secret_value"), None),
            Verdict::Refused
        );
    }

    #[test]
    fn lower_case_app_password_verifies() {
        for typed in ["abcd-efgh-jkmn-pqrs", "ABCDEFGHJKMNPQRS", "abcd efgh jkmn pqrs"] {
            let query = format!("u=alex&p={}", escape_data_string(typed));
            assert_eq!(
                verify(&devices(), &sign_in(&query), None),
                verified("d_sub"),
                "{typed}"
            );
        }
        for typed in ["abcdefghjkmnpqrs", "abcd-efgh-jkmn-pqrs"] {
            let t = subsonic_token(typed, "s1");
            assert_eq!(
                verify(&devices(), &sign_in(&format!("u=alex&t={t}&s=s1")), None),
                verified("d_sub"),
                "{typed}"
            );
        }
    }

    #[test]
    fn other_members_secret_is_refused() {
        let sams = [DeviceSecret {
            id: "d_sam".into(),
            kind: DeviceKind::OctoApp,
            secret: "sams-secret".into(),
        }];
        assert_eq!(
            verify(&sams, &sign_in("u=alex&p=Zx9-secret_value"), None),
            Verdict::Refused
        );
        // Two different passwords, or a token without its salt, are refused too.
        assert_eq!(
            verify(&devices(), &sign_in("u=alex&p=Zx9-secret_value&p=other"), None),
            Verdict::Refused
        );
        assert_eq!(
            verify(&devices(), &sign_in("u=alex&t=abc"), None),
            Verdict::Refused
        );
        assert_eq!(verify(&devices(), &sign_in("u=alex"), None), Verdict::Missing);
        assert_eq!(
            verify(&devices(), &sign_in("u=alex&jwt=x"), None),
            Verdict::Refused
        );
        assert_eq!(
            verify(&devices(), &sign_in("u=alex&jwt=x"), Some("d_web")),
            verified("d_web")
        );
    }

    #[test]
    fn sign_ins_are_read_from_every_place() {
        let read = SignIn::read(
            Some("U=alex&id=1&p=a"),
            Some("application/x-www-form-urlencoded"),
            b"t=tok&s=salt&jwt=j",
        );
        assert_eq!(read.users, ["alex"]);
        assert_eq!(
            (read.passwords.len(), read.tokens.len(), read.jwts.len()),
            (1, 1, 1)
        );
        assert!(!read.user_in_body_only);
        let body_only = SignIn::read(
            None,
            Some("application/x-www-form-urlencoded; charset=utf-8"),
            b"u=alex&p=x",
        );
        assert!(body_only.user_in_body_only);
        let json = SignIn::read(None, Some("application/json"), br#"{"u":"alex","apiKey":"k"}"#);
        assert_eq!(json.api_keys, ["k"]);
        let multipart = SignIn::read(
            None,
            Some("multipart/form-data; boundary=b"),
            b"--b\r\nContent-Disposition: form-data; name=\"p\"\r\n\r\nx\r\n--b--\r\n",
        );
        assert!(multipart.in_multipart);
        let twice = sign_in("u=alex&u=ALEX&u=sam");
        assert_eq!(twice.named_users(), ["alex", "sam"]);
    }

    #[test]
    fn rewrite_keeps_everything_else_as_written() {
        let out = rewrite(
            Some("u=alex&id=A&p=secret&id=B&v=1.16.1&U=x&apikey=k&c=My%20App"),
            None,
            b"",
            false,
            "alex",
            "hidden",
        );
        let query = out.query.expect("a query");
        let (kept, signed) = query.split_at(query.find("&u=").expect("signed in"));
        assert_eq!(kept, "id=A&id=B&v=1.16.1&c=My%20App");
        let signed = parse_query(signed);
        let salt = &signed.get("s").expect("a salt")[0];
        assert_eq!(salt.len(), 12);
        assert_eq!(
            signed.get("t").expect("a token")[0],
            subsonic_token("hidden", salt)
        );
        assert_eq!(signed.get("u").expect("u")[0], "alex");
        assert!(out.body.is_none());
    }

    #[test]
    fn rewrite_follows_a_form_body() {
        let form = Some("application/x-www-form-urlencoded");
        // u only in the body: the sign-in goes back in the body.
        let out = rewrite(
            Some("f=json"),
            form,
            b"u=alex&p=secret&id=1&id=2",
            true,
            "alex",
            "h",
        );
        assert_eq!(out.query.as_deref(), Some("f=json"));
        let body = String::from_utf8(out.body.expect("a body").to_vec()).expect("text");
        assert!(body.starts_with("id=1&id=2&u=alex&t="), "{body}");
        // u in the query: the body only loses its sign-in keys.
        let out = rewrite(Some("u=alex&t=x&s=y"), form, b"id=1&p=z", false, "alex", "h");
        assert_eq!(out.body.as_deref(), Some(&b"id=1"[..]));
        assert!(out.query.expect("a query").starts_with("u=alex&t="));
        // A JSON body loses them too.
        let out = rewrite(
            None,
            Some("application/json"),
            br#"{"p":"x","id":"1"}"#,
            false,
            "alex",
            "h",
        );
        assert_eq!(out.body.as_deref(), Some(&br#"{"id":"1"}"#[..]));
    }

    #[test]
    fn the_throttle_counts_per_address_and_member() {
        let now = Arc::new(Mutex::new(Utc::now()));
        let reading = now.clone();
        let throttle = FailureThrottle::new(Clock::new(move || *reading.lock()));
        let key = FailureThrottle::key(&"203.0.113.9".parse().expect("an address"), "Alex");
        for _ in 0..MAX_FAILURES {
            assert!(!throttle.blocked(&key));
            throttle.failed(&key);
        }
        assert!(throttle.blocked(&key));
        assert!(!throttle.blocked(&FailureThrottle::key(
            &"203.0.113.9".parse().expect("an address"),
            "sam"
        )));
        *now.lock() += FAILURE_WINDOW;
        assert!(!throttle.blocked(&key));
    }
}
