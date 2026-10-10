//! The request pipeline, outermost first, in `Program.cs` order (endpoints.md §2.1):
//!
//! 1. [`request_log`]: the hosting layer's "Request starting/finished" lines.
//! 2. [`forwarded`]: `UseForwardedHeaders()` (only `X-Forwarded-Proto`).
//! 3. [`admin_port_split`]: `UseAdminPortSplit(adminPort)`, before anything else answers, so
//!    the wrong port never even serves a static file.
//! 4. [`admin_headers`]: `UseAdminSecurityHeaders()`, on every /admin and /api/admin answer,
//!    the guard's and the gate's refusals included.
//! 5. [`admin_guard`]: `UseAdminRequestGuard()`, outside CORS so it strips CORS headers.
//! 6. [`admin_sign_in_gate`]: `UseAdminSignInGate()`, nothing under /api/admin answers a caller
//!    who is not signed in.
//! 7. [`cors`]: `UseCors()` with the default policy.
//! 8. [`exception`]: `UseExceptionHandler` + `GlobalExceptionHandler` for panics, inside CORS
//!    so error responses keep the CORS headers as they did in C#.
//! 9. Path canonicalisation.
//! 10. [`family_gate`]: with Family on, who is asking, members signed in to Navidrome with
//!     their hidden passwords, and the family rules. With Family off it hands every request
//!     on untouched.
//! 11. The routes (static files are routes, as `MapStaticAssets` endpoints were), falling back
//!     to the catch-all.
//!
//! The C# raw-body middleware (`HttpContext.Items["Octo.RawBody"]` for every POST, PUT and
//! PATCH) has no counterpart: an axum handler that needs the raw body extracts it as `Bytes`
//! and parses the form or JSON from those bytes itself, so the bytes are still there for the
//! faithful relay.

pub mod admin_guard;
pub mod admin_headers;
pub mod admin_port_split;
pub mod admin_sign_in_gate;
pub mod cors;
pub mod exception;
#[cfg(test)]
mod family_bypass_tests;
pub mod family_gate;
#[cfg(test)]
mod family_reading_tests;
pub mod forwarded;
pub mod request_log;
