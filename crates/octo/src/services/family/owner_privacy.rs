//! The owner's privacy. With `Family:OwnerSeesMemberLibraries` off (the default), the owner's
//! answers leave out the music in members' own libraries: their songs, albums whose every song
//! is a member's, artists whose every song is a member's, and the libraries themselves in
//! `getMusicFolders`. With `Family:OwnerSeesMemberPlaylists` off, the owner's `getPlaylists`
//! leaves out playlists members own.
//!
//! Navidrome shows an admin every library, and `getArtist` and `getAlbum` take no folder, so
//! this works on the answers, from the [`LibraryIndex`]. A song the index does not know yet
//! counts as shared: the owner never loses sight of something just downloaded.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use octo_core::family::abilities::Role;
use octo_core::settings::FamilySettings;
use parking_lot::Mutex;

use super::caller::FamilyCaller;
use super::library_index::LibraryIndex;
use super::list_rules::{NodeKind, listed};

/// The endpoints whose answers to the owner leave out members' own music.
pub const LIBRARY_ENDPOINTS: &[&str] = &[
    "search",
    "search2",
    "search3",
    "getAlbum",
    "getAlbumList",
    "getAlbumList2",
    "getArtist",
    "getArtists",
    "getIndexes",
    "getMusicDirectory",
    "getMusicFolders",
    "getPlaylist",
    "getRandomSongs",
    "getSongsByGenre",
    "getSimilarSongs",
    "getSimilarSongs2",
    "getTopSongs",
    "getStarred",
    "getStarred2",
];

/// Which of the owner's two privacy rules apply to one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerRules {
    /// Leave out members' own music.
    pub libraries: bool,
    /// Leave out members' playlists.
    pub playlists: bool,
}

/// The owner's rules for an answer from `endpoint`, or None when neither applies.
pub fn applies(caller: &FamilyCaller, endpoint: &str, family: &FamilySettings) -> Option<OwnerRules> {
    if caller.managed || caller.role != Role::Owner {
        return None;
    }
    let rules = OwnerRules {
        libraries: !family.owner_sees_member_libraries && listed(LIBRARY_ENDPOINTS, endpoint),
        playlists: !family.owner_sees_member_playlists && endpoint.eq_ignore_ascii_case("getPlaylists"),
    };
    (rules.libraries || rules.playlists).then_some(rules)
}

/// Whether a playlist row is a member's (its owner is a member), given the members' names in
/// lower case. Family playlists are owned by "Family" and stay.
pub fn hides_playlist(members: &HashSet<String>, owner: Option<&str>) -> bool {
    owner.is_some_and(|owner| members.contains(&owner.to_lowercase()))
}

/// What is a member's own, worked out once per read of the index.
#[derive(Debug, Default)]
pub struct PrivateView {
    libraries: HashSet<i64>,
    albums: HashSet<String>,
    artists: HashSet<String>,
}

impl PrivateView {
    /// Reads the index: an album or artist is a member's when every song of it the index knows
    /// is in a member's own library.
    pub fn build(index: &LibraryIndex, personal_libraries: &[i64]) -> Self {
        let libraries: HashSet<i64> = personal_libraries.iter().copied().collect();
        // Songs per album and per artist: (in members' libraries, elsewhere).
        let mut albums: HashMap<String, (u32, u32)> = HashMap::new();
        let mut artists: HashMap<String, (u32, u32)> = HashMap::new();
        index.each_song(|song| {
            let personal = song
                .library_id
                .is_some_and(|library| libraries.contains(&library));
            let tally = |counts: &mut (u32, u32)| {
                if personal {
                    counts.0 += 1;
                } else {
                    counts.1 += 1;
                }
            };
            if !song.album_id.is_empty() {
                tally(albums.entry(song.album_id.clone()).or_default());
            }
            for artist in &song.artist_ids {
                tally(artists.entry(artist.clone()).or_default());
            }
        });
        let own = |counts: HashMap<String, (u32, u32)>| -> HashSet<String> {
            counts
                .into_iter()
                .filter(|(_, (personal, shared))| *personal > 0 && *shared == 0)
                .map(|(id, _)| id)
                .collect()
        };
        PrivateView {
            libraries,
            albums: own(albums),
            artists: own(artists),
        }
    }

    /// Whether a song in `library` is a member's own (an unknown library is shared).
    pub fn hides_song(&self, library: Option<i64>) -> bool {
        library.is_some_and(|library| self.libraries.contains(&library))
    }

    /// Whether a node other than a song is a member's own.
    pub fn hides(&self, kind: NodeKind, id: &str) -> bool {
        match kind {
            NodeKind::Album => self.albums.contains(id),
            NodeKind::Artist => self.artists.contains(id),
            NodeKind::Folder => self.albums.contains(id) || self.artists.contains(id),
            NodeKind::MusicFolder => id
                .trim()
                .parse::<i64>()
                .is_ok_and(|library| self.libraries.contains(&library)),
            NodeKind::Song | NodeKind::Playlist => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewKey {
    refreshed: Option<DateTime<Utc>>,
    songs: usize,
    libraries: Vec<i64>,
}

/// The last [`PrivateView`], kept until the index is read again or the members' libraries
/// change.
#[derive(Default)]
pub struct PrivacyCache {
    cached: Mutex<Option<(ViewKey, Arc<PrivateView>)>>,
}

impl PrivacyCache {
    pub fn view(&self, index: &LibraryIndex, mut personal_libraries: Vec<i64>) -> Arc<PrivateView> {
        personal_libraries.sort_unstable();
        personal_libraries.dedup();
        let key = ViewKey {
            refreshed: index.refreshed_at(),
            songs: index.len(),
            libraries: personal_libraries,
        };
        let mut cached = self.cached.lock();
        if let Some((held, view)) = cached.as_ref()
            && *held == key
        {
            return Arc::clone(view);
        }
        let view = Arc::new(PrivateView::build(index, &key.libraries));
        *cached = Some((key, Arc::clone(&view)));
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octo_core::family::abilities::{Abilities, Preset};

    #[test]
    fn only_the_owner_gets_privacy() {
        let family = FamilySettings::default();
        let owner = FamilyCaller::unmanaged("winters", true);
        let other = FamilyCaller::unmanaged("guest", false);
        let member = FamilyCaller::member("alex", Preset::CoAdmin, Abilities::preset(Preset::CoAdmin));
        let rules = applies(&owner, "getArtist", &family);
        assert_eq!(
            rules,
            (!family.owner_sees_member_libraries).then_some(OwnerRules {
                libraries: true,
                playlists: false
            })
        );
        assert!(applies(&other, "getArtist", &family).is_none());
        assert!(applies(&member, "getArtist", &family).is_none());
        assert!(applies(&owner, "getNowPlaying", &family).is_none());
        let sees_all = FamilySettings {
            owner_sees_member_libraries: true,
            owner_sees_member_playlists: true,
            ..FamilySettings::default()
        };
        assert!(applies(&owner, "getArtist", &sees_all).is_none());
        assert!(applies(&owner, "getPlaylists", &sees_all).is_none());
    }

    #[test]
    fn member_playlists_are_told_by_owner() {
        let members: HashSet<String> = ["alex".to_string()].into();
        assert!(hides_playlist(&members, Some("Alex")));
        assert!(!hides_playlist(&members, Some("winters")));
        assert!(!hides_playlist(&members, Some("Family")));
        assert!(!hides_playlist(&members, None));
    }

    #[test]
    fn music_folders_are_libraries() {
        let view = PrivateView {
            libraries: [3].into(),
            ..PrivateView::default()
        };
        assert!(view.hides(NodeKind::MusicFolder, "3"));
        assert!(!view.hides(NodeKind::MusicFolder, "1"));
        assert!(view.hides_song(Some(3)));
        assert!(!view.hides_song(None), "an unknown song counts as shared");
    }
}
