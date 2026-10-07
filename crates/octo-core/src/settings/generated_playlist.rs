//! `Octo.Models.Settings.GeneratedPlaylistSettings` (the `GeneratedPlaylists` section).

use serde::{Deserialize, Serialize};

/// Genre and decade mixes Octo builds from the library and serves like radio stations: per
/// listener, read-only, drawn again on a schedule (#54). Nothing is written to Navidrome, so a
/// rescan cannot empty them and a listener cannot edit one by accident.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct GeneratedPlaylistSettings {
    /// Environment variable: MIXES_ENABLED
    pub enabled: bool,

    /// A mix per genre. Environment variable: MIXES_GENRES
    pub genres: bool,

    /// A mix per decade. Environment variable: MIXES_DECADES
    pub decades: bool,

    /// Tracks per mix. Environment variable: MIX_TRACK_COUNT
    pub track_count: i32,

    /// At most this many tracks by one artist in a mix: without it a lopsided genre becomes half
    /// an album. Environment variable: MIX_MAX_PER_ARTIST
    pub max_per_artist: i32,

    /// A mix appears once its genre or decade has this many tracks.
    /// Environment variable: MIX_CREATE_AT
    pub create_at: i32,

    /// ...and goes only below this many, so one at the boundary does not come and go.
    /// Environment variable: MIX_REMOVE_BELOW
    pub remove_below: i32,

    /// Most mixes shown, largest first. Environment variable: MIX_MAX_PLAYLISTS
    pub max_playlists: i32,

    /// How long one draw lasts before the mixes are drawn again.
    /// Environment variable: MIX_REFRESH_HOURS
    pub refresh_hours: i32,

    /// Percent of each mix, and of Discovery Mix, kept for tracks new to the listener: never
    /// played, or added in the last NewDays. 0 draws from everything alike, so the default changes
    /// nothing. Environment variable: MIX_NEW_SHARE
    pub new_share: i32,

    /// How recently added a track still counts as new. Environment variable: MIX_NEW_DAYS
    pub new_days: i32,

    /// How a mix is named; "{0}" is the genre or decade. Empty means "{0} Mix".
    /// Environment variable: MIX_NAME_FORMAT
    pub name_format: String,

    // ---- Made for you: three lists per listener, on whether or not the genre and decade mixes
    // are. Each is rebuilt once a RefreshHours period from that listener's own plays.
    /// New releases from the listener's most-played artists, from Deezer.
    /// Environment variable: FOR_YOU_NEW_RELEASES
    pub new_releases: bool,

    /// Songs the listener loved and has not played for RediscoverMonths.
    /// Environment variable: FOR_YOU_REDISCOVER
    pub rediscover: bool,

    /// Songs played at most once, from the listener's most-played artists.
    /// Environment variable: FOR_YOU_DEEP_CUTS
    pub deep_cuts: bool,

    /// How far back New Releases looks. Environment variable: NEW_RELEASE_WEEKS
    pub new_release_weeks: i32,

    /// How many of the listener's most-played artists New Releases follows.
    /// Environment variable: NEW_RELEASE_ARTISTS
    pub new_release_artists: i32,

    /// How long since a loved song was last played before Rediscover brings it back.
    /// Environment variable: REDISCOVER_MONTHS
    pub rediscover_months: i32,

    /// "Popular right now": the chart of the moment as a read-only playlist for every listener,
    /// in every app, on whether or not the mixes are. Songs a listener has are their library's
    /// own; the rest play right away and are added with a heart. Made again every few hours.
    /// Environment variable: POPULAR_NOW
    pub popular_now: bool,
}

impl Default for GeneratedPlaylistSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            genres: true,
            decades: true,
            track_count: 100,
            max_per_artist: 3,
            create_at: 20,
            remove_below: 10,
            max_playlists: 20,
            refresh_hours: 24,
            new_share: 0,
            new_days: 30,
            name_format: "{0} Mix".to_string(),
            new_releases: true,
            rediscover: true,
            deep_cuts: true,
            new_release_weeks: 8,
            new_release_artists: 50,
            rediscover_months: 6,
            popular_now: true,
        }
    }
}

impl GeneratedPlaylistSettings {
    pub fn effective_new_release_weeks(&self) -> i32 {
        self.new_release_weeks.clamp(1, 26)
    }

    pub fn effective_new_release_artists(&self) -> i32 {
        self.new_release_artists.clamp(10, 200)
    }

    pub fn effective_rediscover_months(&self) -> i32 {
        self.rediscover_months.clamp(3, 24)
    }

    /// Whether any of the three Made for you lists is on.
    pub fn any_for_you(&self) -> bool {
        self.new_releases || self.rediscover || self.deep_cuts
    }

    pub fn effective_track_count(&self) -> i32 {
        self.track_count.clamp(10, 500)
    }

    pub fn effective_max_per_artist(&self) -> i32 {
        self.max_per_artist.clamp(1, 50)
    }

    pub fn effective_create_at(&self) -> i32 {
        self.create_at.clamp(1, 10_000)
    }

    pub fn effective_remove_below(&self) -> i32 {
        self.remove_below.clamp(0, self.effective_create_at())
    }

    pub fn effective_max_playlists(&self) -> i32 {
        self.max_playlists.clamp(1, 100)
    }

    pub fn effective_refresh_hours(&self) -> i32 {
        self.refresh_hours.clamp(1, 24 * 14)
    }

    pub fn effective_new_share(&self) -> i32 {
        self.new_share.clamp(0, 100)
    }

    pub fn effective_new_days(&self) -> i32 {
        self.new_days.clamp(1, 3650)
    }

    /// A format that is not a format (a stray brace) names the mix the default way
    /// rather than failing the whole playlist list.
    pub fn name(&self, label: &str) -> String {
        if self.name_format.contains("{0}")
            && let Some(name) = dotnet_format_one(&self.name_format, label)
        {
            return name.trim().to_string();
        }
        format!("{label} Mix")
    }
}

/// `string.Format(CultureInfo.InvariantCulture, format, arg)` with one string argument, as
/// .NET 9 parses a composite format: `{{` and `}}` escapes, `{0}`, an alignment
/// (`{0,-10}`, padding with spaces) and a format specifier (`{0:x}`, which a string ignores).
/// None where .NET throws a FormatException: a stray brace, a malformed item, or an index
/// other than 0.
fn dotnet_format_one(format: &str, arg: &str) -> Option<String> {
    // The largest index and width .NET accepts before it calls the format malformed.
    const LIMIT: usize = 1_000_000;

    let chars: Vec<char> = format.chars().collect();
    let mut out = String::new();
    let mut pos = 0;
    while pos < chars.len() {
        let ch = chars[pos];
        pos += 1;
        match ch {
            '}' => {
                if chars.get(pos) == Some(&'}') {
                    out.push('}');
                    pos += 1;
                } else {
                    return None;
                }
            }
            '{' => {
                if chars.get(pos) == Some(&'{') {
                    out.push('{');
                    pos += 1;
                    continue;
                }
                // The index: at least one digit.
                let mut index = chars.get(pos)?.to_digit(10)? as usize;
                pos += 1;
                while let Some(d) = chars.get(pos).and_then(|c| c.to_digit(10)) {
                    index = index * 10 + d as usize;
                    if index >= LIMIT {
                        return None;
                    }
                    pos += 1;
                }
                while chars.get(pos) == Some(&' ') {
                    pos += 1;
                }
                // The alignment: a comma, optional spaces, an optional minus, digits.
                let mut width = 0usize;
                let mut left_justify = false;
                if chars.get(pos) == Some(&',') {
                    pos += 1;
                    while chars.get(pos) == Some(&' ') {
                        pos += 1;
                    }
                    if chars.get(pos) == Some(&'-') {
                        left_justify = true;
                        pos += 1;
                    }
                    width = chars.get(pos)?.to_digit(10)? as usize;
                    pos += 1;
                    while let Some(d) = chars.get(pos).and_then(|c| c.to_digit(10)) {
                        width = width * 10 + d as usize;
                        if width >= LIMIT {
                            return None;
                        }
                        pos += 1;
                    }
                    while chars.get(pos) == Some(&' ') {
                        pos += 1;
                    }
                }
                // The format specifier runs to the closing brace and may not hold an
                // opening one. A string argument ignores it.
                match chars.get(pos)? {
                    '}' => {}
                    ':' => {
                        pos += 1;
                        loop {
                            match chars.get(pos)? {
                                '}' => break,
                                '{' => return None,
                                _ => pos += 1,
                            }
                        }
                    }
                    _ => return None,
                }
                pos += 1; // the closing brace
                if index != 0 {
                    return None;
                }
                let len = arg.encode_utf16().count();
                let pad = " ".repeat(width.saturating_sub(len));
                if left_justify {
                    out.push_str(arg);
                    out.push_str(&pad);
                } else {
                    out.push_str(&pad);
                    out.push_str(arg);
                }
            }
            c => out.push(c),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // GeneratedPlaylistTests.Name_FollowsTheFormat_AndABrokenFormatFallsBack
    #[test]
    fn name_follows_the_format_and_a_broken_format_falls_back() {
        for (format, expected) in [
            ("{0} Mix", "Rock Mix"),
            ("Mix: {0}", "Mix: Rock"),
            ("", "Rock Mix"),
            ("No placeholder", "Rock Mix"),
            ("{0} {1} Mix", "Rock Mix"),
        ] {
            let settings = GeneratedPlaylistSettings {
                name_format: format.into(),
                ..Default::default()
            };
            assert_eq!(settings.name("Rock"), expected, "{format:?}");
        }
    }

    #[test]
    fn composite_format_handles_escapes_alignment_and_specifiers() {
        let cases = [
            ("{{{0}}}", Some("{Rock}")),
            ("[{0,6}]", Some("[  Rock]")),
            ("[{0,-6}]", Some("[Rock  ]")),
            ("[{0 , 6 }]", Some("[  Rock]")),
            ("{0:yyyy}", Some("Rock")),
            ("{0", None),
            ("{0} }", None),
            ("{ 0}", None),
            ("{0:{x}", None),
            ("{1}", None),
        ];
        for (format, expected) in cases {
            assert_eq!(
                dotnet_format_one(format, "Rock").as_deref(),
                expected,
                "{format:?}"
            );
        }
    }

    #[test]
    fn effective_values_are_clamped() {
        let s = GeneratedPlaylistSettings {
            create_at: 5,
            remove_below: 50,
            refresh_hours: 1000,
            ..Default::default()
        };
        assert_eq!(s.effective_remove_below(), 5);
        assert_eq!(s.effective_refresh_hours(), 336);
    }

    #[test]
    fn made_for_you_values_are_clamped_and_on_by_default() {
        let defaults = GeneratedPlaylistSettings::default();
        assert!(defaults.any_for_you() && defaults.popular_now);
        assert_eq!(
            (
                defaults.effective_new_release_weeks(),
                defaults.effective_new_release_artists(),
                defaults.effective_rediscover_months()
            ),
            (8, 50, 6)
        );
        let s = GeneratedPlaylistSettings {
            new_release_weeks: 0,
            new_release_artists: 1000,
            rediscover_months: 1,
            new_releases: false,
            rediscover: false,
            deep_cuts: false,
            ..Default::default()
        };
        assert_eq!(s.effective_new_release_weeks(), 1);
        assert_eq!(s.effective_new_release_artists(), 200);
        assert_eq!(s.effective_rediscover_months(), 3);
        assert!(!s.any_for_you());
    }
}
