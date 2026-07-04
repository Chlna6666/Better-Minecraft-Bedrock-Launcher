mod cover;
mod library;
pub mod service;
pub mod types;

pub use library::MusicTrack;
pub use service::{CoverDecodeRequest, MusicController, MusicPersistedState};
pub use types::{DecodedCoverImage, MusicPlaybackMode, MusicPlaybackSnapshot};
