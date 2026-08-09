mod cover;
mod cover_cache;
mod library;
pub mod service;
pub mod types;
mod watcher;

pub use library::MusicTrack;
pub use service::{CoverDecodeRequest, MusicController, MusicPersistedState};
pub use types::{DecodedCoverImage, MusicPlaybackMode, MusicPlaybackSnapshot};
pub(crate) use watcher::library_changes;
