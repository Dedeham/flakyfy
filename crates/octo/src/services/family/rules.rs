//! Where family rules plug into the family gate.
//!
//! **Request rules** run before a request goes on, for every caller the gate identified.
//! [`request_rules`] answers None to let the request through, or the response to send instead.
//! To add one, add a match arm on the endpoint name (a `/rest` endpoint without `.view`, any
//! case, or a native path such as `/api/user`), checking `caller.managed` first when the rule
//! is only for members:
//!
//! ```ignore
//! if caller.managed && endpoint.eq_ignore_ascii_case("download") && !caller.abilities.download_files {
//!     return Some(refusal(...));
//! }
//! ```
//!
//! **Answer rules** rewrite the answer to one `/rest` endpoint. Only endpoints with a rule have
//! their answer read into memory; every other answer streams through untouched. A rule gets
//! the whole answer as plain bytes (the gate asks Navidrome for no compression on these
//! endpoints and skips a rule when an answer still arrives compressed) and returns the bytes
//! to send. Register one in [`AnswerRules::standard`] (or on a running state, as tests do):
//!
//! ```ignore
//! rules.register("getUser", Arc::new(|context: &AnswerContext<'_>, body: Bytes| {
//!     if !context.caller.managed { return body; }
//!     rewrite_roles(body, context.format)
//! }));
//! ```

use std::sync::Arc;

use axum::response::Response;
use bytes::Bytes;
use octo_subsonic::subsonic_request_parser::Parameters;
use parking_lot::RwLock;

use super::caller::FamilyCaller;
use crate::app::AppState;

/// The rules to run before a request goes on. None lets it through.
pub fn request_rules(
    _state: &AppState,
    _caller: &FamilyCaller,
    _endpoint: &str,
    _parameters: &Parameters,
) -> Option<Response> {
    None
}

/// What an answer rule knows about the request it rewrites.
pub struct AnswerContext<'a> {
    pub caller: &'a FamilyCaller,
    /// The `/rest` endpoint, without `.view`.
    pub endpoint: &'a str,
    /// The request path as the routes spell it.
    pub path: &'a str,
    pub parameters: &'a Parameters,
    /// `f`: `xml` (the default), `json` or `jsonp`.
    pub format: &'a str,
    /// The answer's content type.
    pub content_type: Option<&'a str>,
}

/// Rewrites one answer: takes the plain body, returns the body to send.
pub type AnswerRule = Arc<dyn Fn(&AnswerContext<'_>, Bytes) -> Bytes + Send + Sync>;

/// The answer rules, by `/rest` endpoint name (matched ignoring case).
#[derive(Default)]
pub struct AnswerRules {
    rules: RwLock<Vec<(String, AnswerRule)>>,
}

impl AnswerRules {
    /// The rules Octo runs.
    pub fn standard() -> Self {
        AnswerRules::default()
    }

    /// Adds a rule for `endpoint`. Rules for one endpoint run in the order they were added.
    pub fn register(&self, endpoint: &str, rule: AnswerRule) {
        self.rules.write().push((endpoint.to_string(), rule));
    }

    /// The rules for `endpoint`, in order.
    pub fn for_endpoint(&self, endpoint: &str) -> Vec<AnswerRule> {
        self.rules
            .read()
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(endpoint))
            .map(|(_, rule)| rule.clone())
            .collect()
    }

    /// Whether `endpoint` has any rule.
    pub fn has(&self, endpoint: &str) -> bool {
        self.rules
            .read()
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(endpoint))
    }
}
