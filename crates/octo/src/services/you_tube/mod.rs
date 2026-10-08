//! `Services/YouTube`: the yt-dlp shim, for search and streams, and YouTube Music through its
//! `/ytm` endpoints.

pub mod you_tube_music_client;
pub mod you_tube_resolver;

pub use you_tube_music_client::{YouTubeMusicClient, YtmRow};
pub use you_tube_resolver::{ShimStream, YouTubeHit, YouTubeResolver};
