use std::{fmt, io};

/// Failures while validating or reading a `BRArchive` container.
#[derive(Debug)]
pub enum Error {
    /// File access, truncated reads or output failures.
    Io(io::Error),
    /// Header, entry table, name or payload range violates the v1 layout.
    Invalid(&'static str),
    /// The container declares a format version this reader cannot interpret.
    UnsupportedVersion(u32),
    /// The index or requested allocation exceeds the documented reader limits.
    LimitExceeded(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "BRArchive I/O: {error}"),
            Self::Invalid(reason) => write!(formatter, "invalid BRArchive: {reason}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported BRArchive version {version}")
            }
            Self::LimitExceeded(reason) => write!(formatter, "BRArchive limit exceeded: {reason}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
