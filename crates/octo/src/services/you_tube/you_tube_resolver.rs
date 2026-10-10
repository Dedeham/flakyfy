//! Port of `Services/YouTube/YouTubeResolver.cs`: the client for the yt-dlp shim.

use std::time::Duration;

use octo_core::settings::SettingsStore;
use octo_core::soulseek::StreamFacts;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;
use tracing::{debug, warn};

use super::you_tube_music_client::{YouTubeMusicClient, pick_song};

/// One video the shim found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YouTubeHit {
    pub video_id: String,
    pub title: Option<String>,
    pub duration: Option<i32>,
    pub channel: Option<String>,
    /// The audio stream the shim picked for this video, when the run that found it had one
    /// (/search does; /meta never picks a stream).
    pub stream: StreamFacts,
}

/// An open `/stream` response. The caller owns it and reads the body from `response`
/// (`bytes_stream()`), which stays open for the whole song.
#[derive(Debug)]
pub struct ShimStream {
    pub response: reqwest::Response,
    pub content_type: String,
    pub content_length: Option<u64>,
    pub status_code: u16,
    pub content_range: Option<String>,
}

/// Pure HTTP client for the yt-dlp-shim sidecar service. Octo never spawns
/// yt-dlp itself: the shim wraps it behind /search and /stream endpoints
/// in its own container, so any process-management quirks stay isolated.
pub struct YouTubeResolver {
    /// `yt-dlp-shim-search`: 60 s rather than 30 s because back-to-back search3 prewarm
    /// bursts can fill the shim's yt-dlp gate (MAX_CONCURRENT_YTDLP, which ships as 5) and
    /// queue requests behind 5-8 s yt-dlp ytsearch1: invocations. 30 s was cancelling the tail
    /// of every prewarm batch.
    search_client: reqwest::Client,
    /// `yt-dlp-shim-stream`: no timeout, because /stream stays open for the whole song and a
    /// client timeout would kill the read mid-track.
    stream_client: reqwest::Client,
    base_url: String,
    /// YouTube Music through the same shim; off while the shim's address is not set.
    music: YouTubeMusicClient,
}

/// `Uri.EscapeDataString`: everything but the RFC 3986 unreserved characters.
const DATA_STRING: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

fn escape(text: &str) -> String {
    utf8_percent_encode(text, DATA_STRING).to_string()
}

impl YouTubeResolver {
    /// The named clients' names in the C#.
    pub const SEARCH_CLIENT_NAME: &'static str = "yt-dlp-shim-search";
    pub const STREAM_CLIENT_NAME: &'static str = "yt-dlp-shim-stream";

    pub const DEFAULT_BASE_URL: &'static str = "http://yt-dlp-shim:8080";

    pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(60);

    /// How long a YouTube Music lookup may take before the yt-dlp lookup is asked instead.
    pub const MUSIC_TIMEOUT: Duration = Duration::from_secs(3);

    /// Rows one YouTube Music lookup asks for.
    pub const MUSIC_ROWS: i32 = 5;

    /// Reads `YouTube:ShimUrl` once: the address is deliberately captured at startup (it is a
    /// restart-only setting; see RestartTracker).
    pub fn new(settings: &SettingsStore) -> Self {
        Self::with_base_url(settings.raw("YouTube:ShimUrl").as_deref())
    }

    /// A resolver on the given shim address (blank → the compose service name).
    pub fn with_base_url(configured: Option<&str>) -> Self {
        // IHttpClientFactory's clients did not decompress, so neither do these: /stream
        // passes the shim's bytes and Content-Length straight through.
        let search_client = reqwest::Client::builder()
            .timeout(Self::SEARCH_TIMEOUT)
            .no_gzip()
            .no_deflate()
            .build()
            .expect("a plain HTTP client builds");
        let stream_client = reqwest::Client::builder()
            .no_gzip()
            .no_deflate()
            .build()
            .expect("a plain HTTP client builds");
        let shim = configured.map(str::trim).filter(|c| !c.is_empty());
        YouTubeResolver {
            search_client,
            stream_client,
            base_url: Self::resolve_base_url(configured),
            music: YouTubeMusicClient::with_shim_url(shim),
        }
    }

    /// The shim address every request is built on. Falls back to the compose
    /// service name when the setting is absent OR blank: the admin UI saves a
    /// cleared field as "", and "" is not null, so a plain null-coalesce left
    /// every request relative and the factory client with no base address.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn resolve_base_url(configured: Option<&str>) -> String {
        let value = match configured {
            Some(c) if !c.trim().is_empty() => c.trim(),
            _ => Self::DEFAULT_BASE_URL,
        };
        value.trim_end_matches('/').to_string()
    }

    fn query_url(&self, endpoint: &str, query: &str, duration_hint: Option<i32>, background: bool) -> String {
        let mut url = format!("{}/{endpoint}?q={}", self.base_url, escape(query));
        if let Some(dh) = duration_hint.filter(|&d| d > 0) {
            url.push_str(&format!("&duration={dh}"));
        }
        if background {
            url.push_str("&bg=1");
        }
        url
    }

    /// Resolves "Artist - Title" to a single best YouTube hit via the shim.
    ///
    /// `background` is true only for fire-and-forget prewarm. The shim keeps a slice of its
    /// yt-dlp gate unreachable to background work, so a user pressing play never queues behind
    /// a prewarm burst. Absence means interactive, so a call site that forgets this fails
    /// safe: slower prewarm, never a slower play.
    pub async fn search(
        &self,
        query: &str,
        duration_hint: Option<i32>,
        background: bool,
    ) -> Option<YouTubeHit> {
        if query.trim().is_empty() {
            return None;
        }
        let url = self.query_url("search", query, duration_hint, background);
        let outcome: anyhow::Result<Option<YouTubeHit>> = async {
            let resp = self.search_client.get(&url).send().await?;
            if !resp.status().is_success() {
                debug!("shim /search HTTP {} for '{query}'", resp.status().as_u16());
                return Ok(None);
            }
            let root: Value = serde_json::from_str(&resp.text().await?)?;
            parse_hit(&root, true)
        }
        .await;
        outcome.unwrap_or_else(|e| {
            warn!("shim /search failed for '{query}': {e}");
            None
        })
    }

    /// The video of `artist - title` on YouTube Music, with its length: the row of a songs search
    /// that is the same song ([`pick_song`]), `length` being the length known for the song. None
    /// when no row is, when YouTube Music cannot answer within [`Self::MUSIC_TIMEOUT`] (busy,
    /// failed or unreachable), or when the shim's address is not set; the caller then asks
    /// yt-dlp. The hit names no stream, since YouTube Music only searches.
    pub async fn music_song(&self, artist: &str, title: &str, length: Option<i32>) -> Option<YouTubeHit> {
        if !self.music.configured() || (artist.trim().is_empty() && title.trim().is_empty()) {
            return None;
        }
        let query = format!("{artist} {title}");
        let lookup = self.music.search_limited(&query, "songs", Self::MUSIC_ROWS);
        let rows = match tokio::time::timeout(Self::MUSIC_TIMEOUT, lookup).await {
            Ok(Ok(Some(rows))) => rows,
            Ok(Ok(None)) => {
                debug!("YouTube Music could not answer for '{query}'");
                return None;
            }
            Ok(Err(e)) => {
                debug!("YouTube Music failed for '{query}': {e}");
                return None;
            }
            Err(_) => {
                debug!(
                    "YouTube Music did not answer within {:?} for '{query}'",
                    Self::MUSIC_TIMEOUT
                );
                return None;
            }
        };
        let Some(row) = pick_song(artist, title, length, &rows) else {
            debug!("no YouTube Music row is '{query}' (length {length:?})");
            return None;
        };
        Some(YouTubeHit {
            video_id: row.video_id.clone(),
            title: Some(row.title.clone()),
            duration: row.duration_seconds,
            channel: None,
            stream: StreamFacts::default(),
        })
    }

    /// Fast metadata-only lookup via the shim's /meta (flat search, no URL
    /// resolution). Returns the top video's id + duration for showing an accurate
    /// length without paying the full /search extraction.
    ///
    /// `background` means the same as on [`Self::search`]: true only for fire-and-forget
    /// prewarm, so the shim's gate never makes an interactive caller queue behind it.
    pub async fn meta(
        &self,
        query: &str,
        duration_hint: Option<i32>,
        background: bool,
    ) -> Option<YouTubeHit> {
        if query.trim().is_empty() {
            return None;
        }
        let url = self.query_url("meta", query, duration_hint, background);
        let outcome: anyhow::Result<Option<YouTubeHit>> = async {
            let resp = self.search_client.get(&url).send().await?;
            if !resp.status().is_success() {
                return Ok(None);
            }
            let root: Value = serde_json::from_str(&resp.text().await?)?;
            parse_hit(&root, false)
        }
        .await;
        outcome.ok().flatten()
    }

    /// What the shim's /stream would send for `video_id`: its format, container, bit rate and
    /// exact size. The shim resolves the video when it has not yet (as /stream would, so a play
    /// after this is a cache hit). None when the shim could not.
    pub async fn facts(&self, video_id: &str, background: bool) -> Option<StreamFacts> {
        if video_id.trim().is_empty() {
            return None;
        }
        let mut url = format!("{}/facts?id={}", self.base_url, escape(video_id));
        if background {
            url.push_str("&bg=1");
        }
        let outcome: anyhow::Result<Option<StreamFacts>> = async {
            let resp = self.search_client.get(&url).send().await?;
            if !resp.status().is_success() {
                debug!("shim /facts HTTP {} for {video_id}", resp.status().as_u16());
                return Ok(None);
            }
            let root: Value = serde_json::from_str(&resp.text().await?)?;
            Ok(Some(parse_stream(&root)).filter(|facts| !facts.is_empty()))
        }
        .await;
        outcome.unwrap_or_else(|e| {
            warn!("shim /facts failed for {video_id}: {e}");
            None
        })
    }

    /// Opens a streaming connection to the shim's /stream endpoint. The shim
    /// proxies bytes from YouTube's CDN to us; we hand the resulting stream
    /// off to the handler, which forwards it to the Subsonic client.
    ///
    /// `range_header` is forwarded verbatim to the shim, which passes it on to googlevideo.
    /// iOS Subsonic clients (Arpeggi, Narjo) probe with `Range: bytes=0-1` and won't play
    /// audio/mp4 unless the server returns 206 with a valid Content-Range, so this passthrough
    /// is required for them to even attempt playback.
    ///
    /// None on failure. The caller owns the returned response.
    pub async fn open_stream(&self, video_id: &str, range_header: Option<&str>) -> Option<ShimStream> {
        if video_id.trim().is_empty() {
            return None;
        }
        let url = format!("{}/stream?id={}", self.base_url, escape(video_id));
        let mut request = self.stream_client.get(&url);
        if let Some(range) = range_header.filter(|r| !r.is_empty()) {
            request = request.header(reqwest::header::RANGE, range);
        }
        let resp = match request.send().await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("shim /stream failed for {video_id}: {e}");
                return None;
            }
        };
        // Accept 200 (full body), 206 (partial content) and 416 (a range past the end, whose
        // `bytes */<size>` is the client's answer). Anything else means the upstream/shim
        // couldn't satisfy the request.
        let status = resp.status().as_u16();
        if !matches!(status, 200 | 206 | 416) {
            warn!("shim /stream HTTP {status} for {video_id}");
            return None;
        }
        let header = |name: reqwest::header::HeaderName| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header(reqwest::header::CONTENT_TYPE).unwrap_or_else(|| "audio/mp4".to_string());
        let content_range = header(reqwest::header::CONTENT_RANGE);
        let content_length = resp.content_length();
        Some(ShimStream {
            response: resp,
            content_type,
            content_length,
            status_code: status,
            content_range,
        })
    }

    /// Downloads a YouTube video as MP3 to `{dest_without_ext}.mp3` via the shim's
    /// /download endpoint, passing clean artist/title so the file is tagged for
    /// the library. Returns the saved path, or None on failure. Uses the
    /// no-timeout stream client because a download can take a while.
    pub async fn download(
        &self,
        video_id: &str,
        dest_without_ext: &str,
        artist: Option<&str>,
        title: Option<&str>,
    ) -> Option<String> {
        if video_id.trim().is_empty() || dest_without_ext.trim().is_empty() {
            return None;
        }
        let mut url = format!(
            "{}/download?id={}&dest={}",
            self.base_url,
            escape(video_id),
            escape(dest_without_ext)
        );
        if let Some(artist) = artist.filter(|a| !a.is_empty()) {
            url.push_str(&format!("&artist={}", escape(artist)));
        }
        if let Some(title) = title.filter(|t| !t.is_empty()) {
            url.push_str(&format!("&title={}", escape(title)));
        }
        let outcome: anyhow::Result<Option<String>> = async {
            let resp = self.stream_client.get(&url).send().await?;
            if !resp.status().is_success() {
                warn!(
                    "shim /download HTTP {} for vid={video_id}",
                    resp.status().as_u16()
                );
                return Ok(None);
            }
            let root: Value = serde_json::from_str(&resp.text().await?)?;
            json_string(&root, "path")
        }
        .await;
        outcome.unwrap_or_else(|e| {
            warn!("shim /download failed for {video_id}: {e}");
            None
        })
    }
}

/// `TryGetProperty(name, out v) ? v.GetString() : null`: absent or null → None; a value that
/// is not a string threw (`InvalidOperationException`), and so does a root that is not an
/// object, which the callers caught.
fn json_string(root: &Value, name: &str) -> anyhow::Result<Option<String>> {
    let object = root
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("the answer is not a JSON object"))?;
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => anyhow::bail!("'{name}' is not a string"),
    }
}

/// The hit in a /search or /meta answer; None without a video id. `with_channel` for /search,
/// whose hit names the channel.
fn parse_hit(root: &Value, with_channel: bool) -> anyhow::Result<Option<YouTubeHit>> {
    let Some(video_id) = json_string(root, "video_id")?.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let title = json_string(root, "title")?;
    // `d.ValueKind == Number ? d.GetInt32() : null`: GetInt32 threw on a number that is not
    // an Int32.
    let duration = match root.get("duration") {
        Some(Value::Number(n)) => Some(
            n.as_i64()
                .and_then(|d| i32::try_from(d).ok())
                .ok_or_else(|| anyhow::anyhow!("'duration' is not an Int32"))?,
        ),
        _ => None,
    };
    let channel = if with_channel {
        json_string(root, "channel")?
    } else {
        None
    };
    Ok(Some(YouTubeHit {
        video_id,
        title,
        duration,
        channel,
        stream: parse_stream(root),
    }))
}

/// The stream facts in a /search or /facts answer (`size`, `itag`, `ext`, `abr`, and, once a play
/// measured it, `track_gain` and `track_peak`). A field that is missing or not what it should be
/// is left unknown rather than failing the answer.
fn parse_stream(root: &Value) -> StreamFacts {
    let text = |name: &str| {
        root.get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let number = |name: &str| root.get(name).and_then(Value::as_f64).filter(|n| n.is_finite());
    let track_gain = number("track_gain");
    StreamFacts {
        track_gain,
        // A peak means nothing without the gain it was measured with.
        track_peak: track_gain.and(number("track_peak")).filter(|peak| *peak > 0.0),
        size: root.get("size").and_then(Value::as_u64).filter(|size| *size > 0),
        // Reported, not played: only the bytes of a play confirm it.
        size_confirmed: false,
        itag: text("itag"),
        ext: text("ext"),
        bit_rate: root
            .get("abr")
            .and_then(Value::as_f64)
            .map(f64::round)
            .filter(|kbps| *kbps > 0.0 && *kbps < f64::from(i32::MAX))
            .map(|kbps| kbps as i32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---- YouTubeResolverBaseUrlTests ----------------------------------------------------

    fn build(shim_url: Option<&str>) -> YouTubeResolver {
        let store = SettingsStore::for_tests(Default::default());
        if let Some(url) = shim_url {
            store.set_raw("YouTube:ShimUrl", Some(url));
        }
        YouTubeResolver::new(&store)
    }

    #[test]
    fn absent_setting_uses_the_compose_service_name() {
        assert_eq!(build(None).base_url(), YouTubeResolver::DEFAULT_BASE_URL);
    }

    #[test]
    fn blank_setting_uses_the_compose_service_name() {
        for shim_url in ["", "   "] {
            assert_eq!(
                build(Some(shim_url)).base_url(),
                YouTubeResolver::DEFAULT_BASE_URL,
                "{shim_url:?}"
            );
        }
    }

    #[test]
    fn configured_setting_is_kept_without_a_trailing_slash() {
        for (shim_url, expected) in [
            ("http://shim.local:8080", "http://shim.local:8080"),
            ("http://shim.local:8080/", "http://shim.local:8080"),
            ("  http://shim.local:8080/  ", "http://shim.local:8080"),
        ] {
            assert_eq!(build(Some(shim_url)).base_url(), expected, "{shim_url:?}");
        }
    }

    // ---- Rust-only: the shim's endpoints ------------------------------------------------

    #[test]
    fn queries_are_escaped_as_uri_escape_data_string_did() {
        // .NET 9: Uri.EscapeDataString("a b+c/é~*'()!")
        assert_eq!(escape("a b+c/é~*'()!"), "a%20b%2Bc%2F%C3%A9~%2A%27%28%29%21");
    }

    #[tokio::test]
    async fn search_asks_with_the_hint_and_background_flag_and_reads_the_hit() {
        let shim = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "Daft Punk Emotion"))
            .and(query_param("duration", "417"))
            .and(query_param("bg", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"video_id":"abc","title":"Emotion","duration":417,"channel":"Daft Punk - Topic"}"#,
            ))
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&format!("{}/", shim.uri())));
        let hit = resolver.search("Daft Punk Emotion", Some(417), true).await;
        assert_eq!(
            hit,
            Some(YouTubeHit {
                video_id: "abc".into(),
                title: Some("Emotion".into()),
                duration: Some(417),
                channel: Some("Daft Punk - Topic".into()),
                stream: StreamFacts::default(),
            })
        );
        assert!(resolver.search("  ", None, false).await.is_none());
    }

    #[tokio::test]
    async fn search_reads_the_stream_the_shim_picked() {
        let shim = MockServer::start().await;
        Mock::given(path("/search"))
            .and(query_param("q", "Daft Punk Emotion"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"video_id":"abc","duration":417,"size":6755585,"itag":"140","ext":"m4a","abr":129.486}"#,
            ))
            .mount(&shim)
            .await;
        Mock::given(path("/search"))
            .and(query_param("q", "odd stream"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"video_id":"x","size":"big","itag":140,"abr":null}"#),
            )
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&shim.uri()));
        let hit = resolver
            .search("Daft Punk Emotion", None, false)
            .await
            .expect("a hit");
        assert_eq!(
            hit.stream,
            StreamFacts {
                size: Some(6_755_585),
                itag: Some("140".into()),
                ext: Some("m4a".into()),
                bit_rate: Some(129),
                ..Default::default()
            }
        );
        // Odd stream fields are unknown, and the hit still counts.
        let odd = resolver.search("odd stream", None, false).await.expect("a hit");
        assert_eq!(odd.stream, StreamFacts::default());
    }

    #[tokio::test]
    async fn facts_asks_for_the_video_in_the_background_and_reads_its_stream() {
        let shim = MockServer::start().await;
        Mock::given(path("/facts"))
            .and(query_param("id", "abc"))
            .and(query_param("bg", "1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"video_id":"abc","size":7012345,"itag":"251","ext":"webm","abr":160}"#,
                ),
            )
            .mount(&shim)
            .await;
        Mock::given(path("/facts"))
            .and(query_param("id", "gone"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&shim)
            .await;
        Mock::given(path("/facts"))
            .and(query_param("id", "empty"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"video_id":"empty"}"#))
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&shim.uri()));
        let facts = resolver.facts("abc", true).await.expect("facts");
        assert_eq!(facts.size, Some(7_012_345));
        assert_eq!((facts.track_gain, facts.track_peak), (None, None));
        assert_eq!(facts.itag.as_deref(), Some("251"));
        assert_eq!(facts.bit_rate, Some(160));
        assert!(resolver.facts("gone", true).await.is_none());
        assert!(resolver.facts("empty", true).await.is_none());
        assert!(resolver.facts(" ", true).await.is_none());
    }

    #[test]
    fn facts_read_the_measured_replay_gain_and_its_peak() {
        let facts = parse_stream(&serde_json::json!({
            "video_id": "abc", "size": 6755585, "track_gain": -10.0, "track_peak": 1.135011
        }));
        assert_eq!(facts.track_gain, Some(-10.0));
        assert_eq!(facts.track_peak, Some(1.135011));
        // A silent track has no peak; a peak alone is not taken.
        let silent = parse_stream(&serde_json::json!({"track_gain": 3.5, "track_peak": null}));
        assert_eq!((silent.track_gain, silent.track_peak), (Some(3.5), None));
        let peak_only = parse_stream(&serde_json::json!({"track_peak": 0.9}));
        assert_eq!(peak_only, StreamFacts::default());
    }

    #[tokio::test]
    async fn search_and_meta_answer_none_on_a_miss_an_error_or_an_odd_answer() {
        let shim = MockServer::start().await;
        Mock::given(path("/search"))
            .and(query_param("q", "missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&shim)
            .await;
        Mock::given(path("/search"))
            .and(query_param("q", "no id"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"video_id":"","title":"x"}"#))
            .mount(&shim)
            .await;
        Mock::given(path("/search"))
            .and(query_param("q", "odd"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"video_id":"v","duration":1.5}"#))
            .mount(&shim)
            .await;
        Mock::given(path("/meta"))
            .and(query_param("q", "meta"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"video_id":"m","duration":"200","channel":"ignored"}"#),
            )
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&shim.uri()));
        assert!(resolver.search("missing", None, false).await.is_none());
        assert!(resolver.search("no id", None, false).await.is_none());
        assert!(resolver.search("odd", None, false).await.is_none());
        let meta = resolver.meta("meta", Some(0), false).await.expect("a hit");
        assert_eq!(
            (meta.video_id.as_str(), meta.duration, meta.channel),
            ("m", None, None)
        );
        assert!(resolver.meta("other", None, false).await.is_none());
    }

    #[tokio::test]
    async fn open_stream_passes_the_range_and_reports_the_partial_answer() {
        let shim = MockServer::start().await;
        Mock::given(path("/stream"))
            .and(query_param("id", "abc"))
            .and(header("range", "bytes=0-1"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-1/1000")
                    .set_body_raw(vec![1u8, 2], "audio/webm"),
            )
            .mount(&shim)
            .await;
        Mock::given(path("/stream"))
            .and(query_param("id", "gone"))
            .respond_with(ResponseTemplate::new(410))
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&shim.uri()));
        let stream = resolver
            .open_stream("abc", Some("bytes=0-1"))
            .await
            .expect("a stream");
        assert_eq!(stream.status_code, 206);
        assert_eq!(stream.content_type, "audio/webm");
        assert_eq!(stream.content_length, Some(2));
        assert_eq!(stream.content_range.as_deref(), Some("bytes 0-1/1000"));
        assert_eq!(
            stream.response.bytes().await.expect("the body").as_ref(),
            &[1u8, 2]
        );
        assert!(resolver.open_stream("gone", None).await.is_none());
        assert!(resolver.open_stream(" ", None).await.is_none());
    }

    #[tokio::test]
    async fn open_stream_hands_back_a_range_past_the_end_with_the_size() {
        let shim = MockServer::start().await;
        Mock::given(path("/stream"))
            .and(query_param("id", "abc"))
            .respond_with(ResponseTemplate::new(416).insert_header("content-range", "bytes */1000"))
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&shim.uri()));
        let stream = resolver
            .open_stream("abc", Some("bytes=5000-"))
            .await
            .expect("the refusal is an answer");
        assert_eq!(stream.status_code, 416);
        assert_eq!(stream.content_range.as_deref(), Some("bytes */1000"));
    }

    #[tokio::test]
    async fn download_names_the_destination_and_tags_and_returns_the_path() {
        let shim = MockServer::start().await;
        Mock::given(path("/download"))
            .and(query_param("id", "abc"))
            .and(query_param("dest", "/music/A/B"))
            .and(query_param("artist", "A & B"))
            .and(query_param("title", "T"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"path":"/music/A/B.mp3"}"#))
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&shim.uri()));
        assert_eq!(
            resolver
                .download("abc", "/music/A/B", Some("A & B"), Some("T"))
                .await
                .as_deref(),
            Some("/music/A/B.mp3")
        );
        assert!(
            resolver
                .download("abc", "/elsewhere", None, Some(""))
                .await
                .is_none()
        );
        assert!(resolver.download("", "/x", None, None).await.is_none());
    }

    const HELLO_ROWS: &str = r#"{"tracks":[
        {"videoId":"omv1","title":"Hello","artists":["Adele"],"durationSeconds":366,"videoType":"MUSIC_VIDEO_TYPE_OMV"},
        {"videoId":"atv1","title":"Hello","artists":["Adele"],"durationSeconds":295,"videoType":"MUSIC_VIDEO_TYPE_ATV"}
    ]}"#;

    /// YouTube Music is asked for five songs rows, and its release track comes back with its
    /// length, naming no stream.
    #[tokio::test]
    async fn music_song_reads_the_release_track_of_a_songs_search() {
        let shim = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ytm/search"))
            .and(query_param("q", "Adele Hello"))
            .and(query_param("filter", "songs"))
            .and(query_param("limit", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_string(HELLO_ROWS))
            .expect(1)
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&shim.uri()));
        let hit = resolver.music_song("Adele", "Hello", Some(296)).await;
        assert_eq!(
            hit,
            Some(YouTubeHit {
                video_id: "atv1".into(),
                title: Some("Hello".into()),
                duration: Some(295),
                channel: None,
                stream: StreamFacts::default(),
            })
        );
    }

    /// A busy or failed YouTube Music, a row too far from the known length, a slow answer and a
    /// shim with no address set are all no hit, so the caller asks yt-dlp.
    #[tokio::test]
    async fn music_song_is_none_when_youtube_music_cannot_say() {
        let shim = MockServer::start().await;
        Mock::given(path("/ytm/search"))
            .and(query_param("q", "Busy Song"))
            .respond_with(ResponseTemplate::new(503).set_body_string(r#"{"error":"busy"}"#))
            .mount(&shim)
            .await;
        Mock::given(path("/ytm/search"))
            .and(query_param("q", "Broken Song"))
            .respond_with(ResponseTemplate::new(502).set_body_string(r#"{"error":"ytm_failed"}"#))
            .mount(&shim)
            .await;
        Mock::given(path("/ytm/search"))
            .and(query_param("q", "Slow Song"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(
                        r#"{"tracks":[{"videoId":"s","title":"Song","artists":["Slow"],"durationSeconds":200}]}"#,
                    )
                    .set_delay(YouTubeResolver::MUSIC_TIMEOUT + Duration::from_millis(500)),
            )
            .mount(&shim)
            .await;
        Mock::given(path("/ytm/search"))
            .and(query_param("q", "Adele Hello"))
            .respond_with(ResponseTemplate::new(200).set_body_string(HELLO_ROWS))
            .mount(&shim)
            .await;
        let resolver = YouTubeResolver::with_base_url(Some(&shim.uri()));
        assert!(resolver.music_song("Busy", "Song", None).await.is_none());
        assert!(resolver.music_song("Broken", "Song", None).await.is_none());
        assert!(resolver.music_song("Adele", "Hello", Some(250)).await.is_none());
        let started = std::time::Instant::now();
        assert!(resolver.music_song("Slow", "Song", None).await.is_none());
        assert!(started.elapsed() < YouTubeResolver::MUSIC_TIMEOUT + Duration::from_millis(400));
        assert!(
            YouTubeResolver::with_base_url(None)
                .music_song("Adele", "Hello", None)
                .await
                .is_none()
        );
    }
}
