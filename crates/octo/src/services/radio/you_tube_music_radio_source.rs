//! Port of `Services/Radio/YouTubeMusicRadioSource.cs`: YouTube Music's radio for the seed. It
//! knows uploads Last.fm never heard of, nightcore and phonk channels included, which is why it
//! suits a library built from YouTube (#78).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use indexmap::IndexMap;
use octo_core::common::SongIdentity;
use octo_core::common::song_identity::{SongMatchOptions, SongRef};
use octo_core::last_fm::SimilarTrack;
use octo_core::last_fm::last_fm_radio_seed_normalizer as seed_normalizer;
use octo_core::radio::radio_provider::YOU_TUBE_MUSIC;
use octo_core::radio::{RadioAnswer, RadioMatch, RadioSeed};
use octo_core::settings::SettingsStore;

use super::radio_source::IRadioSource;
use crate::services::framework::MemoryCache;
use crate::services::you_tube::you_tube_music_client::{YouTubeMusicClient, YtmRow};

pub const UPLOAD: &str = "MUSIC_VIDEO_TYPE_UGC";
pub const PODCAST: &str = "MUSIC_VIDEO_TYPE_PODCAST_EPISODE";

/// The seed's own video, its kind, and whether it is the same recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedVideo {
    pub video_id: String,
    pub video_type: Option<String>,
    pub exact: bool,
}

pub struct YouTubeMusicRadioSource {
    ytm: Arc<YouTubeMusicClient>,
    /// `IOptionsMonitor<RadioSourceSettings>`.
    settings: Arc<SettingsStore>,
    seeds: MemoryCache<Option<SeedVideo>>,
}

impl YouTubeMusicRadioSource {
    pub fn new(ytm: Arc<YouTubeMusicClient>, settings: Arc<SettingsStore>) -> Self {
        YouTubeMusicRadioSource {
            ytm,
            settings,
            seeds: MemoryCache::new(4000),
        }
    }

    pub async fn seed_video(&self, seed: &RadioSeed) -> anyhow::Result<Option<SeedVideo>> {
        if let Some(video) = seed.you_tube_id.as_deref().filter(|id| !id.is_empty()) {
            return Ok(Some(SeedVideo {
                video_id: video.to_string(),
                video_type: None,
                exact: true,
            }));
        }
        let key = seed_normalizer::track_key(&seed.artist, &seed.title);
        if let Some(cached) = self.seeds.get(&key) {
            return Ok(cached);
        }
        let query = format!("{} {}", seed.artist, seed.title);
        let songs = self.ytm.search(&query, "songs").await?;
        let mut found = songs.as_deref().and_then(|songs| pick(seed, songs));
        let mut videos = None;
        if found.is_none() && songs.is_some() {
            videos = self.ytm.search(&query, "videos").await?;
            found = videos.as_deref().and_then(|videos| pick(seed, videos));
        }
        // A miss is kept only when YouTube Music really answered; a shim that failed is asked
        // again next time.
        if found.is_none() && (songs.is_none() || videos.is_none()) {
            return Ok(None);
        }
        let ttl = if found.is_none() {
            Duration::from_secs(3600)
        } else {
            Duration::from_secs(7 * 24 * 3600)
        };
        self.seeds.set(key, found.clone(), 1, ttl);
        Ok(found)
    }
}

/// The rows as tracks this source suggested.
fn tracks<'a>(rows: impl Iterator<Item = &'a YtmRow>, count: i32) -> Vec<SimilarTrack> {
    rows.map(|row| {
        SimilarTrack::new(row.artists.join(", "), row.title.clone(), 1.0)
            .with_duration(row.duration_seconds)
            .with_you_tube_id(Some(row.video_id.clone()))
            .with_provider(Some(YOU_TUBE_MUSIC.to_string()))
    })
    .take(count.max(0) as usize)
    .collect()
}

/// The same recording first; else the same song in another version, whose radio is still close
/// (the original for a nightcore seed, or the other way round).
pub fn pick(seed: &RadioSeed, rows: &[YtmRow]) -> Option<SeedVideo> {
    let strict = SongMatchOptions {
        length_tolerance_seconds: Some(5),
        ..Default::default()
    };
    let seed_ref =
        SongRef::new(seed.title.clone(), seed.artist.clone()).with_seconds(seed.duration.map(f64::from));
    for row in rows {
        let row_ref = SongRef::new(row.title.clone(), row.artists.join(", "))
            .with_seconds(row.duration_seconds.map(f64::from));
        if SongIdentity::same(&seed_ref, &row_ref, Some(&strict)).is_same() {
            return Some(SeedVideo {
                video_id: row.video_id.clone(),
                video_type: row.video_type.clone(),
                exact: true,
            });
        }
    }
    let core = SongIdentity::parse_title(&seed.title, Some(&seed.artist)).key;
    let artist = SongIdentity::key(&SongIdentity::primary_artist(&seed.artist));
    rows.iter()
        .find(|row| {
            !core.is_empty()
                && SongIdentity::parse_title(&row.title, None).key == core
                && row
                    .artists
                    .iter()
                    .any(|name| SongIdentity::key(&SongIdentity::primary_artist(name)) == artist)
        })
        .map(|row| SeedVideo {
            video_id: row.video_id.clone(),
            video_type: row.video_type.clone(),
            exact: false,
        })
}

#[async_trait]
impl IRadioSource for YouTubeMusicRadioSource {
    fn provider(&self) -> &'static str {
        YOU_TUBE_MUSIC
    }

    fn available(&self) -> bool {
        self.settings.current().radio_sources.you_tube_music && self.ytm.configured()
    }

    async fn songs_like(
        &self,
        seed: &RadioSeed,
        count: i32,
        _auth: &IndexMap<String, String>,
    ) -> anyhow::Result<RadioAnswer> {
        if seed.is_artist() {
            // Only when YouTube Music found this very artist: a search for a name it does not
            // know answers with whoever is closest, which is the #78 mistake again.
            let (artist, rows) = self.ytm.artist_radio(&seed.artist, count.clamp(10, 100)).await?;
            let very_artist = artist.as_deref().is_some_and(|artist| {
                SongIdentity::key(artist) == SongIdentity::key(&SongIdentity::primary_artist(&seed.artist))
            });
            if !very_artist {
                return Ok(RadioAnswer::nothing(YOU_TUBE_MUSIC));
            }
            let found = tracks(
                rows.iter()
                    .filter(|row| row.video_type.as_deref() != Some(PODCAST) && !row.artists.is_empty()),
                count,
            );
            let r#match = if found.is_empty() {
                RadioMatch::None
            } else {
                RadioMatch::ArtistsTrusted
            };
            return Ok(RadioAnswer::new(YOU_TUBE_MUSIC, r#match, found, Vec::new()));
        }
        let Some(found) = self.seed_video(seed).await? else {
            return Ok(RadioAnswer::nothing(YOU_TUBE_MUSIC));
        };
        let rows = self
            .ytm
            .radio(&found.video_id, (count + 1).clamp(10, 100))
            .await?;
        // A radio from a release keeps to releases and official videos; from an upload, uploads
        // are the music (nightcore radio is mostly uploads). Podcast episodes are never music.
        let from_upload = found.video_type.as_deref().is_none_or(|kind| kind == UPLOAD);
        let suggested = tracks(
            rows.iter()
                .filter(|row| row.video_id != found.video_id && row.video_type.as_deref() != Some(PODCAST))
                .filter(|row| from_upload || row.video_type.as_deref() != Some(UPLOAD))
                .filter(|row| !row.artists.is_empty()),
            count,
        );
        let r#match = if found.exact {
            RadioMatch::Song
        } else {
            RadioMatch::Original
        };
        Ok(RadioAnswer::new(YOU_TUBE_MUSIC, r#match, suggested, Vec::new()))
    }
}
