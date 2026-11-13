//! Errors shared by the binary and text codecs.

/// Codec result.
pub type Result<T> = std::result::Result<T, Error>;

/// An input or value cannot be represented in the requested NBT format.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Input/output failure, including truncated binary input.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed binary NBT.
    #[error("NBT error: {0}")]
    Nbt(String),
    /// A value cannot be serialized safely.
    #[error("validation failed: {0}")]
    Validation(String),
    /// Malformed SNBT, with the byte offset of the failure.
    #[error("SNBT error at byte {offset}: {message}")]
    Snbt {
        /// Byte offset in the input.
        offset: usize,
        /// Reason for failure.
        message: String,
    },
}

impl From<std::string::FromUtf8Error> for Error {
    fn from(error: std::string::FromUtf8Error) -> Self {
        Self::Nbt(error.to_string())
    }
}

impl From<std::str::Utf8Error> for Error {
    fn from(error: std::str::Utf8Error) -> Self {
        Self::Nbt(error.to_string())
    }
}
