//! Error and result types for the public API.

use thiserror::Error;

/// Crate-wide result type returned by `bedrock-world` APIs.
pub type Result<T> = std::result::Result<T, BedrockWorldError>;

/// Stable high-level category for a [`BedrockWorldError`].
///
/// Prefer matching this enum in application code instead of parsing the
/// human-readable [`std::fmt::Display`] output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BedrockWorldErrorKind {
    /// Filesystem or operating-system I/O failed.
    Io,
    /// NBT bytes were malformed or could not be represented.
    Nbt,
    /// The configured `LevelDB` backend returned an error.
    LevelDb,
    /// A Bedrock `LevelDB` key could not be decoded as the requested key shape.
    InvalidKey,
    /// A chunk, subchunk, or palette format is not supported by this crate.
    UnsupportedChunkFormat,
    /// Caller-supplied input failed validation before touching storage.
    Validation,
    /// A mutating operation was rejected because the handle is read-only.
    ReadOnly,
    /// A cooperative cancellation flag interrupted a scan.
    Cancelled,
    /// A concurrent storage operation failed because a lock was poisoned.
    ConcurrentWrite,
    /// World data was truncated, internally inconsistent, or otherwise corrupt.
    CorruptWorld,
    /// An async wrapper failed to join its blocking task.
    Join,
}

/// Errors returned while reading, parsing, scanning, or editing a Bedrock world.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum BedrockWorldError {
    /// Filesystem or OS I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// NBT parse, serialization, or validation failure.
    #[error("NBT error: {0}")]
    Nbt(String),
    /// `LevelDB` backend failure.
    #[error("LevelDB error: {0}")]
    LevelDb(String),
    /// Invalid Bedrock database key.
    #[error("invalid Bedrock key: {0}")]
    InvalidKey(String),
    /// Unsupported chunk or subchunk payload.
    #[error("unsupported chunk format: {0}")]
    UnsupportedChunkFormat(String),
    /// Invalid caller-supplied input.
    #[error("validation failed: {0}")]
    Validation(String),
    /// Mutating operation attempted through a read-only world handle.
    #[error("world is read-only")]
    ReadOnly,
    /// Long-running operation cancelled by the caller.
    #[error("{operation} was cancelled")]
    Cancelled {
        /// Operation that observed the cancellation flag.
        operation: &'static str,
    },
    /// Concurrent write or lock poisoning failure.
    #[error("concurrent write rejected: {0}")]
    ConcurrentWrite(String),
    /// Corrupt or inconsistent world data.
    #[error("corrupt world: {0}")]
    CorruptWorld(String),
    /// Async runtime join failure.
    #[error("async runtime error: {0}")]
    Join(String),
}

impl BedrockWorldError {
    /// Returns the stable category for this error.
    #[must_use]
    pub const fn kind(&self) -> BedrockWorldErrorKind {
        match self {
            Self::Io(_) => BedrockWorldErrorKind::Io,
            Self::Nbt(_) => BedrockWorldErrorKind::Nbt,
            Self::LevelDb(_) => BedrockWorldErrorKind::LevelDb,
            Self::InvalidKey(_) => BedrockWorldErrorKind::InvalidKey,
            Self::UnsupportedChunkFormat(_) => BedrockWorldErrorKind::UnsupportedChunkFormat,
            Self::Validation(_) => BedrockWorldErrorKind::Validation,
            Self::ReadOnly => BedrockWorldErrorKind::ReadOnly,
            Self::Cancelled { .. } => BedrockWorldErrorKind::Cancelled,
            Self::ConcurrentWrite(_) => BedrockWorldErrorKind::ConcurrentWrite,
            Self::CorruptWorld(_) => BedrockWorldErrorKind::CorruptWorld,
            Self::Join(_) => BedrockWorldErrorKind::Join,
        }
    }
}

impl From<std::string::FromUtf8Error> for BedrockWorldError {
    fn from(error: std::string::FromUtf8Error) -> Self {
        Self::Nbt(error.to_string())
    }
}

impl From<std::str::Utf8Error> for BedrockWorldError {
    fn from(error: std::str::Utf8Error) -> Self {
        Self::Nbt(error.to_string())
    }
}
