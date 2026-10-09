//! Sounds alike, octo-sonic's reading of every library song: the client, the
//! analysis pass, and what the analysis keeps on disk (`sonic-features.json`) with the
//! nearest-song search.

pub mod sonic_analysis_worker;
pub mod sonic_client;
pub mod sonic_store;

pub use sonic_analysis_worker::{
    ISonicLibrary, NavidromeSonicLibrary, SonicAnalysisWorker, SonicFile, SonicStatus,
};
pub use sonic_client::{ISonicClient, SonicAnswer, SonicClient, SonicDropped, SonicFeatures, SonicHealth};
pub use sonic_store::{SonicFailure, SonicSong, SonicState, SonicStore};
