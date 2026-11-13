//! Persisted world-wide NBT records, including events, scoreboards and ticking areas.
//!
//! These APIs edit saved data; they do not simulate game events or execute ticks.

use crate::chunk::GlobalRecordKind;
use crate::error::Result;
use crate::nbt::{NbtTag, parse_consecutive_roots};
use bytes::Bytes;

/// An editable global record with an optional source snapshot for guarded writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    kind: GlobalRecordKind,
    roots: Vec<NbtTag>,
    source: Option<Bytes>,
}

impl Record {
    /// Creates a record whose storage key must be absent when saved.
    #[must_use]
    pub fn new(kind: GlobalRecordKind, roots: Vec<NbtTag>) -> Self {
        Self {
            kind,
            roots,
            source: None,
        }
    }

    /// Returns the exact persisted key family.
    #[must_use]
    pub fn kind(&self) -> &GlobalRecordKind {
        &self.kind
    }

    /// Returns the consecutive NBT roots, including unknown fields.
    #[must_use]
    pub fn roots(&self) -> &[NbtTag] {
        &self.roots
    }

    /// Borrows the roots for editing while retaining the original source snapshot.
    pub fn roots_mut(&mut self) -> &mut Vec<NbtTag> {
        &mut self.roots
    }

    /// Returns the original bytes; `None` means this is a new record.
    ///
    /// A present empty record remains `Some`, so it is never mistaken for absence.
    #[must_use]
    pub fn source(&self) -> Option<&Bytes> {
        self.source.as_ref()
    }
}

/// Parses a global NBT record in memory and retains its exact source bytes.
///
/// # Errors
/// Returns an error for malformed NBT.
pub fn parse(kind: GlobalRecordKind, value: Bytes) -> Result<Record> {
    Ok(Record {
        kind,
        roots: parse_consecutive_roots(&value)?,
        source: Some(value),
    })
}

/// Encodes an edited global record without writing to any storage backend.
///
/// # Errors
/// Returns an error for invalid NBT roots.
pub fn encode(record: &Record) -> Result<Bytes> {
    Ok(Bytes::from(crate::nbt::serialize_consecutive_roots(
        record.roots(),
    )?))
}
