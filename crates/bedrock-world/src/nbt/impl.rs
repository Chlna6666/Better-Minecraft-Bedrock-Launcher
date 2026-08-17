//! Little-endian Bedrock NBT reader and writer.
//!
//! Bedrock worlds use little-endian numeric fields and occasionally store
//! consecutive root compounds inside one LevelDB value. This module keeps those
//! details local and validates container sizes before allocation.

use crate::error::{BedrockWorldError, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::{Cursor, Read, Write};
use std::ops::ControlFlow;

const MAX_NBT_DEPTH: usize = 128;
const MAX_NBT_CONTAINER_LENGTH: usize = 134_217_728;
const MAX_NBT_BYTE_LENGTH: usize = 32 * 1024 * 1024;
const MAX_NBT_STRING_BYTES: usize = u16::MAX as usize;

const TAG_END: u8 = 0x00;
const TAG_BYTE: u8 = 0x01;
const TAG_SHORT: u8 = 0x02;
const TAG_INT: u8 = 0x03;
const TAG_LONG: u8 = 0x04;
const TAG_FLOAT: u8 = 0x05;
const TAG_DOUBLE: u8 = 0x06;
const TAG_BYTE_ARRAY: u8 = 0x07;
const TAG_STRING: u8 = 0x08;
const TAG_LIST: u8 = 0x09;
const TAG_COMPOUND: u8 = 0x0a;
const TAG_INT_ARRAY: u8 = 0x0b;
const TAG_LONG_ARRAY: u8 = 0x0c;
const TAG_SHORT_ARRAY: u8 = 0x64;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
/// Owned Bedrock NBT tag.
pub enum NbtTag {
    /// `TAG_End`.
    End,
    /// Signed byte.
    Byte(i8),
    /// Signed 16-bit integer.
    Short(i16),
    /// Signed 32-bit integer.
    Int(i32),
    /// Signed 64-bit integer.
    Long(i64),
    /// 32-bit float.
    Float(f32),
    /// 64-bit float.
    Double(f64),
    /// Signed byte array.
    ByteArray(Vec<i8>),
    /// UTF-8 string.
    String(String),
    /// Homogeneous list.
    List(Vec<NbtTag>),
    /// Named tag map preserving source order.
    Compound(IndexMap<String, NbtTag>),
    /// Signed 32-bit integer array.
    IntArray(Vec<i32>),
    /// Signed 64-bit integer array.
    LongArray(Vec<i64>),
    /// Bedrock short array extension.
    ShortArray(Vec<i16>),
}

/// Alias kept for callers that prefer value-oriented naming.
pub type NbtValue = NbtTag;

#[derive(Debug, Clone, PartialEq)]
/// Borrow-friendly view of an NBT tag.
pub enum NbtRef<'a> {
    /// The End dimension.
    End,
    /// Byte variant.
    Byte(i8),
    /// Short variant.
    Short(i16),
    /// Int variant.
    Int(i32),
    /// Long variant.
    Long(i64),
    /// Float variant.
    Float(f32),
    /// Double variant.
    Double(f64),
    /// Byte array variant.
    ByteArray(Cow<'a, [i8]>),
    /// String variant.
    String(Cow<'a, str>),
    /// List variant.
    List(Vec<NbtRef<'a>>),
    /// Compound variant.
    Compound(Vec<(Cow<'a, str>, NbtRef<'a>)>),
    /// Int array variant.
    IntArray(Cow<'a, [i32]>),
    /// Long array variant.
    LongArray(Cow<'a, [i64]>),
    /// Short array variant.
    ShortArray(Cow<'a, [i16]>),
}

impl NbtRef<'_> {
    #[must_use]
    /// Converts this borrowed view into an owned [`NbtTag`].
    pub fn to_owned_tag(&self) -> NbtTag {
        match self {
            Self::End => NbtTag::End,
            Self::Byte(value) => NbtTag::Byte(*value),
            Self::Short(value) => NbtTag::Short(*value),
            Self::Int(value) => NbtTag::Int(*value),
            Self::Long(value) => NbtTag::Long(*value),
            Self::Float(value) => NbtTag::Float(*value),
            Self::Double(value) => NbtTag::Double(*value),
            Self::ByteArray(values) => NbtTag::ByteArray(values.to_vec()),
            Self::String(value) => NbtTag::String(value.to_string()),
            Self::List(values) => NbtTag::List(values.iter().map(Self::to_owned_tag).collect()),
            Self::Compound(values) => NbtTag::Compound(
                values
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_owned_tag()))
                    .collect(),
            ),
            Self::IntArray(values) => NbtTag::IntArray(values.to_vec()),
            Self::LongArray(values) => NbtTag::LongArray(values.to_vec()),
            Self::ShortArray(values) => NbtTag::ShortArray(values.to_vec()),
        }
    }
}

/// Reader for a single in-memory Bedrock NBT payload.
pub struct NbtReader<'a> {
    data: &'a [u8],
}

impl<'a> NbtReader<'a> {
    #[must_use]
    /// Creates a reader over a complete in-memory Bedrock NBT payload.
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Parses the payload as a single root NBT tag.
    pub fn parse_root(&self) -> Result<NbtTag> {
        parse_root_nbt(self.data)
    }

    /// Parses one root NBT tag and returns the number of bytes consumed.
    pub fn parse_root_with_consumed(&self) -> Result<(NbtTag, usize)> {
        parse_root_nbt_with_consumed(self.data)
    }

    /// Parses one root tag and exposes it through a borrowed-style view.
    pub fn parse_root_ref(&self) -> Result<NbtRef<'a>> {
        crate::nbt_ref::parse_root(self.data)
    }

    /// Returns a borrowed view over the same raw payload.
    pub fn view(&self) -> NbtView<'a> {
        NbtView::new(self.data)
    }
}

#[derive(Debug, Clone, Copy)]
/// Borrowed Bedrock NBT payload view.
pub struct NbtView<'a> {
    data: &'a [u8],
}

impl<'a> NbtView<'a> {
    #[must_use]
    /// Creates a borrowed view over a complete in-memory Bedrock NBT payload.
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Parses the payload into a flat stream of borrowed NBT events.
    pub fn events(&self) -> Result<Vec<NbtEvent<'a>>> {
        parse_nbt_events(self.data)
    }

    /// Visits borrowed NBT events without materializing an event vector.
    ///
    /// Returning [`ControlFlow::Break`] stops parsing immediately.
    pub fn visit_events(&self, visitor: impl FnMut(NbtEvent<'a>) -> ControlFlow<()>) -> Result<()> {
        visit_nbt_events(self.data, visitor)
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Borrowed NBT parse event.
pub enum NbtEvent<'a> {
    /// Begin compound variant.
    BeginCompound {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
    },
    /// End compound variant.
    EndCompound,
    /// Begin list variant.
    BeginList {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Raw NBT tag id used by every element in this list.
        element_type: u8,
        /// Number of elements declared by the list header.
        len: usize,
    },
    /// End list variant.
    EndList,
    /// Byte variant.
    Byte {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Parsed or raw value associated with this record.
        value: i8,
    },
    /// Short variant.
    Short {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Parsed or raw value associated with this record.
        value: i16,
    },
    /// Int variant.
    Int {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Parsed or raw value associated with this record.
        value: i32,
    },
    /// Long variant.
    Long {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Parsed or raw value associated with this record.
        value: i64,
    },
    /// Float variant.
    Float {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Parsed or raw value associated with this record.
        value: f32,
    },
    /// Double variant.
    Double {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Parsed or raw value associated with this record.
        value: f64,
    },
    /// String variant.
    String {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Parsed or raw value associated with this record.
        value: &'a str,
    },
    /// Byte array variant.
    ByteArray {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Raw payload bytes preserved for unsupported formats.
        bytes: &'a [u8],
    },
    /// Int array variant.
    IntArray {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Raw payload bytes preserved for unsupported formats.
        bytes: &'a [u8],
        /// Number of 32-bit integers declared by the array header.
        len: usize,
    },
    /// Long array variant.
    LongArray {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Raw payload bytes preserved for unsupported formats.
        bytes: &'a [u8],
        /// Number of 64-bit integers declared by the array header.
        len: usize,
    },
    /// Short array variant.
    ShortArray {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Raw payload bytes preserved for unsupported formats.
        bytes: &'a [u8],
        /// Number of 16-bit integers declared by the array header.
        len: usize,
    },
}

/// Writer for Bedrock little-endian NBT roots.
pub struct NbtWriter;

impl NbtWriter {
    /// Write root.
    pub fn write_root(tag: &NbtTag) -> Result<Vec<u8>> {
        serialize_root_nbt(tag)
    }
}

/// Parses one root compound from a byte slice.
pub fn parse_root_nbt(data: &[u8]) -> Result<NbtTag> {
    parse_root_nbt_with_consumed(data).map(|(tag, _)| tag)
}

/// Parses one root compound and returns the number of bytes consumed.
pub fn parse_root_nbt_with_consumed(data: &[u8]) -> Result<(NbtTag, usize)> {
    let mut cursor = Cursor::new(data);
    let (_, tag) = parse_named_tag(&mut cursor, 0)?;
    if !matches!(tag, NbtTag::Compound(_)) {
        return Err(BedrockWorldError::Nbt(
            "root NBT tag must be Compound".to_string(),
        ));
    }
    let consumed = usize::try_from(cursor.position())
        .map_err(|_| BedrockWorldError::Nbt("NBT cursor position overflowed".to_string()))?;
    Ok((tag, consumed))
}

/// Parses consecutive root compounds until `data` is exhausted.
pub fn parse_consecutive_root_nbt(mut data: &[u8]) -> Result<Vec<NbtTag>> {
    let mut tags = Vec::new();
    while !data.is_empty() {
        let (tag, consumed) = parse_root_nbt_with_consumed(data)?;
        if consumed == 0 || consumed > data.len() {
            return Err(BedrockWorldError::Nbt(
                "consecutive NBT parser did not advance".to_string(),
            ));
        }
        tags.push(tag);
        data = &data[consumed..];
    }
    Ok(tags)
}

/// Serializes a root compound to Bedrock little-endian NBT bytes.
pub fn serialize_root_nbt(tag: &NbtTag) -> Result<Vec<u8>> {
    validate_root_nbt_for_write(tag)?;
    let mut buf = Vec::new();
    serialize_named_tag(&mut buf, "", tag)?;
    Ok(buf)
}

/// Validates that a tag can be written as a root compound.
pub fn validate_root_nbt_for_write(tag: &NbtTag) -> Result<()> {
    match tag {
        NbtTag::Compound(_) => validate_nbt_tag_for_write(tag, 0, "<root>"),
        _ => Err(BedrockWorldError::Validation(
            "level.dat root must be Compound".to_string(),
        )),
    }
}

fn parse_named_tag(reader: &mut impl Read, depth: usize) -> Result<(String, NbtTag)> {
    let tag_type = read_u8(reader)?;
    if tag_type == TAG_END {
        return Ok((String::new(), NbtTag::End));
    }
    ensure_depth(depth, tag_type)?;
    let name = read_string(reader)?;
    let value = parse_tag_payload(reader, tag_type, depth)?;
    Ok((name, value))
}

fn parse_tag_payload(reader: &mut impl Read, tag_type: u8, depth: usize) -> Result<NbtTag> {
    ensure_depth(depth, tag_type)?;
    match tag_type {
        TAG_BYTE => Ok(NbtTag::Byte(read_i8(reader)?)),
        TAG_SHORT => Ok(NbtTag::Short(read_i16(reader)?)),
        TAG_INT => Ok(NbtTag::Int(read_i32(reader)?)),
        TAG_LONG => Ok(NbtTag::Long(read_i64(reader)?)),
        TAG_FLOAT => Ok(NbtTag::Float(read_f32(reader)?)),
        TAG_DOUBLE => Ok(NbtTag::Double(read_f64(reader)?)),
        TAG_BYTE_ARRAY => {
            let len = read_byte_length(reader, "ByteArray")?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_i8(reader)?);
            }
            Ok(NbtTag::ByteArray(values))
        }
        TAG_STRING => Ok(NbtTag::String(read_string(reader)?)),
        TAG_LIST => {
            let element_type = read_u8(reader)?;
            let len = read_container_length(reader, "List")?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(parse_tag_payload(reader, element_type, depth + 1)?);
            }
            Ok(NbtTag::List(values))
        }
        TAG_COMPOUND => {
            let mut map = IndexMap::new();
            loop {
                let tag_type = read_u8(reader)?;
                if tag_type == TAG_END {
                    break;
                }
                let name = read_string(reader)?;
                let value = parse_tag_payload(reader, tag_type, depth + 1)?;
                map.insert(name, value);
                if map.len() > MAX_NBT_CONTAINER_LENGTH {
                    return Err(BedrockWorldError::Nbt("Compound is too large".to_string()));
                }
            }
            Ok(NbtTag::Compound(map))
        }
        TAG_INT_ARRAY => {
            let len = read_container_length(reader, "IntArray")?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_i32(reader)?);
            }
            Ok(NbtTag::IntArray(values))
        }
        TAG_LONG_ARRAY => {
            let len = read_container_length(reader, "LongArray")?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_i64(reader)?);
            }
            Ok(NbtTag::LongArray(values))
        }
        TAG_SHORT_ARRAY => {
            let len = read_container_length(reader, "ShortArray")?;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(read_i16(reader)?);
            }
            Ok(NbtTag::ShortArray(values))
        }
        _ => Err(BedrockWorldError::Nbt(format!(
            "unknown NBT tag type: {tag_type}"
        ))),
    }
}

fn serialize_named_tag(writer: &mut impl Write, name: &str, tag: &NbtTag) -> Result<()> {
    let tag_type = tag_discriminant(tag);
    writer.write_all(&[tag_type])?;
    if tag_type != TAG_END {
        write_string(writer, name)?;
        serialize_tag_payload(writer, tag)?;
    }
    Ok(())
}

fn serialize_tag_payload(writer: &mut impl Write, tag: &NbtTag) -> Result<()> {
    match tag {
        NbtTag::End => Ok(()),
        NbtTag::Byte(value) => writer.write_all(&[*value as u8]).map_err(Into::into),
        NbtTag::Short(value) => write_i16(writer, *value),
        NbtTag::Int(value) => write_i32(writer, *value),
        NbtTag::Long(value) => write_i64(writer, *value),
        NbtTag::Float(value) => writer.write_all(&value.to_le_bytes()).map_err(Into::into),
        NbtTag::Double(value) => writer.write_all(&value.to_le_bytes()).map_err(Into::into),
        NbtTag::ByteArray(values) => {
            write_i32_len(writer, values.len())?;
            for value in values {
                writer.write_all(&[*value as u8])?;
            }
            Ok(())
        }
        NbtTag::String(value) => write_string(writer, value),
        NbtTag::List(values) => {
            let element_type = values.first().map_or(TAG_END, tag_discriminant);
            writer.write_all(&[element_type])?;
            write_i32_len(writer, values.len())?;
            for value in values {
                serialize_tag_payload(writer, value)?;
            }
            Ok(())
        }
        NbtTag::Compound(values) => {
            for (name, value) in values {
                serialize_named_tag(writer, name, value)?;
            }
            writer.write_all(&[TAG_END])?;
            Ok(())
        }
        NbtTag::IntArray(values) => {
            write_i32_len(writer, values.len())?;
            for value in values {
                write_i32(writer, *value)?;
            }
            Ok(())
        }
        NbtTag::LongArray(values) => {
            write_i32_len(writer, values.len())?;
            for value in values {
                write_i64(writer, *value)?;
            }
            Ok(())
        }
        NbtTag::ShortArray(values) => {
            write_i32_len(writer, values.len())?;
            for value in values {
                write_i16(writer, *value)?;
            }
            Ok(())
        }
    }
}

fn validate_nbt_tag_for_write(tag: &NbtTag, depth: usize, path: &str) -> Result<()> {
    if depth > MAX_NBT_DEPTH {
        return Err(BedrockWorldError::Validation(format!(
            "NBT nesting is too deep: {path}"
        )));
    }
    match tag {
        NbtTag::End
        | NbtTag::Byte(_)
        | NbtTag::Short(_)
        | NbtTag::Int(_)
        | NbtTag::Long(_)
        | NbtTag::Float(_)
        | NbtTag::Double(_) => Ok(()),
        NbtTag::String(value) => validate_string(value, path),
        NbtTag::ByteArray(values) => validate_array_len(values.len(), path),
        NbtTag::IntArray(values) => validate_array_len(values.len(), path),
        NbtTag::LongArray(values) => validate_array_len(values.len(), path),
        NbtTag::ShortArray(values) => validate_array_len(values.len(), path),
        NbtTag::List(values) => {
            validate_array_len(values.len(), path)?;
            let Some(first) = values.first() else {
                return Ok(());
            };
            let first_type = tag_discriminant(first);
            if first_type == TAG_END {
                return Err(BedrockWorldError::Validation(format!(
                    "NBT List cannot contain End: {path}"
                )));
            }
            for (index, value) in values.iter().enumerate() {
                if tag_discriminant(value) != first_type {
                    return Err(BedrockWorldError::Validation(format!(
                        "NBT List element type mismatch at {path}[{index}]"
                    )));
                }
                validate_nbt_tag_for_write(value, depth + 1, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        NbtTag::Compound(values) => {
            validate_array_len(values.len(), path)?;
            for (key, value) in values {
                validate_string(key, path)?;
                validate_nbt_tag_for_write(value, depth + 1, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
    }
}

/// Nbt tags equal for write.
pub fn nbt_tags_equal_for_write(left: &NbtTag, right: &NbtTag) -> bool {
    match (left, right) {
        (NbtTag::End, NbtTag::End) => true,
        (NbtTag::Byte(left), NbtTag::Byte(right)) => left == right,
        (NbtTag::Short(left), NbtTag::Short(right)) => left == right,
        (NbtTag::Int(left), NbtTag::Int(right)) => left == right,
        (NbtTag::Long(left), NbtTag::Long(right)) => left == right,
        (NbtTag::Float(left), NbtTag::Float(right)) => left.to_bits() == right.to_bits(),
        (NbtTag::Double(left), NbtTag::Double(right)) => left.to_bits() == right.to_bits(),
        (NbtTag::ByteArray(left), NbtTag::ByteArray(right)) => left == right,
        (NbtTag::String(left), NbtTag::String(right)) => left == right,
        (NbtTag::List(left), NbtTag::List(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| nbt_tags_equal_for_write(left, right))
        }
        (NbtTag::Compound(left), NbtTag::Compound(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|right_value| nbt_tags_equal_for_write(value, right_value))
                })
        }
        (NbtTag::IntArray(left), NbtTag::IntArray(right)) => left == right,
        (NbtTag::LongArray(left), NbtTag::LongArray(right)) => left == right,
        (NbtTag::ShortArray(left), NbtTag::ShortArray(right)) => left == right,
        _ => false,
    }
}

fn validate_string(value: &str, path: &str) -> Result<()> {
    if value.len() > MAX_NBT_STRING_BYTES {
        return Err(BedrockWorldError::Validation(format!(
            "NBT string is too long at {path}"
        )));
    }
    Ok(())
}

fn validate_array_len(len: usize, path: &str) -> Result<()> {
    if len > MAX_NBT_CONTAINER_LENGTH {
        return Err(BedrockWorldError::Validation(format!(
            "NBT container is too large at {path}"
        )));
    }
    if len > i32::MAX as usize {
        return Err(BedrockWorldError::Validation(format!(
            "NBT container length exceeds i32 at {path}"
        )));
    }
    Ok(())
}

fn ensure_depth(depth: usize, tag_type: u8) -> Result<()> {
    if depth > MAX_NBT_DEPTH {
        return Err(BedrockWorldError::Nbt(format!(
            "NBT nesting is too deep at tag {tag_type}"
        )));
    }
    Ok(())
}

fn tag_discriminant(tag: &NbtTag) -> u8 {
    match tag {
        NbtTag::End => TAG_END,
        NbtTag::Byte(_) => TAG_BYTE,
        NbtTag::Short(_) => TAG_SHORT,
        NbtTag::Int(_) => TAG_INT,
        NbtTag::Long(_) => TAG_LONG,
        NbtTag::Float(_) => TAG_FLOAT,
        NbtTag::Double(_) => TAG_DOUBLE,
        NbtTag::ByteArray(_) => TAG_BYTE_ARRAY,
        NbtTag::String(_) => TAG_STRING,
        NbtTag::List(_) => TAG_LIST,
        NbtTag::Compound(_) => TAG_COMPOUND,
        NbtTag::IntArray(_) => TAG_INT_ARRAY,
        NbtTag::LongArray(_) => TAG_LONG_ARRAY,
        NbtTag::ShortArray(_) => TAG_SHORT_ARRAY,
    }
}

fn read_container_length(reader: &mut impl Read, field_name: &str) -> Result<usize> {
    let len = read_i32(reader)?;
    if len < 0 {
        return Err(BedrockWorldError::Nbt(format!(
            "{field_name} length cannot be negative"
        )));
    }
    let len = len as usize;
    if len > MAX_NBT_CONTAINER_LENGTH {
        return Err(BedrockWorldError::Nbt(format!(
            "{field_name} length is too large: {len}"
        )));
    }
    Ok(len)
}

fn read_byte_length(reader: &mut impl Read, field_name: &str) -> Result<usize> {
    let len = read_container_length(reader, field_name)?;
    if len > MAX_NBT_BYTE_LENGTH {
        return Err(BedrockWorldError::Nbt(format!(
            "{field_name} byte length is too large: {len}"
        )));
    }
    Ok(len)
}

fn read_u8(reader: &mut impl Read) -> Result<u8> {
    let mut buf = [0; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_i8(reader: &mut impl Read) -> Result<i8> {
    Ok(read_u8(reader)? as i8)
}

fn read_i16(reader: &mut impl Read) -> Result<i16> {
    let mut buf = [0; 2];
    reader.read_exact(&mut buf)?;
    Ok(i16::from_le_bytes(buf))
}

fn read_i32(reader: &mut impl Read) -> Result<i32> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_i64(reader: &mut impl Read) -> Result<i64> {
    let mut buf = [0; 8];
    reader.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}

fn read_f32(reader: &mut impl Read) -> Result<f32> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

fn read_f64(reader: &mut impl Read) -> Result<f64> {
    let mut buf = [0; 8];
    reader.read_exact(&mut buf)?;
    Ok(f64::from_le_bytes(buf))
}

fn read_string(reader: &mut impl Read) -> Result<String> {
    let mut len = [0; 2];
    reader.read_exact(&mut len)?;
    let len = u16::from_le_bytes(len) as usize;
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    Ok(String::from_utf8(bytes)?)
}

fn write_string(writer: &mut impl Write, value: &str) -> Result<()> {
    validate_string(value, "<string>")?;
    writer.write_all(&(value.len() as u16).to_le_bytes())?;
    writer.write_all(value.as_bytes())?;
    Ok(())
}

fn write_i16(writer: &mut impl Write, value: i16) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_i32(writer: &mut impl Write, value: i32) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_i64(writer: &mut impl Write, value: i64) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_i32_len(writer: &mut impl Write, len: usize) -> Result<()> {
    validate_array_len(len, "<len>")?;
    write_i32(writer, len as i32)
}

fn parse_nbt_events(data: &[u8]) -> Result<Vec<NbtEvent<'_>>> {
    let mut events = Vec::new();
    visit_nbt_events(data, |event| {
        events.push(event);
        ControlFlow::Continue(())
    })?;
    Ok(events)
}

/// Visits a flat stream of borrowed NBT events without allocating a DOM or
/// intermediate event vector.
///
/// Returning [`ControlFlow::Break`] stops parsing immediately.
///
/// # Errors
///
/// Returns an error when the payload is truncated, malformed, exceeds parser
/// limits, or does not contain a compound root.
pub fn visit_nbt_events<'a>(
    data: &'a [u8],
    mut visitor: impl FnMut(NbtEvent<'a>) -> ControlFlow<()>,
) -> Result<()> {
    let mut reader = SliceNbtReader::new(data);
    let tag_type = reader.read_u8()?;
    if tag_type != TAG_COMPOUND {
        return Err(BedrockWorldError::Nbt(
            "root NBT tag must be Compound".to_string(),
        ));
    }
    let name = reader.read_string_ref()?;
    let _ = visit_event_payload(&mut reader, tag_type, Some(name), 0, &mut visitor)?;
    Ok(())
}

fn visit_event_payload<'a, F>(
    reader: &mut SliceNbtReader<'a>,
    tag_type: u8,
    name: Option<&'a str>,
    depth: usize,
    visitor: &mut F,
) -> Result<ControlFlow<()>>
where
    F: FnMut(NbtEvent<'a>) -> ControlFlow<()>,
{
    macro_rules! emit {
        ($event:expr) => {
            if visitor($event).is_break() {
                return Ok(ControlFlow::Break(()));
            }
        };
    }

    ensure_depth(depth, tag_type)?;
    match tag_type {
        TAG_BYTE => emit!(NbtEvent::Byte {
            name,
            value: reader.read_i8()?,
        }),
        TAG_SHORT => emit!(NbtEvent::Short {
            name,
            value: reader.read_i16()?,
        }),
        TAG_INT => emit!(NbtEvent::Int {
            name,
            value: reader.read_i32()?,
        }),
        TAG_LONG => emit!(NbtEvent::Long {
            name,
            value: reader.read_i64()?,
        }),
        TAG_FLOAT => emit!(NbtEvent::Float {
            name,
            value: reader.read_f32()?,
        }),
        TAG_DOUBLE => emit!(NbtEvent::Double {
            name,
            value: reader.read_f64()?,
        }),
        TAG_BYTE_ARRAY => {
            let len = reader.read_byte_length("ByteArray")?;
            emit!(NbtEvent::ByteArray {
                name,
                bytes: reader.take(len)?,
            });
        }
        TAG_STRING => emit!(NbtEvent::String {
            name,
            value: reader.read_string_ref()?,
        }),
        TAG_LIST => return visit_list_events(reader, name, depth, visitor),
        TAG_COMPOUND => return visit_compound_events(reader, name, depth, visitor),
        TAG_INT_ARRAY => {
            let len = reader.read_container_length("IntArray")?;
            emit!(NbtEvent::IntArray {
                name,
                bytes: reader.take_array_bytes(len, 4, "IntArray")?,
                len,
            });
        }
        TAG_LONG_ARRAY => {
            let len = reader.read_container_length("LongArray")?;
            emit!(NbtEvent::LongArray {
                name,
                bytes: reader.take_array_bytes(len, 8, "LongArray")?,
                len,
            });
        }
        TAG_SHORT_ARRAY => {
            let len = reader.read_container_length("ShortArray")?;
            emit!(NbtEvent::ShortArray {
                name,
                bytes: reader.take_array_bytes(len, 2, "ShortArray")?,
                len,
            });
        }
        TAG_END => {}
        _ => {
            return Err(BedrockWorldError::Nbt(format!(
                "unknown NBT tag type: {tag_type}"
            )));
        }
    }
    Ok(ControlFlow::Continue(()))
}

fn visit_list_events<'a, F>(
    reader: &mut SliceNbtReader<'a>,
    name: Option<&'a str>,
    depth: usize,
    visitor: &mut F,
) -> Result<ControlFlow<()>>
where
    F: FnMut(NbtEvent<'a>) -> ControlFlow<()>,
{
    let element_type = reader.read_u8()?;
    let len = reader.read_container_length("List")?;
    if visitor(NbtEvent::BeginList {
        name,
        element_type,
        len,
    })
    .is_break()
    {
        return Ok(ControlFlow::Break(()));
    }
    for _ in 0..len {
        if visit_event_payload(reader, element_type, None, depth + 1, visitor)?.is_break() {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(visitor(NbtEvent::EndList))
}

fn visit_compound_events<'a, F>(
    reader: &mut SliceNbtReader<'a>,
    name: Option<&'a str>,
    depth: usize,
    visitor: &mut F,
) -> Result<ControlFlow<()>>
where
    F: FnMut(NbtEvent<'a>) -> ControlFlow<()>,
{
    if visitor(NbtEvent::BeginCompound { name }).is_break() {
        return Ok(ControlFlow::Break(()));
    }
    loop {
        let child_type = reader.read_u8()?;
        if child_type == TAG_END {
            break;
        }
        let child_name = reader.read_string_ref()?;
        if visit_event_payload(reader, child_type, Some(child_name), depth + 1, visitor)?.is_break()
        {
            return Ok(ControlFlow::Break(()));
        }
    }
    Ok(visitor(NbtEvent::EndCompound))
}

struct SliceNbtReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> SliceNbtReader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| BedrockWorldError::Nbt("NBT slice range overflowed".to_string()))?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| BedrockWorldError::Nbt("NBT payload is truncated".to_string()))?;
        self.offset = end;
        Ok(bytes)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(*self
            .take(1)?
            .first()
            .ok_or_else(|| BedrockWorldError::Nbt("NBT byte is truncated".to_string()))?)
    }

    fn read_i8(&mut self) -> Result<i8> {
        Ok(self.read_u8()? as i8)
    }

    fn read_i16(&mut self) -> Result<i16> {
        let bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| BedrockWorldError::Nbt("NBT i16 is truncated".to_string()))?;
        Ok(i16::from_le_bytes(bytes))
    }

    fn read_i32(&mut self) -> Result<i32> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| BedrockWorldError::Nbt("NBT i32 is truncated".to_string()))?;
        Ok(i32::from_le_bytes(bytes))
    }

    fn read_i64(&mut self) -> Result<i64> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| BedrockWorldError::Nbt("NBT i64 is truncated".to_string()))?;
        Ok(i64::from_le_bytes(bytes))
    }

    fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_i32()? as u32))
    }

    fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.read_i64()? as u64))
    }

    fn read_string_ref(&mut self) -> Result<&'a str> {
        let len_bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| BedrockWorldError::Nbt("NBT string length is truncated".to_string()))?;
        let len = u16::from_le_bytes(len_bytes) as usize;
        let bytes = self.take(len)?;
        Ok(std::str::from_utf8(bytes)?)
    }

    fn read_container_length(&mut self, name: &str) -> Result<usize> {
        let len = self.read_i32()?;
        if len < 0 {
            return Err(BedrockWorldError::Nbt(format!(
                "{name} has negative length"
            )));
        }
        let len = usize::try_from(len)
            .map_err(|_| BedrockWorldError::Nbt(format!("{name} length overflow")))?;
        if len > MAX_NBT_CONTAINER_LENGTH {
            return Err(BedrockWorldError::Nbt(format!("{name} is too large")));
        }
        Ok(len)
    }

    fn read_byte_length(&mut self, name: &str) -> Result<usize> {
        let len = self.read_container_length(name)?;
        if len > MAX_NBT_BYTE_LENGTH {
            return Err(BedrockWorldError::Nbt(format!("{name} is too large")));
        }
        Ok(len)
    }

    fn take_array_bytes(&mut self, len: usize, width: usize, name: &str) -> Result<&'a [u8]> {
        let byte_len = len
            .checked_mul(width)
            .ok_or_else(|| BedrockWorldError::Nbt(format!("{name} byte length overflow")))?;
        self.take(byte_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_nbt_roundtrips_little_endian_tags() {
        let mut root = IndexMap::new();
        root.insert("Name".to_string(), NbtTag::String("World".to_string()));
        root.insert("Seed".to_string(), NbtTag::Long(-42));
        root.insert(
            "List".to_string(),
            NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(2)]),
        );
        let tag = NbtTag::Compound(root);

        let bytes = serialize_root_nbt(&tag).expect("serialize");
        let parsed = parse_root_nbt(&bytes).expect("parse");

        assert!(nbt_tags_equal_for_write(&tag, &parsed));
    }

    #[test]
    fn mixed_list_is_rejected_before_write() {
        let mut root = IndexMap::new();
        root.insert(
            "Bad".to_string(),
            NbtTag::List(vec![NbtTag::Int(1), NbtTag::String("bad".to_string())]),
        );

        assert!(validate_root_nbt_for_write(&NbtTag::Compound(root)).is_err());
    }

    #[test]
    fn root_parser_reports_consumed_bytes() {
        let mut root = IndexMap::new();
        root.insert("Name".to_string(), NbtTag::String("First".to_string()));
        let bytes = serialize_root_nbt(&NbtTag::Compound(root)).expect("serialize");
        let mut combined = bytes.clone();
        combined.extend_from_slice(&bytes);

        let (_, consumed) = parse_root_nbt_with_consumed(&combined).expect("parse");

        assert_eq!(consumed, bytes.len());
    }

    #[test]
    fn nbt_view_emits_borrowed_events_without_owned_dom() {
        let mut root = IndexMap::new();
        root.insert("Name".to_string(), NbtTag::String("Borrowed".to_string()));
        root.insert("Seed".to_string(), NbtTag::Long(42));
        root.insert("Bytes".to_string(), NbtTag::ByteArray(vec![1, 2, 3]));
        let bytes = serialize_root_nbt(&NbtTag::Compound(root)).expect("serialize");

        let events = NbtReader::new(&bytes).view().events().expect("events");

        assert!(matches!(
            events.first(),
            Some(NbtEvent::BeginCompound { name: Some("") })
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            NbtEvent::String {
                name: Some("Name"),
                value: "Borrowed"
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            NbtEvent::Long {
                name: Some("Seed"),
                value: 42
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            NbtEvent::ByteArray {
                name: Some("Bytes"),
                bytes
            } if *bytes == [1, 2, 3]
        )));
    }

    #[test]
    fn nbt_event_visitor_stops_without_materializing_remaining_events() {
        let root = NbtTag::Compound(IndexMap::from([
            ("Name".to_string(), NbtTag::String("Wanted".to_string())),
            ("Seed".to_string(), NbtTag::Long(42)),
        ]));
        let bytes = serialize_root_nbt(&root).expect("serialize");
        let mut visited = 0_usize;
        let mut found = None;

        NbtView::new(&bytes)
            .visit_events(|event| {
                visited = visited.saturating_add(1);
                if let NbtEvent::String {
                    name: Some("Name"),
                    value,
                } = event
                {
                    found = Some(value);
                    return ControlFlow::Break(());
                }
                ControlFlow::Continue(())
            })
            .expect("visit events");

        assert_eq!(found, Some("Wanted"));
        assert_eq!(visited, 2);
    }

    #[test]
    fn root_ref_parser_borrows_string_values() {
        let bytes = serialize_root_nbt(&NbtTag::Compound(IndexMap::from([(
            "Name".to_string(),
            NbtTag::String("Borrowed".to_string()),
        )])))
        .expect("serialize");

        let parsed = NbtReader::new(&bytes).parse_root_ref().expect("parse ref");
        let NbtRef::Compound(entries) = parsed else {
            panic!("expected compound");
        };

        assert!(matches!(
            entries.first(),
            Some((
                Cow::Borrowed("Name"),
                NbtRef::String(Cow::Borrowed("Borrowed"))
            ))
        ));
    }

    #[test]
    fn root_ref_parser_matches_owned_parser_for_all_tag_kinds() {
        let tag = NbtTag::Compound(IndexMap::from([
            ("Byte".to_string(), NbtTag::Byte(-1)),
            ("Short".to_string(), NbtTag::Short(-2)),
            ("Int".to_string(), NbtTag::Int(-3)),
            ("Long".to_string(), NbtTag::Long(-4)),
            ("Float".to_string(), NbtTag::Float(1.5)),
            ("Double".to_string(), NbtTag::Double(2.5)),
            ("Bytes".to_string(), NbtTag::ByteArray(vec![-1, 0, 1])),
            ("String".to_string(), NbtTag::String("value".to_string())),
            (
                "List".to_string(),
                NbtTag::List(vec![NbtTag::Int(1), NbtTag::Int(2)]),
            ),
            (
                "Compound".to_string(),
                NbtTag::Compound(IndexMap::from([(
                    "Nested".to_string(),
                    NbtTag::String("value".to_string()),
                )])),
            ),
            ("Ints".to_string(), NbtTag::IntArray(vec![-1, 0, 1])),
            ("Longs".to_string(), NbtTag::LongArray(vec![-1, 0, 1])),
            ("Shorts".to_string(), NbtTag::ShortArray(vec![-1, 0, 1])),
        ]));
        let bytes = serialize_root_nbt(&tag).expect("serialize");

        let owned = NbtReader::new(&bytes).parse_root().expect("parse owned");
        let borrowed = NbtReader::new(&bytes)
            .parse_root_ref()
            .expect("parse borrowed")
            .to_owned_tag();

        assert!(nbt_tags_equal_for_write(&owned, &borrowed));
    }

    #[test]
    fn consecutive_root_parser_advances_between_compounds() {
        let mut root = IndexMap::new();
        root.insert("Name".to_string(), NbtTag::String("First".to_string()));
        let bytes = serialize_root_nbt(&NbtTag::Compound(root)).expect("serialize");
        let mut combined = bytes.clone();
        combined.extend_from_slice(&bytes);

        let tags = parse_consecutive_root_nbt(&combined).expect("parse consecutive");

        assert_eq!(tags.len(), 2);
    }
}
