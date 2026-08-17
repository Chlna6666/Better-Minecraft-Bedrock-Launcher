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

    /// Returns the map-owned `RandomSeed` without modifying the document.
    ///
    /// Bedrock stores the seed as a signed 64-bit integer in modern worlds, although some
    /// older tooling has emitted an `Int`. Both representations are accepted. A missing seed
    /// is reported as `Ok(None)` so callers can distinguish a new/incomplete document from an
    /// existing map. Any other scalar type is treated as corruption instead of silently
    /// falling back to a configured or randomly generated seed.
    pub fn random_seed(&self) -> Result<Option<i64>> {
        let NbtTag::Compound(root) = &self.root else {
            return Err(BedrockWorldError::CorruptWorld(
                "level.dat root is not a compound".to_string(),
            ));
        };
        match root.get("RandomSeed") {
            None => Ok(None),
            Some(NbtTag::Long(seed)) => Ok(Some(*seed)),
            Some(NbtTag::Int(seed)) => Ok(Some(i64::from(*seed))),
            Some(other) => Err(BedrockWorldError::CorruptWorld(format!(
                "level.dat RandomSeed uses unsupported NBT type: {other:?}"
            ))),
        }
    }

    /// Initializes `RandomSeed` only when the document does not already own a seed.
    ///
    /// Existing maps are authoritative: if `RandomSeed` already exists, its value is returned
    /// unchanged even when `candidate` differs. This prevents map editors or servers from
    /// accidentally generating new chunks with a different seed and creating terrain seams.
    pub fn initialize_random_seed_if_missing(&mut self, candidate: i64) -> Result<i64> {
        if let Some(seed) = self.random_seed()? {
            return Ok(seed);
        }
        let NbtTag::Compound(root) = &mut self.root else {
            return Err(BedrockWorldError::CorruptWorld(
                "level.dat root is not a compound".to_string(),
            ));
        };
        root.insert("RandomSeed".to_string(), NbtTag::Long(candidate));
        Ok(candidate)
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

/// Reads the map-owned `RandomSeed` from an existing `level.dat`.
///
/// Unlike application-level configuration fallbacks, this function never invents a seed. A
/// missing seed remains `None` and malformed metadata is returned as an error.
pub fn read_level_dat_random_seed(path: &Path) -> Result<Option<i64>> {
    read_level_dat_document(path)?.random_seed()
}

/// Initializes a missing `RandomSeed` and atomically writes the document only when needed.
///
/// If the map already has a seed, no file write occurs and the existing value is returned.
pub fn initialize_level_dat_random_seed_if_missing(path: &Path, candidate: i64) -> Result<i64> {
    let mut document = read_level_dat_document(path)?;
    if let Some(seed) = document.random_seed()? {
        return Ok(seed);
    }
    let seed = document.initialize_random_seed_if_missing(candidate)?;
    write_level_dat_document(path, &document)?;
    Ok(seed)
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

    #[test]
    fn existing_random_seed_is_never_replaced() {
        let mut root = IndexMap::new();
        root.insert("RandomSeed".to_string(), NbtTag::Long(123456789));
        let mut document = LevelDatDocument::new(10, NbtTag::Compound(root));

        let resolved = document
            .initialize_random_seed_if_missing(-987654321)
            .expect("seed");
        assert_eq!(resolved, 123456789);
        assert_eq!(document.random_seed().expect("read"), Some(123456789));
    }

    #[test]
    fn missing_random_seed_is_initialized_once() {
        let mut document = LevelDatDocument::new(10, NbtTag::Compound(IndexMap::new()));
        assert_eq!(document.random_seed().expect("read"), None);
        assert_eq!(
            document
                .initialize_random_seed_if_missing(42)
                .expect("initialize"),
            42
        );
        assert_eq!(document.random_seed().expect("read"), Some(42));
        assert_eq!(
            document
                .initialize_random_seed_if_missing(99)
                .expect("preserve"),
            42
        );
    }

    #[test]
    fn malformed_random_seed_does_not_fall_back() {
        let mut root = IndexMap::new();
        root.insert(
            "RandomSeed".to_string(),
            NbtTag::String("not-a-seed".to_string()),
        );
        let document = LevelDatDocument::new(10, NbtTag::Compound(root));
        assert!(document.random_seed().is_err());
    }
}
