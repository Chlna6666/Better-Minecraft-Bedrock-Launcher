use thiserror::Error;

/// Convenient result alias used by all fallible `bedrock-render` APIs.
pub type Result<T> = std::result::Result<T, BedrockRenderError>;

/// Stable error categories for application-level handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BedrockRenderErrorKind {
    /// The backing `bedrock-world` crate returned an error.
    World,
    /// The backing `bedrock-leveldb` crate returned an error.
    LevelDb,
    /// Image encoding failed.
    Image,
    /// Filesystem or other I/O failed.
    Io,
    /// Rendering observed a caller-provided cancellation flag.
    Cancelled,
    /// The requested mode, format, or feature is not available.
    UnsupportedMode,
    /// Caller input or decoded render data failed validation.
    Validation,
    /// An asynchronous worker task failed.
    Join,
}

/// Error returned by public `bedrock-render` APIs.
#[derive(Debug, Error)]
pub enum BedrockRenderError {
    /// Error propagated from `bedrock-world`.
    #[error("Bedrock world error: {0}")]
    World(#[from] bedrock_world::BedrockWorldError),
    /// Error propagated from `bedrock-leveldb`.
    #[error("Bedrock LevelDB error: {0}")]
    LevelDb(#[from] bedrock_leveldb::LevelDbError),
    /// Error emitted while encoding image bytes.
    #[error("{message}: {source}")]
    Image {
        /// Human-readable operation context.
        message: String,
        /// Original image crate error.
        #[source]
        source: image::ImageError,
    },
    /// Error emitted while reading or writing files.
    #[error("{message}: {source}")]
    Io {
        /// Human-readable operation context.
        message: String,
        /// Original I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Rendering was cancelled by the caller.
    #[error("render was cancelled")]
    Cancelled,
    /// Requested render mode or image format is unavailable in this build.
    #[error("unsupported render mode: {0}")]
    UnsupportedMode(String),
    /// Input options or decoded render data failed validation.
    #[error("validation failed: {0}")]
    Validation(String),
    /// Async worker task failed to complete.
    #[error("async runtime error: {0}")]
    Join(String),
}

impl BedrockRenderError {
    /// Builds an image error with operation context.
    #[must_use]
    pub fn image(message: impl Into<String>, source: image::ImageError) -> Self {
        Self::Image {
            message: message.into(),
            source,
        }
    }

    /// Builds an I/O error with operation context.
    #[must_use]
    pub fn io(message: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            message: message.into(),
            source,
        }
    }

    /// Returns the stable category for this error.
    #[must_use]
    pub const fn kind(&self) -> BedrockRenderErrorKind {
        match self {
            Self::World(_) => BedrockRenderErrorKind::World,
            Self::LevelDb(_) => BedrockRenderErrorKind::LevelDb,
            Self::Image { .. } => BedrockRenderErrorKind::Image,
            Self::Io { .. } => BedrockRenderErrorKind::Io,
            Self::Cancelled => BedrockRenderErrorKind::Cancelled,
            Self::UnsupportedMode(_) => BedrockRenderErrorKind::UnsupportedMode,
            Self::Validation(_) => BedrockRenderErrorKind::Validation,
            Self::Join(_) => BedrockRenderErrorKind::Join,
        }
    }
}
