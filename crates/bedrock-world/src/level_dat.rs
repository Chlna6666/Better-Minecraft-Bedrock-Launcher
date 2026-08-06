//! `level.dat` parsing and atomic write helpers.
//!
//! Bedrock `level.dat` starts with an 8-byte little-endian header followed by a
//! little-endian NBT compound. The read API keeps header warnings explicit so
//! tools can surface tolerated data issues without failing the entire open path.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, nbt_tags_equal_for_write, parse_root_nbt, serialize_root_nbt};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_LEVEL_DAT_PAYLOAD_BYTES: usize = u32::MAX as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Header metadata read from a `level.dat` file.
pub struct LevelDatHeader {
    /// Bedrock file format version field.
    pub version: u32,
    /// Payload length declared by the header.
    pub declared_len: u32,
    /// Payload bytes actually parsed by this crate.
    pub actual_payload_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Non-fatal conditions observed while reading `level.dat`.
pub enum LevelDatReadWarning {
    /// Header length exceeded the bytes available after the header.
    DeclaredLengthTooLarge {
        /// Length declared by the header.
        declared_len: u32,
        /// Bytes available after the header.
        actual_payload_len: usize,
    },
    /// Additional bytes were present after the declared payload.
    TrailingBytes {
        /// Length declared by the header.
        declared_len: u32,
        /// Bytes available after the header.
        actual_payload_len: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
/// Parsed `level.dat` document with header, root NBT, and warnings.
pub struct LevelDatDocument {
    /// Parsed header values.
    pub header: LevelDatHeader,
    /// Root little-endian NBT compound.
    pub root: NbtTag,
    /// Non-fatal read warnings.
    pub warnings: Vec<LevelDatReadWarning>,
}

impl LevelDatDocument {
    #[must_use]
    /// Creates a document with the given format version and root tag.
    pub fn new(version: u32, root: NbtTag) -> Self {
        Self {
            header: LevelDatHeader {
                version,
                declared_len: 0,
                actual_payload_len: 0,
            },
            root,
            warnings: Vec::new(),
        }
    }

    #[must_use]
    /// Returns the Bedrock `level.dat` format version from the header.
    pub const fn version(&self) -> u32 {
        self.header.version
    }
}

/// Parses a complete `level.dat` byte slice.
pub fn parse_level_dat_document(data: &[u8]) -> Result<LevelDatDocument> {
    if data.len() < 8 {
        return Err(BedrockWorldError::CorruptWorld(
            "level.dat is shorter than its 8-byte header".to_string(),
        ));
    }

    let version = read_header_u32(data, 0)?;
    let declared_len = read_header_u32(data, 4)?;
    let remaining = data.len().saturating_sub(8);
    let declared_len_usize = declared_len as usize;

    let mut warnings = Vec::new();
    let payload = if declared_len_usize <= remaining {
        if declared_len_usize < remaining {
            warnings.push(LevelDatReadWarning::TrailingBytes {
                declared_len,
                actual_payload_len: remaining,
            });
        }
        &data[8..8 + declared_len_usize]
    } else {
        warnings.push(LevelDatReadWarning::DeclaredLengthTooLarge {
            declared_len,
            actual_payload_len: remaining,
        });
        &data[8..]
    };

    let root = parse_root_nbt(payload)?;
    Ok(LevelDatDocument {
        header: LevelDatHeader {
            version,
            declared_len,
            actual_payload_len: payload.len(),
        },
        root,
        warnings,
    })
}

/// Reads and parses a `level.dat` file from disk.
pub fn read_level_dat_document(path: &Path) -> Result<LevelDatDocument> {
    let bytes = fs::read(path)?;
    parse_level_dat_document(&bytes)
}

/// Alias for [`read_level_dat_document`].
pub fn read_level_dat(path: &Path) -> Result<LevelDatDocument> {
    read_level_dat_document(path)
}

/// Writes a `level.dat` document through a temporary file and replacement.
pub fn write_level_dat_document(path: &Path, document: &LevelDatDocument) -> Result<()> {
    if path.file_name().is_some_and(|name| name != "level.dat") {
        return Err(BedrockWorldError::Validation(format!(
            "refusing to write non-level.dat file: {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        if !parent.is_dir() {
            return Err(BedrockWorldError::Validation(format!(
                "level.dat parent directory does not exist: {}",
                parent.display()
            )));
        }
    }

    let payload = serialize_root_nbt(&document.root)?;
    if payload.len() > MAX_LEVEL_DAT_PAYLOAD_BYTES {
        return Err(BedrockWorldError::Validation(
            "level.dat payload is too large".to_string(),
        ));
    }

    let mut bytes = Vec::with_capacity(payload.len() + 8);
    bytes.extend_from_slice(&document.header.version.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&payload);
    validate_level_dat_bytes_for_write(&bytes, &document.root, document.header.version)?;

    let temporary_path = temporary_level_dat_path(path);
    let mut file = fs::File::create(&temporary_path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);

    replace_file(&temporary_path, path)
}

/// Alias for [`write_level_dat_document`].
pub fn write_level_dat_atomic(path: &Path, document: &LevelDatDocument) -> Result<()> {
    write_level_dat_document(path, document)
}

#[cfg(feature = "async")]
/// Async wrapper for [`read_level_dat`] using `tokio::task::spawn_blocking`.
pub async fn read_level_dat_async(path: impl AsRef<Path>) -> Result<LevelDatDocument> {
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || read_level_dat(&path))
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
}

#[cfg(feature = "async")]
/// Async wrapper for [`write_level_dat_atomic`] using `tokio::task::spawn_blocking`.
pub async fn write_level_dat_atomic_async(
    path: impl AsRef<Path>,
    document: LevelDatDocument,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || write_level_dat_atomic(&path, &document))
        .await
        .map_err(|error| BedrockWorldError::Join(error.to_string()))?
}

/// Re-parses candidate bytes before replacing `level.dat`.
pub fn validate_level_dat_bytes_for_write(
    bytes: &[u8],
    expected_root: &NbtTag,
    expected_version: u32,
) -> Result<()> {
    let parsed = parse_level_dat_document(bytes)?;
    if parsed.header.version != expected_version {
        return Err(BedrockWorldError::Validation(
            "level.dat version changed during write validation".to_string(),
        ));
    }
    if parsed.header.declared_len as usize != bytes.len().saturating_sub(8) {
        return Err(BedrockWorldError::Validation(
            "level.dat declared length does not match payload".to_string(),
        ));
    }
    if !parsed.warnings.is_empty() {
        return Err(BedrockWorldError::Validation(format!(
            "level.dat validation produced warnings: {:?}",
            parsed.warnings
        )));
    }
    if !nbt_tags_equal_for_write(&parsed.root, expected_root) {
        return Err(BedrockWorldError::Validation(
            "level.dat roundtrip root mismatch".to_string(),
        ));
    }
    Ok(())
}

fn read_header_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data.get(offset..offset + 4).ok_or_else(|| {
        BedrockWorldError::CorruptWorld("level.dat header is incomplete".to_string())
    })?;
    let bytes: [u8; 4] = bytes
        .try_into()
        .map_err(|_| BedrockWorldError::CorruptWorld("invalid level.dat header".to_string()))?;
    Ok(u32::from_le_bytes(bytes))
}

fn temporary_level_dat_path(path: &Path) -> PathBuf {
    path.with_file_name("level.dat.bmcbtmp")
}

fn replace_file(source: &Path, target: &Path) -> Result<()> {
    replace_file_impl(source, target)
}

fn replace_file_impl(source: &Path, target: &Path) -> Result<()> {
    if fs::rename(source, target).is_ok() {
        return Ok(());
    }
    if target.exists() {
        fs::remove_file(target)?;
    }
    fs::rename(source, target)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::NbtTag;
    use indexmap::IndexMap;

    #[test]
    fn level_dat_header_roundtrips() {
        let mut root = IndexMap::new();
        root.insert("LevelName".to_string(), NbtTag::String("Test".to_string()));
        let root = NbtTag::Compound(root);
        let payload = serialize_root_nbt(&root).expect("serialize");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&10_u32.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);

        let document = parse_level_dat_document(&bytes).expect("parse");
        assert_eq!(document.header.version, 10);
        assert_eq!(document.header.actual_payload_len, payload.len());
        assert!(document.warnings.is_empty());
    }

    #[test]
    fn level_dat_warns_when_declared_length_is_too_large() {
        let root = NbtTag::Compound(IndexMap::new());
        let payload = serialize_root_nbt(&root).expect("serialize");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&10_u32.to_le_bytes());
        bytes.extend_from_slice(&((payload.len() + 8) as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);

        let document = parse_level_dat_document(&bytes).expect("parse");
        assert_eq!(
            document.warnings,
            vec![LevelDatReadWarning::DeclaredLengthTooLarge {
                declared_len: (payload.len() + 8) as u32,
                actual_payload_len: payload.len(),
            }]
        );
    }
}
