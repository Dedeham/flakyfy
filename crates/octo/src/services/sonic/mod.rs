//! `Services/Sonic`: Sounds alike, octo-sonic's reading of every library song. What is here so
//! far is what the analysis keeps on disk, `sonic-features.json`; the client, the analysis pass
//! and the nearest-song search are group L's.

pub mod sonic_store;

pub use sonic_store::{SonicFailure, SonicSong, SonicState};
