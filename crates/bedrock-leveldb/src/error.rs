use std::path::{Path, PathBuf};
use thiserror::Error;

/// Crate-wide result type.
pub type Result<T> = std::result::Result<T, LevelDbError>;

/// Stable category for a [`LevelDbError`].
///
/// Prefer matching this enum or [`LevelDbError::path`] in application code
/// instead of parsing the human-readable [`std::fmt::Display`] output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Filesystem or operating-system I/O failed.
    Io,
    /// On-disk bytes were malformed, truncated, or failed validation.
    Corruption,
    /// Caller-supplied options or inputs were invalid.
    InvalidArgument,
    /// A requested codec or behavior is disabled or not implemented.
    Unsupported,
    /// Compression or decompression failed.
    Compression,
    /// A scan observed a caller-supplied cancellation flag.
    Cancelled,
    /// A mutating operation was requested from a read-only handle.
    ReadOnly,
    /// Opening failed because the target already existed.
    AlreadyExists,
    /// A database directory or required metadata file was missing.
    NotFound,
    /// An internal synchronization primitive was poisoned.
    LockPoisoned,
    /// The optional async wrapper failed to join a blocking task.
    Join,
}

/// Errors returned while opening, reading, writing, or repairing a database.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LevelDbError {
    /// Filesystem or OS I/O failure.
    #[error("I/O error while {operation}{}: {source}", path_suffix(path.as_deref()))]
    Io {
        /// Operation being performed when the error occurred.
        operation: &'static str,
        /// Filesystem path involved in the operation, when known.
        path: Option<PathBuf>,
        /// Underlying operating-system error.
        #[source]
        source: std::io::Error,
    },
    /// On-disk data was malformed or failed validation.
    #[error("corrupt database{}: {message}", path_suffix(path.as_deref()))]
    Corruption {
        /// Path to the file whose bytes failed validation, when known.
        path: Option<PathBuf>,
        /// Human-readable corruption reason.
        message: String,
    },
    /// Caller supplied invalid options or input.
    #[error("invalid argument: {message}")]
    InvalidArgument {
        /// Human-readable validation failure.
        message: String,
    },
    /// The requested feature is disabled or not implemented.
    #[error("unsupported feature {feature}: {message}")]
    Unsupported {
        /// Feature, codec, or behavior that is unavailable.
        feature: &'static str,
        /// Human-readable explanation.
        message: String,
    },
    /// Compression or decompression failed.
    #[error("compression error in {codec}: {message}")]
    Compression {
        /// Codec or table compression family being processed.
        codec: &'static str,
        /// Human-readable codec error.
        message: String,
    },
    /// Scan cancelled through [`crate::ScanCancelFlag`].
    #[error("scan was cancelled")]
    Cancelled,
    /// A mutating operation was requested on a read-only database.
    #[error("database is read-only")]
    ReadOnly,
    /// `OpenOptions::error_if_exists` rejected an existing database.
    #[error("database already exists: {}", path.display())]
    AlreadyExists {
        /// Existing database directory.
        path: PathBuf,
    },
    /// The requested database or required metadata file was missing.
    #[error("database not found: {}", path.display())]
    NotFound {
        /// Missing database directory or metadata file.
        path: PathBuf,
    },
    /// An internal lock was poisoned.
    #[error("lock poisoned while {operation}")]
    LockPoisoned {
        /// Locking operation that observed poisoning.
        operation: &'static str,
    },
    /// A blocking task failed to join in the optional async wrapper.
    #[error("async runtime error: {message}")]
    Join {
        /// Human-readable join failure.
        message: String,
    },
}

impl LevelDbError {
    /// Returns the stable category of this error.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Io { .. } => ErrorKind::Io,
            Self::Corruption { .. } => ErrorKind::Corruption,
            Self::InvalidArgument { .. } => ErrorKind::InvalidArgument,
            Self::Unsupported { .. } => ErrorKind::Unsupported,
            Self::Compression { .. } => ErrorKind::Compression,
            Self::Cancelled => ErrorKind::Cancelled,
            Self::ReadOnly => ErrorKind::ReadOnly,
            Self::AlreadyExists { .. } => ErrorKind::AlreadyExists,
            Self::NotFound { .. } => ErrorKind::NotFound,
            Self::LockPoisoned { .. } => ErrorKind::LockPoisoned,
            Self::Join { .. } => ErrorKind::Join,
        }
    }

    /// Returns the filesystem path associated with this error, when known.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Io { path, .. } | Self::Corruption { path, .. } => path.as_deref(),
            Self::AlreadyExists { path } | Self::NotFound { path } => Some(path),
            Self::InvalidArgument { .. }
            | Self::Unsupported { .. }
            | Self::Compression { .. }
            | Self::Cancelled
            | Self::ReadOnly
            | Self::LockPoisoned { .. }
            | Self::Join { .. } => None,
        }
    }

    pub(crate) fn io(
        operation: &'static str,
        path: impl Into<Option<PathBuf>>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    pub(crate) fn io_at(
        operation: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::io(operation, Some(path.into()), source)
    }

    pub(crate) fn corruption(message: impl Into<String>) -> Self {
        Self::Corruption {
            path: None,
            message: message.into(),
        }
    }

    pub(crate) fn corruption_at(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Corruption {
            path: Some(path.into()),
            message: message.into(),
        }
    }

    pub(crate) fn invalid_argument(message: impl Into<String>) -> Self {
        Self::InvalidArgument {
            message: message.into(),
        }
    }

    #[allow(
        dead_code,
        reason = "used by codec functions when optional features are disabled"
    )]
    pub(crate) fn unsupported(feature: &'static str, message: impl Into<String>) -> Self {
        Self::Unsupported {
            feature,
            message: message.into(),
        }
    }

    pub(crate) fn compression(codec: &'static str, message: impl Into<String>) -> Self {
        Self::Compression {
            codec,
            message: message.into(),
        }
    }

    pub(crate) fn already_exists(path: impl Into<PathBuf>) -> Self {
        Self::AlreadyExists { path: path.into() }
    }

    pub(crate) fn not_found(path: impl Into<PathBuf>) -> Self {
        Self::NotFound { path: path.into() }
    }

    pub(crate) const fn lock_poisoned(operation: &'static str) -> Self {
        Self::LockPoisoned { operation }
    }

    #[cfg(feature = "async")]
    pub(crate) fn join(message: impl Into<String>) -> Self {
        Self::Join {
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for LevelDbError {
    fn from(source: std::io::Error) -> Self {
        Self::io("perform filesystem I/O", None, source)
    }
}

fn path_suffix(path: Option<&Path>) -> String {
    path.map(|path| format!(" at {}", path.display()))
        .unwrap_or_default()
}
