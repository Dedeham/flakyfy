//! YouTube Music search and radio, through the yt-dlp shim's `/ytm` endpoints.

use octo_core::common::dotnet;
use octo_core::json::element::{ElementResult, enumerate_array, get_int32, try_get_property};
use octo_core::settings::SettingsStore;
use serde_json::Value;
use tracing::debug;

use super::YouTubeResolver;

/// One row of YouTube Music, as the shim reads it through ytmusicapi.
///
/// `video_type`: MUSIC_VIDEO_TYPE_ATV (a release), OMV (official video), UGC (an upload) or
/// PODCAST_EPISODE; None when YouTube Music did not say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YtmRow {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_seconds: Option<i32>,
    pub video_type: Option<String>,
}

pub struct YouTubeMusicClient {
    /// `yt-dlp-shim-search`, the shim's 60-second client.
    http: reqwest::Client,
    shim_url: Option<String>,
}

impl YouTubeMusicClient {
    /// Reads `YouTube:ShimUrl` once, like YouTubeResolver: the shim's address changes only with a
    /// restart.
    pub fn new(settings: &SettingsStore) -> Self {
        Self::with_shim_url(settings.raw("YouTube:ShimUrl").as_deref())
    }

    pub fn with_shim_url(shim_url: Option<&str>) -> Self {
        YouTubeMusicClient {
            http: reqwest::Client::builder()
                .timeout(YouTubeResolver::SEARCH_TIMEOUT)
                .no_gzip()
                .no_deflate()
                .build()
                .expect("a plain HTTP client builds"),
            shim_url: shim_url.map(|url| url.trim_end_matches('/').to_string()),
        }
    }

    pub fn configured(&self) -> bool {
        self.shim_url.as_deref().is_some_and(|url| !url.is_empty())
    }

    /// `filter`: "songs" for releases, "videos" for uploads. The rows; `Ok(None)` when the shim
    /// could not answer, so a caller never keeps that as "nothing found". `Err` when the shim is
    /// unreachable, or for an answer that is not its JSON.
    pub async fn search(&self, query: &str, filter: &str) -> anyhow::Result<Option<Vec<YtmRow>>> {
        Ok(self
            .get_rows(&format!(
                "/ytm/search?q={}&filter={filter}&limit=10",
                dotnet::escape_data_string(query)
            ))
            .await?
            .map(|(_, rows)| rows))
    }

    pub async fn radio(&self, video_id: &str, limit: i32) -> anyhow::Result<Vec<YtmRow>> {
        Ok(self
            .get_rows(&format!(
                "/ytm/radio?videoId={}&limit={limit}",
                dotnet::escape_data_string(video_id)
            ))
            .await?
            .map(|(_, rows)| rows)
            .unwrap_or_default())
    }

    /// The radio YouTube Music offers for an artist, found by name, and the name it found.
    pub async fn artist_radio(
        &self,
        artist: &str,
        limit: i32,
    ) -> anyhow::Result<(Option<String>, Vec<YtmRow>)> {
        let answer = self
            .get_rows(&format!(
                "/ytm/artist-radio?q={}&limit={limit}",
                dotnet::escape_data_string(artist)
            ))
            .await?;
        Ok(match answer {
            Some((root, rows)) => {
                let found = match &root {
                    Value::Object(map) => map.get("artist").and_then(Value::as_str).map(str::to_string),
                    _ => None,
                };
                (found, rows)
            }
            None => (None, Vec::new()),
        })
    }

    async fn get_rows(&self, path_and_query: &str) -> anyhow::Result<Option<(Value, Vec<YtmRow>)>> {
        let Some(shim) = self.shim_url.as_deref().filter(|url| !url.is_empty()) else {
            return Ok(None);
        };
        let response = self.http.get(format!("{shim}{path_and_query}")).send().await?;
        if !response.status().is_success() {
            debug!(
                "YouTube Music {} answered {}",
                path_and_query.split('?').next().unwrap_or(""),
                response.status().as_u16()
            );
            return Ok(None);
        }
        let root: Value = serde_json::from_str(&response.text().await?)?;
        let rows = rows(&root)?;
        Ok(Some((root, rows)))
    }
}

/// The `tracks` of an answer, each with a video id and a title.
pub fn rows(root: &Value) -> ElementResult<Vec<YtmRow>> {
    let Some(tracks @ Value::Array(_)) = try_get_property(root, "tracks")? else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for row in enumerate_array(tracks)? {
        let (Some(id), Some(title)) = (text(row, "videoId")?, text(row, "title")?) else {
            continue;
        };
        let artists = match try_get_property(row, "artists")? {
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        };
        let seconds = match try_get_property(row, "durationSeconds")? {
            Some(value @ Value::Number(_)) => Some(get_int32(value)?),
            _ => None,
        };
        rows.push(YtmRow {
            video_id: id,
            title,
            artists,
            album: text(row, "album")?,
            duration_seconds: seconds,
            video_type: text(row, "videoType")?,
        });
    }
    Ok(rows)
}

/// A non-empty string property.
fn text(element: &Value, name: &str) -> ElementResult<Option<String>> {
    Ok(match try_get_property(element, name)? {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Rows without an id or a title are left out, odd artists are skipped, and a fractional
    /// length fails the answer.
    #[test]
    fn rows_are_read_as_the_shim_writes_them() {
        let read = rows(&json!({"tracks": [
            {"videoId": "v1", "title": "One", "artists": ["A", "", 3, "B"], "durationSeconds": 200, "videoType": "MUSIC_VIDEO_TYPE_ATV", "album": "X"},
            {"videoId": "", "title": "No id"},
            {"videoId": "v3", "title": "Three", "durationSeconds": null}
        ]}))
        .expect("reads");
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].artists, ["A", "B"]);
        assert_eq!(read[0].duration_seconds, Some(200));
        assert_eq!(read[0].album.as_deref(), Some("X"));
        assert_eq!(read[1].duration_seconds, None);
        assert!(rows(&json!({"tracks": [{"videoId": "v", "title": "T", "durationSeconds": 1.5}]})).is_err());
        assert!(rows(&json!({"tracks": "none"})).expect("reads").is_empty());
        assert!(rows(&json!([])).is_err());
        assert!(!YouTubeMusicClient::with_shim_url(None).configured());
        assert!(!YouTubeMusicClient::with_shim_url(Some("")).configured());
        assert!(YouTubeMusicClient::with_shim_url(Some("http://shim:8080/")).configured());
    }
}
