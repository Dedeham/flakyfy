//! Octo Family's services: the key file and sealed secrets, member passwords, the family
//! stores, Octo's admin calls to Navidrome, the library index that tells shared music from
//! members' own, and what the family gate uses to sign members in: the bridge to their hidden
//! Navidrome passwords, browser tokens, the device registry and the rule hooks.

pub mod caller;
pub mod device_registry;
pub mod family_playlist_store;
pub mod family_pool_store;
pub mod family_store;
pub mod library_index;
pub mod library_write_gate;
pub mod member_bridge;
pub mod member_credentials;
pub mod navidrome_admin;
pub mod passwords;
pub mod personal_library;
pub mod pool;
pub mod request_service;
pub mod request_store;
pub mod rules;
pub mod saved_store;
mod store_file;
#[cfg(test)]
pub(crate) mod test_support;
pub mod vault;
pub mod web_session;

pub use caller::{FamilyCaller, family_caller};
pub use device_registry::{DeviceRegistry, NewDevice, Sighting};
pub use family_playlist_store::{FamilyPlaylist, FamilyPlaylistEntry, FamilyPlaylistStore};
pub use family_pool_store::{FamilyPoolStore, PoolFile};
pub use family_store::{FamilyStore, Member, PairOutcome, SharedLibraries};
pub use library_index::{IndexedSong, LibraryIndex, LibraryScope, ScopeResolver};
pub use library_write_gate::{HeartOutcome, LibraryWriteGate, WriteDecision};
pub use member_bridge::MemberBridge;
pub use member_credentials::MemberCredentials;
pub use navidrome_admin::{NavidromeAdmin, NdLibrary, NdPlayer, NdUser};
pub use personal_library::{LibraryDestination, LibraryError, PersonalLibrary};
pub use pool::{
    FamilyPlacement, FamilyPool, LinkMode, LinkStatus, MemberStars, NavidromeMemberStars, PoolParts,
};
pub use request_service::{FamilyEvent, FamilyEvents, RequestError, RequestService};
pub use request_store::RequestStore;
pub use rules::{AnswerContext, AnswerRule, AnswerRules, request_rules};
pub use saved_store::{SavedAlbum, SavedSong, SavedStore};
pub use vault::FamilyVault;
