//! Octo Family's services: the key file and sealed secrets, member passwords, the family
//! stores, Octo's admin calls to Navidrome, and the library index that tells shared music from
//! members' own.

pub mod family_playlist_store;
pub mod family_pool_store;
pub mod family_store;
pub mod passwords;
pub mod request_store;
pub mod saved_store;
mod store_file;
pub mod vault;

pub use family_playlist_store::{FamilyPlaylist, FamilyPlaylistEntry, FamilyPlaylistStore};
pub use family_pool_store::{FamilyPoolStore, PoolFile};
pub use family_store::{FamilyStore, Member, PairOutcome, SharedLibraries};
pub use request_store::RequestStore;
pub use saved_store::{SavedAlbum, SavedSong, SavedStore};
pub use vault::FamilyVault;
