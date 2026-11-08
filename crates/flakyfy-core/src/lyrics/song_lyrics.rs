//! What lyrics a song file has, the pure half: the answer and how text is timed. The half
//! that looks at files (`of`, `may_write_inside`, `may_write_beside`) is
//! `flakyfy::services::lyrics::song_lyrics`.

use super::lyrics_models::LyricsTiming;
use super::lyrics_text::LyricsText;
use crate::common::dotnet::is_null_or_white_space;

/// The first line of every lyrics file Flakyfy writes, and of the lyrics it writes inside a song's
/// tags. Flakyfy replaces only lyrics that start with it.
pub const FLAKYFY_MARK: &str = "[re:Flakyfy]";

/// Where a song's lyrics are: nowhere, in a file beside it, or in its own tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SongLyricsPlace {
    #[default]
    None,
    Beside,
    Inside,
}

/// The lyrics a song file has now, as Navidrome serves them: a .lrc beside it first, then a .txt,
/// then its tags (Navidrome's default LyricsPriority). Flakyfys is whether Flakyfy wrote them, so may
/// replace them. Unknown is a lyrics file in a format Flakyfy does not read, which it leaves alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SongLyrics {
    pub place: SongLyricsPlace,
    pub timing: LyricsTiming,
    pub flakyfys: bool,
    pub unknown: bool,
}

impl SongLyrics {
    /// No lyrics anywhere.
    pub const NOTHING: SongLyrics = SongLyrics {
        place: SongLyricsPlace::None,
        timing: LyricsTiming::None,
        flakyfys: false,
        unknown: false,
    };

    /// Lyrics files Navidrome or another player may read beside a song, besides .lrc and .txt.
    pub const OTHER_EXTENSIONS: [&'static str; 5] = [".ttml", ".elrc", ".srt", ".yaml", ".yml"];

    pub fn new(place: SongLyricsPlace, timing: LyricsTiming, flakyfys: bool, unknown: bool) -> Self {
        Self {
            place,
            timing,
            flakyfys,
            unknown,
        }
    }

    /// How lyrics text is timed: word tags, line tags, or neither.
    pub fn timing_of(text: Option<&str>) -> LyricsTiming {
        if is_null_or_white_space(text) {
            LyricsTiming::None
        } else if LyricsText::has_word_tags(text) {
            LyricsTiming::Word
        } else if LyricsText::has_timestamps(text) {
            LyricsTiming::Line
        } else {
            LyricsTiming::Plain
        }
    }

    /// Whether lyrics text (from a song's tags) is Flakyfy's: its first line, byte order mark and
    /// spaces aside, is [`FLAKYFY_MARK`].
    pub fn is_flakyfys_text(text: &str) -> bool {
        let text = text.trim_start_matches('\u{FEFF}');
        text.split('\n').next().unwrap_or("").trim() == FLAKYFY_MARK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_of_reads_word_tags_then_line_tags_then_text() {
        assert_eq!(SongLyrics::timing_of(None), LyricsTiming::None);
        assert_eq!(SongLyrics::timing_of(Some(" \n ")), LyricsTiming::None);
        assert_eq!(
            SongLyrics::timing_of(Some("[00:01.00]<00:01.00>a")),
            LyricsTiming::Word
        );
        assert_eq!(SongLyrics::timing_of(Some("[00:01.00]a")), LyricsTiming::Line);
        assert_eq!(SongLyrics::timing_of(Some("a")), LyricsTiming::Plain);
    }

    #[test]
    fn flakyfys_text_starts_with_the_mark() {
        assert!(SongLyrics::is_flakyfys_text("[re:Flakyfy]\n[00:01.00]x"));
        assert!(SongLyrics::is_flakyfys_text("\u{FEFF} [re:Flakyfy] \r\nx"));
        assert!(!SongLyrics::is_flakyfys_text("x\n[re:Flakyfy]"));
        assert!(!SongLyrics::is_flakyfys_text(""));
        assert_eq!(SongLyrics::default(), SongLyrics::NOTHING);
    }
}
