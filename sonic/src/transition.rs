//! A song's transition profile as Octo keeps it and the apps read it: the
//! features of its first 30 s and last 60 s, its level there every 100 ms,
//! and the whole song's level and tempo. Made from the same decoded sound
//! as the Sounds alike reading.
//!
//! The JSON, every time in milliseconds of song time and every level in
//! dBFS; a time or tempo the song does not have is left out:
//!
//! ```text
//! { "version": 1, "durationMs": 241330, "bodyDb": -11.52, "tempo": TEMPO,
//!   "head": SECTION, "tail": SECTION }
//! SECTION: { "startMs", "hopMs", "levels", "bodyDb", "gateDb", "soundStartMs",
//!   "soundEndMs", "outroStartMs", "introEndMs", "boundariesMs": [..], "tempo": TEMPO }
//! TEMPO: { "bpm", "trusted", "beatMs", "firstBeatMs", "downbeatMs", "confidence",
//!   "consistency", "steady" }
//! ```
//!
//! `levels` is base64 of one byte per hop: the level in half-dB steps above
//! -100 dBFS. The tail's features are measured against the whole song's
//! level.

use serde_json::{json, Map, Value};

use crate::automix::features::{SectionFeatures, Tempo};
use crate::automix::profile::{level_code, SectionProfile, TransitionProfile, PROFILE_VERSION};

/// The profile version this octo-sonic writes.
pub fn version() -> u32 {
    PROFILE_VERSION
}

/// The profile of decoded mono sound at `sample_rate`.
pub fn analyze(mono: &[f32], sample_rate: u32) -> TransitionProfile {
    TransitionProfile::analyze(mono, sample_rate, 1)
}

/// The profile as JSON.
pub fn to_json(profile: &TransitionProfile) -> Value {
    let mut out = Map::new();
    out.insert("version".into(), json!(profile.version));
    out.insert("durationMs".into(), json!(profile.duration_ms));
    if let Some(body) = profile.body_db {
        out.insert("bodyDb".into(), json!(decibels(body)));
    }
    if let Some(tempo) = &profile.tempo {
        out.insert("tempo".into(), tempo_json(tempo));
    }
    out.insert("head".into(), section_json(&profile.head));
    out.insert("tail".into(), section_json(&profile.tail));
    Value::Object(out)
}

fn section_json(section: &SectionProfile) -> Value {
    let codes: Vec<u8> = section.levels.iter().map(|&db| level_code(db)).collect();
    let mut out = Map::new();
    out.insert("startMs".into(), json!(section.start_ms));
    out.insert("hopMs".into(), json!(section.hop_ms));
    out.insert("levels".into(), json!(base64(&codes)));
    features_into(&mut out, &section.features);
    Value::Object(out)
}

fn features_into(out: &mut Map<String, Value>, f: &SectionFeatures) {
    out.insert("bodyDb".into(), json!(decibels(f.body_db)));
    out.insert("gateDb".into(), json!(decibels(f.gate_db)));
    let times = [
        ("soundStartMs", f.sound_start_ms),
        ("soundEndMs", f.sound_end_ms),
        ("outroStartMs", f.outro_start_ms),
        ("introEndMs", f.intro_end_ms),
    ];
    for (name, time) in times {
        if let Some(ms) = time {
            out.insert(name.into(), json!(ms));
        }
    }
    out.insert("boundariesMs".into(), json!(f.boundaries_ms));
    if let Some(tempo) = &f.tempo {
        out.insert("tempo".into(), tempo_json(tempo));
    }
}

// The beat grid in full: the planner builds bars from it over many beats.
fn tempo_json(t: &Tempo) -> Value {
    json!({
        "bpm": t.bpm,
        "trusted": t.confident(),
        "beatMs": t.beat_ms,
        "firstBeatMs": t.first_beat_ms,
        "downbeatMs": t.downbeat_ms,
        "confidence": t.confidence,
        "consistency": t.consistency,
        "steady": t.steady,
    })
}

// A level to a thousandth of a dB.
fn decibels(db: f64) -> f64 {
    (db * 1000.0).round() / 1000.0
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automix::profile::level_of_code;
    use crate::automix::synthetic::{SongLayer, SyntheticSong};

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/transition-profile.json");

    fn layer(kind: &str, from_ms: f64, to_ms: f64, hz: f64, db: f64) -> SongLayer {
        SongLayer {
            kind: kind.into(),
            from_ms,
            to_ms,
            hz,
            db,
            end_db: db,
            bpm: 120.0,
            first_ms: 4_000.0,
            every: 1,
            decay_ms: 10.0,
        }
    }

    /// 75 s at 120 BPM: a quiet 4 s intro, a kick on every bar and a click
    /// on every beat, then a 15 s fade from 60 s.
    fn fixture_song() -> SyntheticSong {
        let mut pad = layer("tone", 0.0, 4_000.0, 300.0, -40.0);
        pad.end_db = -40.0;
        let mut body = layer("tone", 4_000.0, 60_000.0, 300.0, -22.0);
        body.end_db = -22.0;
        let mut fade = layer("tone", 60_000.0, 75_000.0, 300.0, -22.0);
        fade.end_db = -70.0;
        let kick = SongLayer {
            every: 4,
            decay_ms: 80.0,
            ..layer("pulse", 4_000.0, 60_000.0, 60.0, -6.0)
        };
        let click = layer("pulse", 4_000.0, 60_000.0, 2_000.0, -16.0);
        SyntheticSong {
            rate: 22_050,
            channels: 1,
            length_ms: 75_000,
            layers: vec![pad, body, fade, kick, click],
        }
    }

    fn fixture_profile() -> Value {
        let song = fixture_song();
        to_json(&analyze(&song.render_mono(0.0, song.length_ms as f64), song.rate))
    }

    fn decode64(text: &str) -> Vec<u8> {
        let value = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        let mut out = Vec::new();
        for chunk in text.as_bytes().chunks(4) {
            let n = chunk
                .iter()
                .take_while(|&&c| c != b'=')
                .enumerate()
                .fold(0u32, |n, (i, &c)| n | u32::from(value(c)) << (18 - 6 * i));
            let kept = chunk.iter().filter(|&&c| c != b'=').count() - 1;
            out.extend((0..kept).map(|i| (n >> (16 - 8 * i)) as u8));
        }
        out
    }

    // Equal but for the last bits of floating point, which differ between
    // machines: numbers within a hundredth, levels within one step.
    fn alike(path: &str, want: &Value, got: &Value, off: &mut Vec<String>) {
        match (want, got) {
            (Value::Object(w), Value::Object(g)) => {
                let (mut wk, mut gk): (Vec<_>, Vec<_>) = (w.keys().collect(), g.keys().collect());
                wk.sort();
                gk.sort();
                if wk != gk {
                    off.push(format!("{path}: keys {gk:?}, expected {wk:?}"));
                    return;
                }
                for (key, w) in w {
                    alike(&format!("{path}.{key}"), w, &g[key], off);
                }
            }
            (Value::Array(w), Value::Array(g)) if w.len() == g.len() => {
                for (i, (w, g)) in w.iter().zip(g).enumerate() {
                    alike(&format!("{path}[{i}]"), w, g, off);
                }
            }
            (Value::Number(w), Value::Number(g)) => {
                let (w, g) = (w.as_f64().unwrap_or(f64::NAN), g.as_f64().unwrap_or(f64::NAN));
                if (w - g).abs() > 0.01 {
                    off.push(format!("{path}: {g}, expected {w}"));
                }
            }
            (Value::String(w), Value::String(g)) if path.ends_with(".levels") => {
                let (w, g) = (decode64(w), decode64(g));
                if w.len() != g.len() || w.iter().zip(&g).any(|(a, b)| a.abs_diff(*b) > 1) {
                    off.push(format!("{path}: levels differ"));
                }
            }
            (w, g) if w == g => {}
            (w, g) => off.push(format!("{path}: {g}, expected {w}")),
        }
    }

    /// The fixture the apps read their profile parsing against
    /// (`transition-profile.json`, copied into their tests by
    /// `sync-automix.sh`). `OCTO_WRITE_FIXTURE=1 cargo test` writes it again
    /// after a change to the form.
    #[test]
    fn the_profile_is_the_one_the_apps_read() {
        let got = fixture_profile();
        if std::env::var("OCTO_WRITE_FIXTURE").is_ok_and(|v| v == "1") {
            let text = serde_json::to_string_pretty(&got).expect("JSON") + "\n";
            std::fs::write(FIXTURE, text).expect("the fixture is written");
        }
        let want: Value =
            serde_json::from_str(&std::fs::read_to_string(FIXTURE).expect("the fixture")).expect("JSON");
        let mut off = Vec::new();
        alike("profile", &want, &got, &mut off);
        assert!(off.is_empty(), "{}", off.join("\n"));
    }

    #[test]
    fn the_fixture_song_has_what_a_blend_needs() {
        let p = fixture_profile();
        assert_eq!(p["version"], json!(1));
        assert_eq!(p["durationMs"], json!(75_000));
        assert_eq!(
            (p["head"]["startMs"].as_i64(), p["head"]["hopMs"].as_i64()),
            (Some(0), Some(100))
        );
        assert_eq!(p["tail"]["startMs"], json!(15_000));
        let intro = p["head"]["introEndMs"].as_i64().expect("an intro end");
        assert!((3_400..=4_300).contains(&intro), "{intro}");
        let outro = p["tail"]["outroStartMs"].as_i64().expect("an outro");
        assert!((58_000..=66_000).contains(&outro), "{outro}");
        let tempo = &p["tempo"];
        assert!((tempo["bpm"].as_f64().unwrap() - 120.0).abs() < 0.5, "{tempo}");
        assert_eq!(decode64(p["tail"]["levels"].as_str().unwrap()).len(), 600);
        assert_eq!(decode64(p["head"]["levels"].as_str().unwrap()).len(), 300);
        assert!(p["tail"]["boundariesMs"].is_array());
        // Small enough to keep for every song and ask for at every song change.
        let size = serde_json::to_string(&p).unwrap().len();
        assert!(size < 2_500, "{size} bytes");
    }

    #[test]
    fn a_silent_song_leaves_out_what_it_does_not_have() {
        let p = to_json(&analyze(&vec![0.0; 22_050 * 40], 22_050));
        assert!(p.get("bodyDb").is_none() && p.get("tempo").is_none());
        assert!(p["tail"].get("soundStartMs").is_none() && p["tail"].get("tempo").is_none());
        assert!(decode64(p["tail"]["levels"].as_str().unwrap())
            .iter()
            .all(|&c| c == 0));
    }

    #[test]
    fn base64_is_the_standard_one() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0, 255, 171, 7]), "AP+rBw==");
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode64(&base64(&all)), all);
        assert_eq!(level_of_code(level_code(-14.26)), -14.5);
    }
}
