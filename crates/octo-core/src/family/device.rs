//! Devices: what a member signs in with, the app name Octo shows for it, whether it is home or
//! away, and the client address behind trusted proxies (CHECK-DELTA.md M5).

use std::net::IpAddr;

use chrono::{DateTime, Utc};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};

use crate::json::datetime;

/// How a device signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceKind {
    /// An Octo app paired with a 6-digit code.
    OctoApp,
    /// Any Subsonic app, with a shown-once app password.
    SubsonicApp,
    /// Navidrome's web player, signed in through Octo.
    NavidromeWeb,
    /// Seen but not signed in by Octo (the owner and unmanaged accounts).
    Detected,
}

/// Home or away, from the client address and `Family:HomeNetworks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum NetworkPlace {
    #[default]
    Home,
    Away,
}

/// One device as `family.json` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Device {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub name: String,
    pub kind: DeviceKind,
    /// The device's secret, sealed (`sealed:...`); empty for detected devices.
    #[serde(default)]
    pub secret: String,
    #[serde(with = "datetime::utc")]
    pub created: DateTime<Utc>,
    #[serde(default, with = "datetime::utc_option")]
    pub last_seen: Option<DateTime<Utc>>,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub place: NetworkPlace,
    #[serde(default)]
    pub seen_apps: Vec<String>,
    /// A `NavidromeWeb` device's real Navidrome JWT, sealed (CHECK-DELTA.md C1 item 4). Never
    /// handed to the member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navidrome_jwt: Option<String>,
}

/// Subsonic clients Octo names by their `c` parameter, matched ignoring case at the start.
const KNOWN_CLIENTS: [(&str, &str); 11] = [
    ("symfonium", "Symfonium"),
    ("feishin", "Feishin"),
    ("amperfy", "Amperfy"),
    ("substreamer", "substreamer"),
    ("tempo", "Tempo"),
    ("dsub", "DSub"),
    ("ultrasonic", "Ultrasonic"),
    ("play:sub", "play:Sub"),
    ("sonixd", "Sonixd"),
    ("supersonic", "Supersonic"),
    ("navidromeui", "Navidrome"),
];

/// The app name the Family page shows for a device, from the Subsonic `c` parameter and the
/// User-Agent. `Octo` with `Octo/1.6 (Android)` is `Octo 1.6 (Android)`; a known client is
/// its name plus its version from the User-Agent when there is one; anything else is `c` as
/// given.
pub fn app_label(client: &str, user_agent: &str) -> String {
    let client = client.trim();
    let user_agent = user_agent.trim();
    if client.eq_ignore_ascii_case("octo") {
        if let Some(rest) = strip_prefix_ignore_case(user_agent, "octo/") {
            let (version, platform) = match rest.split_once(' ') {
                Some((version, tail)) => (version, tail.trim()),
                None => (rest, ""),
            };
            let platform = platform.trim_start_matches('(').trim_end_matches(')').trim();
            return if platform.is_empty() {
                format!("Octo {version}")
            } else {
                format!("Octo {version} ({platform})")
            };
        }
        return "Octo".to_string();
    }
    let lower = client.to_ascii_lowercase();
    if let Some((key, name)) = KNOWN_CLIENTS.iter().find(|(key, _)| lower.starts_with(key)) {
        return match version_in(user_agent, key) {
            Some(version) => format!("{name} {version}"),
            None => (*name).to_string(),
        };
    }
    if client.is_empty() {
        "Unknown app".to_string()
    } else {
        client.to_string()
    }
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

/// The version after `<name>/` anywhere in the User-Agent (`Symfonium/12.3.0 (...)`).
fn version_in(user_agent: &str, name: &str) -> Option<String> {
    user_agent.split_whitespace().find_map(|token| {
        let rest = strip_prefix_ignore_case(token, name)?.strip_prefix('/')?;
        let version: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
            .collect();
        (!version.is_empty()).then_some(version)
    })
}

/// Splits a list of networks written by hand: commas and whitespace both separate, a bare
/// address is a single host (/32 or /128). Returns the networks and the pieces that were not
/// networks.
pub fn parse_networks(text: &str) -> (Vec<IpNet>, Vec<String>) {
    let mut networks = Vec::new();
    let mut rejected = Vec::new();
    for piece in text.split(|c: char| c == ',' || c.is_whitespace()) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        if let Ok(net) = piece.parse::<IpNet>() {
            networks.push(net);
        } else if let Ok(ip) = piece.parse::<IpAddr>() {
            networks.push(IpNet::from(ip));
        } else {
            rejected.push(piece.to_string());
        }
    }
    (networks, rejected)
}

/// An IPv4 address carried in IPv6 (`::ffff:192.168.1.5`) as the IPv4 address it is.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

fn contains(networks: &[IpNet], ip: IpAddr) -> bool {
    let ip = canonical(ip);
    networks.iter().any(|net| net.contains(&ip))
}

/// Home when the client address is in one of the home networks, else away.
pub fn place_of(ip: IpAddr, home: &[IpNet]) -> NetworkPlace {
    if contains(home, ip) {
        NetworkPlace::Home
    } else {
        NetworkPlace::Away
    }
}

/// The address of the person making the request (CHECK-DELTA.md M5, Navidrome 0.64.0's rule).
///
/// Headers are believed only when the TCP peer is a trusted proxy (the tunnel or a reverse
/// proxy running beside Octo). Then `Cf-Connecting-Ip` wins; else the rightmost
/// `X-Forwarded-For` hop that is not itself a trusted proxy (hops to its left are written by
/// the client and could say anything); else the peer. A peer that is not trusted is the
/// client, whatever its headers say.
pub fn client_address(
    peer: IpAddr,
    cf_connecting_ip: Option<&str>,
    x_forwarded_for: Option<&str>,
    trusted: &[IpNet],
) -> IpAddr {
    let peer = canonical(peer);
    if !contains(trusted, peer) {
        return peer;
    }
    if let Some(ip) = cf_connecting_ip.and_then(|v| v.trim().parse::<IpAddr>().ok()) {
        return canonical(ip);
    }
    if let Some(list) = x_forwarded_for {
        for hop in list.rsplit(',') {
            let Ok(ip) = hop.trim().parse::<IpAddr>() else {
                // A hop that is not an address cannot be trusted further left either.
                return peer;
            };
            let ip = canonical(ip);
            if !contains(trusted, ip) {
                return ip;
            }
        }
    }
    peer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets(text: &str) -> Vec<IpNet> {
        let (nets, rejected) = parse_networks(text);
        assert!(rejected.is_empty(), "{rejected:?}");
        nets
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("an address")
    }

    const HOME: &str = "127.0.0.0/8, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, ::1/128, fc00::/7";
    const TRUSTED: &str = "127.0.0.0/8, ::1/128";

    #[test]
    fn octo_android_label() {
        assert_eq!(app_label("Octo", "Octo/1.6 (Android)"), "Octo 1.6 (Android)");
        assert_eq!(app_label("Octo", "Octo/1.6.2 (Windows)"), "Octo 1.6.2 (Windows)");
        assert_eq!(app_label("Octo", "okhttp/4.12.0"), "Octo");
    }

    #[test]
    fn symfonium_label_from_client() {
        assert_eq!(
            app_label("Symfonium", "Symfonium/12.3.0 (Android 15)"),
            "Symfonium 12.3.0"
        );
        assert_eq!(app_label("Symfonium", "okhttp/4.12.0"), "Symfonium");
        assert_eq!(
            app_label("feishin", "Mozilla/5.0 Feishin/0.12.1 Chrome/1"),
            "Feishin 0.12.1"
        );
        assert_eq!(app_label("NavidromeUI", "Mozilla/5.0"), "Navidrome");
        assert_eq!(app_label("DSub", ""), "DSub");
    }

    #[test]
    fn unknown_client_keeps_its_name() {
        assert_eq!(app_label("MyPlayer", "MyPlayer/2.0"), "MyPlayer");
        assert_eq!(app_label("  ", ""), "Unknown app");
    }

    #[test]
    fn private_ranges_are_home() {
        let home = nets(HOME);
        for address in [
            "192.168.1.5",
            "10.1.2.3",
            "172.20.0.4",
            "127.0.0.1",
            "::1",
            "fd00::5",
            "::ffff:192.168.1.5",
        ] {
            assert_eq!(place_of(ip(address), &home), NetworkPlace::Home, "{address}");
        }
        for address in ["203.0.113.9", "8.8.8.8", "2001:db8::1", "172.32.0.1"] {
            assert_eq!(place_of(ip(address), &home), NetworkPlace::Away, "{address}");
        }
    }

    #[test]
    fn bare_ip_is_a_single_host() {
        let home = nets("203.0.113.9 2001:db8::7");
        assert_eq!(home[0].prefix_len(), 32);
        assert_eq!(home[1].prefix_len(), 128);
        assert_eq!(place_of(ip("203.0.113.9"), &home), NetworkPlace::Home);
        assert_eq!(place_of(ip("203.0.113.10"), &home), NetworkPlace::Away);
    }

    #[test]
    fn spaces_after_commas_are_fine() {
        let (home, rejected) = parse_networks(" 10.0.0.0/8,  192.168.0.0/16 ,,\n::1/128, nonsense ");
        assert_eq!(home.len(), 3);
        assert_eq!(rejected, ["nonsense"]);
    }

    #[test]
    fn spoofed_forwarded_for_stays_away() {
        let trusted = nets(TRUSTED);
        let home = nets(HOME);
        // The client wrote a home address into X-Forwarded-For; the local proxy appended the
        // real one. The rightmost untrusted hop is the real one.
        let address = client_address(ip("127.0.0.1"), None, Some("192.168.1.5, 203.0.113.9"), &trusted);
        assert_eq!(address, ip("203.0.113.9"));
        assert_eq!(place_of(address, &home), NetworkPlace::Away);
        // A client connecting straight to Octo cannot claim anything with headers.
        let direct = client_address(
            ip("203.0.113.9"),
            Some("192.168.1.5"),
            Some("192.168.1.5"),
            &trusted,
        );
        assert_eq!(direct, ip("203.0.113.9"));
        assert_eq!(place_of(direct, &home), NetworkPlace::Away);
    }

    #[test]
    fn tunnel_client_address_used() {
        let trusted = nets(TRUSTED);
        let home = nets(HOME);
        // cloudflared on the same host: the peer is loopback, the person is in the header.
        let address = client_address(
            ip("127.0.0.1"),
            Some("203.0.113.9"),
            Some("203.0.113.9"),
            &trusted,
        );
        assert_eq!(address, ip("203.0.113.9"));
        assert_eq!(place_of(address, &home), NetworkPlace::Away);
        // Without headers the loopback peer is the client (someone on the box itself).
        assert_eq!(client_address(ip("::1"), None, None, &trusted), ip("::1"));
    }

    #[test]
    fn trusted_hops_are_skipped_from_the_right() {
        let trusted = nets("127.0.0.0/8, 10.9.0.0/16");
        let address = client_address(
            ip("127.0.0.1"),
            None,
            Some("198.51.100.2, 192.168.1.20, 10.9.0.5"),
            &trusted,
        );
        assert_eq!(address, ip("192.168.1.20"));
        // Every hop trusted, or a hop that is not an address: the peer.
        assert_eq!(
            client_address(ip("127.0.0.1"), None, Some("10.9.0.5"), &trusted),
            ip("127.0.0.1")
        );
        assert_eq!(
            client_address(ip("127.0.0.1"), None, Some("unknown, 10.9.0.5"), &trusted),
            ip("127.0.0.1")
        );
    }

    #[test]
    fn device_round_trips_without_a_jwt_field_when_unset() {
        let device = Device {
            id: "d_1".into(),
            username: "alex".into(),
            name: "Pixel 9".into(),
            kind: DeviceKind::OctoApp,
            secret: "sealed:abc".into(),
            created: datetime::parse_utc("2026-10-20T18:00:00Z").expect("a date"),
            last_seen: None,
            app: "Octo 1.6 (Android)".into(),
            place: NetworkPlace::Away,
            seen_apps: vec!["Octo 1.6 (Android)".into()],
            navidrome_jwt: None,
        };
        let text = serde_json::to_string(&device).expect("serializes");
        assert!(!text.contains("NavidromeJwt"));
        assert!(text.contains("\"Kind\":\"OctoApp\""));
        let back: Device = serde_json::from_str(&text).expect("reads back");
        assert_eq!(back, device);
    }
}
