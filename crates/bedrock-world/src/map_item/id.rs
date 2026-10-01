use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Validated Bedrock map item identifier without the `map_` storage prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MapItemId(String);

impl MapItemId {
    /// Creates an ID from a non-empty printable ASCII suffix.
    ///
    /// This identifies the LevelDB key `map_<id>`; it does not read or modify world storage.
    /// IDs decoded from LevelDB use the same validation. The value is not a numeric map UUID.
    ///
    /// # Errors
    /// Returns a validation error when the suffix is empty or contains non-printable/non-ASCII
    /// bytes.
    pub fn new(id: impl Into<String>) -> crate::Result<Self> {
        let id = id.into();
        if id.is_empty() || !id.as_bytes().iter().all(u8::is_ascii_graphic) {
            return Err(crate::BedrockWorldError::Validation(
                "map id must be non-empty printable ASCII".to_string(),
            ));
        }
        Ok(Self(id))
    }

    pub(crate) fn unchecked(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the suffix without the `map_` storage prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Encodes this ID as the LevelDB key `map_<id>`.
    #[must_use]
    pub fn storage_key(&self) -> Bytes {
        Bytes::from(format!("map_{}", self.0))
    }

    /// Decodes and validates a LevelDB key beginning with `map_`.
    #[must_use]
    pub fn from_storage_key(key: &[u8]) -> Option<Self> {
        let suffix = key.strip_prefix(b"map_")?;
        if suffix.is_empty() || !suffix.iter().all(u8::is_ascii_graphic) {
            return None;
        }
        Some(Self(String::from_utf8_lossy(suffix).into_owned()))
    }
}

impl std::fmt::Display for MapItemId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for MapItemId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
