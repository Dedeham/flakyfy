//! Port of `Services/Admin/SignInThrottle.cs`.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, TimeDelta, Utc};
use octo_core::common::Clock;
use parking_lot::Mutex;

/// Slows down guessing at the dashboard's sign-in: after ten wrong tries from one address in
/// fifteen minutes, that address waits until the oldest try is fifteen minutes old. In memory
/// only. Behind a reverse proxy every request comes from the proxy, so this becomes one shared
/// limit; that can hold up a real sign-in for fifteen minutes, never let a guess through.
pub struct SignInThrottle {
    failures: Mutex<HashMap<String, VecDeque<DateTime<Utc>>>>,
    clock: Clock,
}

impl Default for SignInThrottle {
    fn default() -> Self {
        Self::new()
    }
}

impl SignInThrottle {
    pub const MAX_FAILURES: usize = 10;
    pub const WINDOW: TimeDelta = TimeDelta::minutes(15);

    /// A flood from many addresses must not grow the table forever: past this many addresses,
    /// the ones with nothing recent are dropped.
    const MAX_ADDRESSES: usize = 10_000;

    pub fn new() -> Self {
        SignInThrottle {
            failures: Mutex::new(HashMap::new()),
            clock: Clock::system(),
        }
    }

    /// Reads the time from `clock` from now on (the C# `internal Func<DateTime> Clock`).
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// How long this address must wait, or None when it may try now.
    pub fn retry_after(&self, key: &str) -> Option<TimeDelta> {
        let mut failures = self.failures.lock();
        let tries = failures.get_mut(key)?;
        let now = self.clock.now();
        while tries.front().is_some_and(|first| now - *first >= Self::WINDOW) {
            tries.pop_front();
        }
        if tries.len() >= Self::MAX_FAILURES {
            tries.front().map(|first| *first + Self::WINDOW - now)
        } else {
            None
        }
    }

    pub fn failed(&self, key: &str) {
        let now = self.clock.now();
        let mut failures = self.failures.lock();
        let tries = failures.entry(key.to_string()).or_default();
        while tries.front().is_some_and(|first| now - *first >= Self::WINDOW) {
            tries.pop_front();
        }
        tries.push_back(now);
        if failures.len() > Self::MAX_ADDRESSES {
            failures.retain(|_, tries| tries.back().is_some_and(|last| now - *last < Self::WINDOW));
        }
    }

    pub fn succeeded(&self, key: &str) {
        self.failures.lock().remove(key);
    }

    /// A wait in words: "1 minute", "12 minutes".
    pub fn minutes(wait: TimeDelta) -> String {
        let seconds = wait.num_milliseconds() as f64 / 1000.0;
        let n = ((seconds / 60.0).ceil() as i64).max(1);
        if n == 1 {
            "1 minute".to_string()
        } else {
            format!("{n} minutes")
        }
    }

    /// `Retry-After` in whole seconds, rounded up, as the sign-in endpoints send it.
    pub fn retry_after_seconds(wait: TimeDelta) -> i64 {
        (wait.num_milliseconds() as f64 / 1000.0).ceil() as i64
    }
}

/// The key for a request: its remote address (`Connection.RemoteIpAddress`), or "unknown".
pub fn key_of(remote: Option<std::net::SocketAddr>) -> String {
    remote.map_or_else(|| "unknown".to_string(), |addr| addr.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Arc;

    #[test]
    fn ten_wrong_tries_make_the_address_wait_until_the_oldest_is_fifteen_minutes_old() {
        let now = Arc::new(Mutex::new(Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()));
        let clock = {
            let now = now.clone();
            Clock::new(move || *now.lock())
        };
        let throttle = SignInThrottle::new().with_clock(clock);
        for _ in 0..SignInThrottle::MAX_FAILURES - 1 {
            throttle.failed("10.0.0.5");
            *now.lock() += TimeDelta::minutes(1);
        }
        assert_eq!(throttle.retry_after("10.0.0.5"), None);
        throttle.failed("10.0.0.5");
        let wait = throttle.retry_after("10.0.0.5").expect("waits");
        assert_eq!(wait, TimeDelta::minutes(6));
        assert_eq!(SignInThrottle::minutes(wait), "6 minutes");
        assert_eq!(throttle.retry_after("10.0.0.6"), None, "another address may try");

        *now.lock() += TimeDelta::minutes(6);
        assert_eq!(
            throttle.retry_after("10.0.0.5"),
            None,
            "the oldest try has aged out"
        );

        throttle.failed("10.0.0.7");
        throttle.succeeded("10.0.0.7");
        assert!(throttle.failures.lock().get("10.0.0.7").is_none());
    }

    #[test]
    fn a_wait_reads_in_whole_minutes_rounded_up() {
        assert_eq!(SignInThrottle::minutes(TimeDelta::seconds(5)), "1 minute");
        assert_eq!(SignInThrottle::minutes(TimeDelta::seconds(61)), "2 minutes");
        assert_eq!(
            SignInThrottle::retry_after_seconds(TimeDelta::milliseconds(1500)),
            2
        );
    }

    #[test]
    fn the_key_is_the_remote_address() {
        assert_eq!(key_of(None), "unknown");
        assert_eq!(key_of(Some("10.1.2.3:5555".parse().unwrap())), "10.1.2.3");
        assert_eq!(key_of(Some("[::1]:5555".parse().unwrap())), "::1");
    }
}
