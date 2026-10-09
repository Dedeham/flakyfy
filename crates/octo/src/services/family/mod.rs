//! Octo Family's services: the key file and sealed secrets, member passwords, the family
//! stores, Octo's admin calls to Navidrome, the library index that tells shared music from
//! members' own, and what the family gate uses to sign members in: the bridge to their hidden
//! Navidrome passwords, browser tokens, the device registry and the rule hooks.

pub mod caller;
pub mod device_registry;
pub mod family_playlist_store;
pub mod family_playlists;
pub mod family_pool_store;
pub mod family_store;
pub mod kid_filter;
pub mod library_index;
pub mod list_rules;
pub mod member_bridge;
pub mod member_credentials;
pub mod navidrome_admin;
pub mod owner_privacy;
pub mod passwords;
pub mod personal_marks;
pub mod request_store;
pub mod rules;
pub mod saved_store;
mod store_file;
pub mod vault;
pub mod web_session;

pub use caller::{FamilyCaller, family_caller};
pub use device_registry::{DeviceRegistry, NewDevice, Sighting};
pub use family_playlist_store::{FamilyPlaylist, FamilyPlaylistEntry, FamilyPlaylistStore};
pub use family_pool_store::{FamilyPoolStore, PoolFile};
pub use family_store::{FamilyStore, Member, PairOutcome, SharedLibraries};
pub use library_index::{IndexedSong, LibraryIndex, LibraryScope, ScopeResolver};
pub use member_bridge::MemberBridge;
pub use member_credentials::MemberCredentials;
pub use navidrome_admin::{NavidromeAdmin, NdLibrary, NdPlayer, NdUser};
pub use request_store::RequestStore;
pub use rules::{AnswerContext, AnswerRule, AnswerRules, request_rules};
pub use saved_store::{SavedAlbum, SavedSong, SavedStore};
pub use vault::FamilyVault;
