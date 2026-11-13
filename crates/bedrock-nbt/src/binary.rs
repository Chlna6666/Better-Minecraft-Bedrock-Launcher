//! Explicit Bedrock wire encodings and named root tags.
//!
//! Disk NBT (including older Pocket Edition saves) uses [`Encoding::LittleEndian`].
//! Network NBT from MCPE 0.16 uses [`Encoding::NetworkLittleEndian`]. No format
//! guessing, decompression or `level.dat` header handling is performed here.

use crate::{Error, NbtTag, Result, codec, validation};
use std::io::{Cursor, Read, Write};

/// Selects the integer and length encoding, independently of world versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Fixed-width little-endian values and unsigned 16-bit string lengths.
    LittleEndian,
    /// ZigZag VarInts/VarLongs, unsigned VarInt string lengths; other numbers stay LE.
    NetworkLittleEndian,
}

/// A root tag with its original name; compound roots are not required here.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedTag {
    /// Root name (empty for an End marker).
    pub name: String,
    /// Root payload.
    pub tag: NbtTag,
}

/// Reads one named root and returns the number of consumed bytes.
///
/// # Errors
/// Rejects truncated, oversized, over-nested or malformed input.
pub fn parse(data: &[u8], encoding: Encoding) -> Result<(NamedTag, usize)> {
    let mut reader = Decoder::new(data, encoding);
    let (name, tag) = codec::parse_named_tag(&mut reader, 0)?;
    Ok((NamedTag { name, tag }, reader.position() as usize))
}

/// Writes one named tag in the selected encoding.
///
/// # Errors
/// Rejects values that exceed codec limits or contain invalid lists/End children.
pub fn serialize(root: &NamedTag, encoding: Encoding) -> Result<Vec<u8>> {
    validation::validate_nbt_tag_for_write(&root.tag, 0, "<root>")?;
    if matches!(root.tag, NbtTag::End) && !root.name.is_empty() {
        return Err(Error::Validation("End has no name".into()));
    }
    let mut writer = Encoder::new(encoding);
    codec::serialize_named_tag(&mut writer, &root.name, &root.tag)?;
    Ok(writer.bytes)
}

pub(crate) struct Decoder<'a> {
    cursor: Cursor<&'a [u8]>,
    pub(crate) encoding: Encoding,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(data: &'a [u8], encoding: Encoding) -> Self {
        Self {
            cursor: Cursor::new(data),
            encoding,
        }
    }

    pub(crate) fn position(&self) -> u64 {
        self.cursor.position()
    }

    pub(crate) fn remaining(&self) -> usize {
        self.cursor.get_ref().len() - self.position() as usize
    }

    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let start = self.position() as usize;
        let end = start
            .checked_add(len)
            .ok_or_else(|| Error::Nbt("range overflow".into()))?;
        let bytes = self
            .cursor
            .get_ref()
            .get(start..end)
            .ok_or_else(|| Error::Nbt("NBT payload is truncated".into()))?;
        self.cursor.set_position(end as u64);
        Ok(bytes)
    }

    pub(crate) fn take_array(&mut self, len: usize, width: usize) -> Result<&'a [u8]> {
        let bytes = len
            .checked_mul(width)
            .ok_or_else(|| Error::Nbt("array size overflow".into()))?;
        self.take(bytes)
    }

    pub(crate) fn varint(&mut self, bits: u32) -> Result<u64> {
        let mut value = 0_u64;
        for shift in (0..bits).step_by(7) {
            let byte = codec::read_u8(self)?;
            let payload = u64::from(byte & 0x7f);
            if payload > (u64::MAX >> (64 - bits + shift)) {
                return Err(Error::Nbt("VarInt overflow".into()));
            }
            value |= payload << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::Nbt("unterminated VarInt".into()))
    }
}

impl Read for Decoder<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}

pub(crate) struct Encoder {
    pub(crate) bytes: Vec<u8>,
    pub(crate) encoding: Encoding,
}

impl Encoder {
    pub(crate) fn new(encoding: Encoding) -> Self {
        Self {
            bytes: Vec::new(),
            encoding,
        }
    }

    pub(crate) fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.bytes.push(value as u8 | 0x80);
            value >>= 7;
        }
        self.bytes.push(value as u8);
    }
}

impl Write for Encoder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
