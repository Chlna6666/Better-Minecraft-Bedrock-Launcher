//! Legacy second block-layer payload stored under chunk tag `0x34` (`BlockExtraData`).
//!
//! The physical value is `i32 count` followed by fixed six-byte entries: little-endian `u32` raw
//! location index, `u8` numeric block ID, and `u8` wide block data. The raw index remains the
//! authoritative representation because old world generations differ in whether Y is interpreted as
//! chunk-relative or subchunk-relative.

use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;

const HEADER_LEN: usize = 4;
const ENTRY_LEN: usize = 6;

/// One legacy extra-block entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LegacyBlockExtraDataEntry {
    /// Exact little-endian 32-bit location index stored on disk.
    pub raw_index: u32,
    /// Legacy numeric block ID for the second block layer.
    pub block_id: u8,
    /// Legacy wide block data byte for the second block layer.
    pub block_data: u8,
}

impl LegacyBlockExtraDataEntry {
    /// Builds the standard chunk-column index `((x << 4) | z) << 8 | y`.
    pub fn from_chunk_coordinates(
        local_x: u8,
        local_y: u8,
        local_z: u8,
        block_id: u8,
        block_data: u8,
    ) -> Result<Self> {
        if local_x >= 16 || local_z >= 16 {
            return Err(BedrockWorldError::Validation(format!(
                "legacy block extra-data X/Z coordinates must be below 16, got ({local_x}, {local_z})"
            )));
        }
        let raw_index = u32::from(local_y)
            | (u32::from(local_z) << 8)
            | (u32::from(local_x) << 12);
        Ok(Self {
            raw_index,
            block_id,
            block_data,
        })
    }

    /// Builds the subchunk-local interpretation used with legacy fixed-array SubChunks.
    pub fn from_subchunk_coordinates(
        local_x: u8,
        local_y: u8,
        local_z: u8,
        block_id: u8,
        block_data: u8,
    ) -> Result<Self> {
        if local_x >= 16 || local_y >= 16 || local_z >= 16 {
            return Err(BedrockWorldError::Validation(format!(
                "legacy subchunk extra-data coordinates must be below 16, got ({local_x}, {local_y}, {local_z})"
            )));
        }
        Self::from_chunk_coordinates(local_x, local_y, local_z, block_id, block_data)
    }

    /// Interprets the low 16 bits as chunk-column coordinates and refuses unknown high index bits.
    #[must_use]
    pub fn chunk_coordinates(self) -> Option<(u8, u8, u8)> {
        if self.raw_index & 0xffff_0000 != 0 {
            return None;
        }
        let local_y = (self.raw_index & 0xff) as u8;
        let local_z = ((self.raw_index >> 8) & 0x0f) as u8;
        let local_x = ((self.raw_index >> 12) & 0x0f) as u8;
        Some((local_x, local_y, local_z))
    }

    /// Interprets the entry as coordinates local to one legacy 16-high SubChunk.
    ///
    /// Returns `None` if high index bits are non-zero or the stored low-byte Y exceeds 15.
    #[must_use]
    pub fn subchunk_coordinates(self) -> Option<(u8, u8, u8)> {
        let (local_x, local_y, local_z) = self.chunk_coordinates()?;
        (local_y < 16).then_some((local_x, local_y, local_z))
    }
}

/// Validated zero-copy view of one complete `BlockExtraData` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyBlockExtraData {
    bytes: Bytes,
    count: u32,
}

impl LegacyBlockExtraData {
    /// Validates a raw `BlockExtraData` payload without materializing its entries.
    pub fn parse(bytes: Bytes) -> Result<Self> {
        let count_bytes: [u8; 4] = bytes
            .get(..HEADER_LEN)
            .ok_or_else(|| {
                BedrockWorldError::UnsupportedChunkFormat(
                    "legacy block extra-data value is shorter than its 4-byte count".to_string(),
                )
            })?
            .try_into()
            .map_err(|_| {
                BedrockWorldError::UnsupportedChunkFormat(
                    "legacy block extra-data count is truncated".to_string(),
                )
            })?;
        let signed_count = i32::from_le_bytes(count_bytes);
        let count = u32::try_from(signed_count).map_err(|_| {
            BedrockWorldError::CorruptWorld(format!(
                "legacy block extra-data count is negative: {signed_count}"
            ))
        })?;
        let entry_bytes = usize::try_from(count)
            .ok()
            .and_then(|count| count.checked_mul(ENTRY_LEN))
            .ok_or_else(|| {
                BedrockWorldError::CorruptWorld(
                    "legacy block extra-data entry length overflowed".to_string(),
                )
            })?;
        let expected_len = HEADER_LEN.checked_add(entry_bytes).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(
                "legacy block extra-data total length overflowed".to_string(),
            )
        })?;
        if bytes.len() != expected_len {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "legacy block extra-data count {count} requires {expected_len} bytes, got {}",
                bytes.len()
            )));
        }
        Ok(Self { bytes, count })
    }

    /// Returns the number of encoded extra-block entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count as usize
    }

    /// Returns whether there are no second-layer blocks.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns the exact raw payload.
    #[must_use]
    pub fn raw(&self) -> &Bytes {
        &self.bytes
    }

    /// Iterates fixed-size entries directly from the backing `Bytes` without allocation.
    #[must_use]
    pub fn entries(&self) -> LegacyBlockExtraDataEntries<'_> {
        LegacyBlockExtraDataEntries {
            bytes: &self.bytes[HEADER_LEN..],
            offset: 0,
        }
    }

    /// Creates an editable one-allocation copy that preserves raw index values exactly.
    #[must_use]
    pub fn to_builder(&self) -> LegacyBlockExtraDataBuilder {
        LegacyBlockExtraDataBuilder {
            bytes: self.bytes.to_vec(),
        }
    }

    /// Returns the owned raw payload without copying its bytes.
    #[must_use]
    pub fn into_raw(self) -> Bytes {
        self.bytes
    }
}

/// Allocation-free iterator over legacy extra-block entries.
#[derive(Debug, Clone)]
pub struct LegacyBlockExtraDataEntries<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Iterator for LegacyBlockExtraDataEntries<'_> {
    type Item = LegacyBlockExtraDataEntry;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.bytes.get(self.offset..self.offset + ENTRY_LEN)?;
        self.offset += ENTRY_LEN;
        Some(LegacyBlockExtraDataEntry {
            raw_index: u32::from_le_bytes(entry[..4].try_into().ok()?),
            block_id: entry[4],
            block_data: entry[5],
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.bytes.len().saturating_sub(self.offset)) / ENTRY_LEN;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for LegacyBlockExtraDataEntries<'_> {}

/// One-allocation builder for a `BlockExtraData` value.
#[derive(Debug, Clone)]
pub struct LegacyBlockExtraDataBuilder {
    bytes: Vec<u8>,
}

impl Default for LegacyBlockExtraDataBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl LegacyBlockExtraDataBuilder {
    /// Creates an empty payload.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bytes: vec![0; HEADER_LEN],
        }
    }

    /// Creates an empty payload preallocated for `entries` records.
    pub fn with_capacity(entries: usize) -> Result<Self> {
        let payload = entries.checked_mul(ENTRY_LEN).ok_or_else(|| {
            BedrockWorldError::Validation(
                "legacy block extra-data capacity overflowed".to_string(),
            )
        })?;
        let capacity = HEADER_LEN.checked_add(payload).ok_or_else(|| {
            BedrockWorldError::Validation(
                "legacy block extra-data capacity overflowed".to_string(),
            )
        })?;
        let mut bytes = Vec::with_capacity(capacity);
        bytes.extend_from_slice(&[0; HEADER_LEN]);
        Ok(Self { bytes })
    }

    /// Returns the number of entries currently queued for encoding.
    #[must_use]
    pub fn len(&self) -> usize {
        (self.bytes.len() - HEADER_LEN) / ENTRY_LEN
    }

    /// Returns whether no entries are queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.len() == HEADER_LEN
    }

    /// Appends one entry while preserving its raw location index exactly.
    pub fn push(&mut self, entry: LegacyBlockExtraDataEntry) -> Result<()> {
        if self.len() >= i32::MAX as usize {
            return Err(BedrockWorldError::Validation(
                "legacy block extra-data exceeds i32 entry count".to_string(),
            ));
        }
        self.bytes.extend_from_slice(&entry.raw_index.to_le_bytes());
        self.bytes.push(entry.block_id);
        self.bytes.push(entry.block_data);
        Ok(())
    }

    /// Appends one entry using chunk-column coordinates.
    pub fn push_chunk_coordinates(
        &mut self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
        block_id: u8,
        block_data: u8,
    ) -> Result<()> {
        self.push(LegacyBlockExtraDataEntry::from_chunk_coordinates(
            local_x,
            local_y,
            local_z,
            block_id,
            block_data,
        )?)
    }

    /// Appends one entry using SubChunk-local coordinates.
    pub fn push_subchunk_coordinates(
        &mut self,
        local_x: u8,
        local_y: u8,
        local_z: u8,
        block_id: u8,
        block_data: u8,
    ) -> Result<()> {
        self.push(LegacyBlockExtraDataEntry::from_subchunk_coordinates(
            local_x,
            local_y,
            local_z,
            block_id,
            block_data,
        )?)
    }

    /// Finalizes the little-endian count and validates the resulting payload.
    pub fn build(mut self) -> Result<LegacyBlockExtraData> {
        let count = i32::try_from(self.len()).map_err(|_| {
            BedrockWorldError::Validation(
                "legacy block extra-data exceeds i32 entry count".to_string(),
            )
        })?;
        self.bytes[..HEADER_LEN].copy_from_slice(&count.to_le_bytes());
        LegacyBlockExtraData::parse(Bytes::from(self.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_data_roundtrips_chunk_and_subchunk_coordinate_views() {
        let mut builder = LegacyBlockExtraDataBuilder::with_capacity(2).unwrap();
        builder
            .push_chunk_coordinates(15, 200, 9, 8, 240)
            .unwrap();
        builder
            .push_subchunk_coordinates(3, 14, 4, 21, 7)
            .unwrap();
        let data = builder.build().unwrap();
        assert_eq!(data.len(), 2);
        let entries = data.entries().collect::<Vec<_>>();
        assert_eq!(entries[0].chunk_coordinates(), Some((15, 200, 9)));
        assert_eq!(entries[0].subchunk_coordinates(), None);
        assert_eq!(entries[0].block_data, 240);
        assert_eq!(entries[1].subchunk_coordinates(), Some((3, 14, 4)));
    }

    #[test]
    fn raw_unknown_index_bits_are_preserved() {
        let entry = LegacyBlockExtraDataEntry {
            raw_index: 0xabcd_1234,
            block_id: 2,
            block_data: 3,
        };
        let mut builder = LegacyBlockExtraDataBuilder::new();
        builder.push(entry).unwrap();
        let parsed = builder.build().unwrap();
        assert_eq!(parsed.entries().next(), Some(entry));
        assert_eq!(parsed.entries().next().unwrap().chunk_coordinates(), None);
    }

    #[test]
    fn malformed_count_and_trailing_bytes_are_rejected() {
        assert!(LegacyBlockExtraData::parse(Bytes::from_static(&[0xff, 0xff, 0xff, 0xff])).is_err());
        assert!(LegacyBlockExtraData::parse(Bytes::from_static(&[0, 0, 0, 0, 1])).is_err());
    }

    #[test]
    fn empty_payload_is_valid_and_exact_size_iterator_reports_zero() {
        let data = LegacyBlockExtraDataBuilder::new().build().unwrap();
        assert!(data.is_empty());
        assert_eq!(data.entries().len(), 0);
        assert_eq!(data.raw().as_ref(), &[0, 0, 0, 0]);
    }
}
