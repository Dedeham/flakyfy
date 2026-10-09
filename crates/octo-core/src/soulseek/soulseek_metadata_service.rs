//! The routing model declared at the bottom of `Services/Soulseek/SoulseekMetadataService.cs`:
//! [`RoutingKind`] and [`SoulseekRouting`], which `external-ids.json` carries.
//!
//! These two types are complete ports (checked against the `external-ids.json` fixture). The
//! service itself is task 4-A's and lives in `octo::services::soulseek::soulseek_metadata_service`.

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::soulseek::song_length::LengthSource;
use crate::soulseek::stream_facts::StreamFacts;

/// What a short external id stands for. Written as its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize_repr, Deserialize_repr)]
#[repr(i32)]
pub enum RoutingKind {
    #[default]
    Song = 0,
    Album = 1,
    Artist = 2,
}

/// Where an outside song, album or artist came from, enough to find it again: what the short
/// external ids Octo hands to Subsonic clients stand for.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct SoulseekRouting {
    pub kind: RoutingKind,
    pub you_tube_id: Option<String>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub album: Option<String>,
    pub duration: Option<i32>,

    /// Deezer album id, when an album search resolved one. Absent on album
    /// routings minted from a song row, which fall back to a name lookup.
    pub external_album_id: Option<String>,

    /// Deezer artist id behind an artist routing, once an artist search or an artist
    /// page settled on one. The id itself is minted from the name alone, and two artists can
    /// share a name, so this is what says which of them the page lists. Not part of the id.
    pub external_artist_id: Option<String>,

    /// Position within its album. Carried so a track downloaded as part of an
    /// album keeps its ordering: the download path rebuilds the song from its id alone,
    /// and without this every track lands untracked and sorts alphabetically.
    pub track: Option<i32>,

    /// Disc within a multi-disc release. Same reasoning as [`SoulseekRouting::track`].
    pub disc_number: Option<i32>,

    /// Track count of the album this came from. Without it the tagger fills the
    /// "x of y" denominator from a per-track Deezer search that can match a different
    /// release, producing nonsense like 5/10 on an 8-track album.
    pub total_tracks: Option<i32>,

    /// The track's ISRC, when the album listing that minted this routing named one.
    /// Carried for the same reason as [`SoulseekRouting::track`], and because it is the
    /// strongest evidence download verification can be given about which recording was asked
    /// for. Not part of the id, so routings minted before it existed keep their ids.
    pub isrc: Option<String>,

    /// The length shown for this song once a lookup found one, kept here so every
    /// later response carries it, across restarts too. Display only: see
    /// [`crate::soulseek::SongLength`] for why this is not [`SoulseekRouting::duration`].
    pub shown_duration: Option<i32>,

    /// Where [`SoulseekRouting::shown_duration`] came from, so a weaker source never
    /// replaces a stronger one.
    pub shown_duration_source: LengthSource,

    /// Whether the words are explicit (1), the clean edit (3) or neither (0), as the catalog
    /// said, so every later response for this id carries it. Display only, not part of the id
    /// (`Octo.Models.Domain.ExplicitStatus`).
    pub explicit_content: Option<i32>,

    /// "clean" for a clean edit, which gets an id of its own so it can be listed beside the
    /// explicit original. None for every other song or album, so every id minted before this
    /// existed (stars, playlists, pins) stays the same.
    pub version: Option<String>,

    /// The exact byte count of the pinned video's audio stream, once an answer named it. It
    /// belongs to [`SoulseekRouting::you_tube_id`]: pinning another video clears it. Not part of
    /// the id.
    pub stream_size: Option<u64>,

    /// The pinned video's audio format id (140 is AAC in mp4, 251 Opus in webm).
    pub stream_itag: Option<String>,

    /// The pinned video's audio container, as yt-dlp names it ("m4a", "webm").
    pub stream_ext: Option<String>,

    /// The pinned video's average audio bit rate in kbps.
    pub stream_bit_rate: Option<i32>,

    /// ReplayGain track gain in dB of the pinned video's audio, once a play measured it.
    pub track_gain: Option<f64>,

    /// ReplayGain track peak (linear) of the pinned video's audio, measured with the gain.
    pub track_peak: Option<f64>,
}

/// The gain and peak are plain measured numbers, never NaN, so equality is total.
impl Eq for SoulseekRouting {}

impl SoulseekRouting {
    pub fn has_you_tube(&self) -> bool {
        self.you_tube_id.as_deref().is_some_and(|id| !id.is_empty())
    }

    /// What is known of the pinned video's stream.
    pub fn stream_facts(&self) -> StreamFacts {
        StreamFacts {
            size: self.stream_size,
            itag: self.stream_itag.clone(),
            ext: self.stream_ext.clone(),
            bit_rate: self.stream_bit_rate,
            track_gain: self.track_gain,
            track_peak: self.track_peak,
        }
    }

    /// Takes what `facts` knows of the pinned video's stream, keeping what it does not.
    pub fn merge_stream_facts(&mut self, facts: &StreamFacts) {
        let mut known = self.stream_facts();
        known.merge(facts);
        self.stream_size = known.size;
        self.stream_itag = known.itag;
        self.stream_ext = known.ext;
        self.stream_bit_rate = known.bit_rate;
        self.track_gain = known.track_gain;
        self.track_peak = known.track_peak;
    }

    /// Pins `video` for playback. A video other than the pinned one forgets the old one's
    /// stream facts, which describe a different stream. True when the pin changed.
    pub fn pin_video(&mut self, video: &str) -> bool {
        if self.you_tube_id.as_deref() == Some(video) {
            return false;
        }
        self.you_tube_id = Some(video.to_string());
        self.stream_size = None;
        self.stream_itag = None;
        self.stream_ext = None;
        self.stream_bit_rate = None;
        self.track_gain = None;
        self.track_peak = None;
        true
    }

    pub fn has_artist_title(&self) -> bool {
        self.artist.as_deref().is_some_and(|a| !a.is_empty())
            && self.title.as_deref().is_some_and(|t| !t.is_empty())
    }
}

/// By hand, because System.Text.Json also wrote the computed `HasYouTube` and `HasArtistTitle`
/// (ignored again on read). The stream facts are written only when known, so a routing without
/// them is written exactly as before.
impl Serialize for SoulseekRouting {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let stream_fields = [
            self.stream_size.is_some(),
            self.stream_itag.is_some(),
            self.stream_ext.is_some(),
            self.stream_bit_rate.is_some(),
            self.track_gain.is_some(),
            self.track_peak.is_some(),
        ]
        .into_iter()
        .filter(|known| *known)
        .count();
        let mut s = serializer.serialize_struct("SoulseekRouting", 18 + stream_fields)?;
        s.serialize_field("Kind", &self.kind)?;
        s.serialize_field("YouTubeId", &self.you_tube_id)?;
        s.serialize_field("Artist", &self.artist)?;
        s.serialize_field("Title", &self.title)?;
        s.serialize_field("Album", &self.album)?;
        s.serialize_field("Duration", &self.duration)?;
        s.serialize_field("ExternalAlbumId", &self.external_album_id)?;
        s.serialize_field("ExternalArtistId", &self.external_artist_id)?;
        s.serialize_field("Track", &self.track)?;
        s.serialize_field("DiscNumber", &self.disc_number)?;
        s.serialize_field("TotalTracks", &self.total_tracks)?;
        s.serialize_field("Isrc", &self.isrc)?;
        s.serialize_field("ShownDuration", &self.shown_duration)?;
        s.serialize_field("ShownDurationSource", &self.shown_duration_source)?;
        s.serialize_field("ExplicitContent", &self.explicit_content)?;
        s.serialize_field("Version", &self.version)?;
        if let Some(size) = &self.stream_size {
            s.serialize_field("StreamSize", size)?;
        }
        if let Some(itag) = &self.stream_itag {
            s.serialize_field("StreamItag", itag)?;
        }
        if let Some(ext) = &self.stream_ext {
            s.serialize_field("StreamExt", ext)?;
        }
        if let Some(bit_rate) = &self.stream_bit_rate {
            s.serialize_field("StreamBitRate", bit_rate)?;
        }
        if let Some(gain) = &self.track_gain {
            s.serialize_field("TrackGain", gain)?;
        }
        if let Some(peak) = &self.track_peak {
            s.serialize_field("TrackPeak", peak)?;
        }
        s.serialize_field("HasYouTube", &self.has_you_tube())?;
        s.serialize_field("HasArtistTitle", &self.has_artist_title())?;
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_computed_flags_are_written_and_ignored_on_read() {
        let routing = SoulseekRouting {
            you_tube_id: Some("abc".into()),
            artist: Some("A".into()),
            title: Some(String::new()),
            ..Default::default()
        };
        let json = crate::json::to_string(&routing);
        assert!(
            json.ends_with(
                r#""ShownDurationSource":0,"ExplicitContent":null,"Version":null,"HasYouTube":true,"HasArtistTitle":false}"#
            ),
            "{json}"
        );
        let back: SoulseekRouting = serde_json::from_str(&json).expect("reads back");
        assert_eq!(back, routing);
    }

    #[test]
    fn stream_facts_round_trip_and_a_routing_written_before_them_still_reads() {
        let routing = SoulseekRouting {
            you_tube_id: Some("abc".into()),
            stream_size: Some(6_755_585),
            stream_itag: Some("140".into()),
            stream_ext: Some("m4a".into()),
            stream_bit_rate: Some(129),
            ..Default::default()
        };
        let json = crate::json::to_string(&routing);
        assert!(
            json.contains(r#""StreamSize":6755585,"StreamItag":"140","StreamExt":"m4a","StreamBitRate":129"#),
            "{json}"
        );
        assert_eq!(
            serde_json::from_str::<SoulseekRouting>(&json).expect("reads"),
            routing
        );
        let old: SoulseekRouting = serde_json::from_str(r#"{"Kind":0,"YouTubeId":"abc"}"#).expect("reads");
        assert_eq!(old.stream_facts(), StreamFacts::default());
    }

    #[test]
    fn pinning_another_video_forgets_the_old_streams_facts() {
        let mut routing = SoulseekRouting::default();
        assert!(routing.pin_video("a"));
        routing.merge_stream_facts(&StreamFacts {
            size: Some(10),
            itag: Some("140".into()),
            ..Default::default()
        });
        assert!(!routing.pin_video("a"));
        assert_eq!(routing.stream_size, Some(10));
        assert!(routing.pin_video("b"));
        assert_eq!(routing.you_tube_id.as_deref(), Some("b"));
        assert_eq!(routing.stream_facts(), StreamFacts::default());
    }
}
