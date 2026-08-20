use std::io;
use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, BlockModelError>;

#[derive(Debug, Error)]
pub enum BlockModelError {
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },

    #[error("failed to write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },

    #[error("failed to parse JSON {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[error("failed to walk {path}: {source}")]
    Walk {
        path: PathBuf,
        source: walkdir::Error,
    },

    #[error("{0}")]
    Message(String),
}
