use crate::binary::{Decoder, Encoder, Encoding};
use crate::{
    Error, NbtTag, Result,
    constants::*,
    tag::discriminant,
    validation::{
        ensure_depth, validate_array_len, validate_root_for_write, validate_string,
        validate_string_bytes,
    },
};
use indexmap::IndexMap;
use std::io::{Read, Write};

/// Parses one root compound from a byte slice.
///
/// # Errors
/// Rejects malformed input, unsupported tag IDs, codec limits and non-compound roots.
pub fn parse_root(data: &[u8]) -> Result<NbtTag> {
    parse_root_with_consumed(data).map(|(tag, _)| tag)
}

/// Parses one root compound and returns the number of bytes consumed.
///
/// # Errors
/// Rejects malformed input, unsupported tag IDs, codec limits and non-compound roots.
pub fn parse_root_with_consumed(data: &[u8]) -> Result<(NbtTag, usize)> {
    let mut cursor = Decoder::new(data, Encoding::LittleEndian);
    let (_, tag) = parse_named_tag(&mut cursor, 0)?;
    if !matches!(tag, NbtTag::Compound(_)) {
        return Err(Error::Nbt("root NBT tag must be Compound".to_string()));
    }
    let consumed = usize::try_from(cursor.position())
        .map_err(|_| Error::Nbt("NBT cursor position overflowed".to_string()))?;
    Ok((tag, consumed))
}

/// Parses consecutive root compounds until `data` is exhausted.
///
/// # Errors
/// Rejects any malformed or non-compound root, including trailing incomplete bytes.
pub fn parse_consecutive_roots(mut data: &[u8]) -> Result<Vec<NbtTag>> {
    let mut tags = Vec::new();
    while !data.is_empty() {
        let (tag, consumed) = parse_root_with_consumed(data)?;
        if consumed == 0 || consumed > data.len() {
            return Err(Error::Nbt(
                "consecutive NBT parser did not advance".to_string(),
            ));
        }
        tags.push(tag);
        data = &data[consumed..];
    }
    Ok(tags)
}

/// Serializes a root compound to Bedrock little-endian NBT bytes.
///
/// # Errors
/// Rejects non-compound roots, invalid lists/End children and values exceeding limits.
pub fn serialize_root(tag: &NbtTag) -> Result<Vec<u8>> {
    validate_root_for_write(tag)?;
    let mut buf = Encoder::new(Encoding::LittleEndian);
    serialize_named_tag(&mut buf, "", tag)?;
    Ok(buf.bytes)
}

/// Serializes consecutive little-endian root compounds into one buffer.
///
/// An empty sequence produces an empty payload, matching [`parse_consecutive_roots`].
///
/// # Errors
/// Rejects non-compound roots, invalid NBT children and values exceeding codec limits.
pub fn serialize_consecutive_roots(roots: &[NbtTag]) -> Result<Vec<u8>> {
    let mut buffer = Encoder::new(Encoding::LittleEndian);
    for root in roots {
        validate_root_for_write(root)?;
        serialize_named_tag(&mut buffer, "", root)?;
    }
    Ok(buffer.bytes)
}

pub(crate) fn parse_named_tag(reader: &mut Decoder<'_>, depth: usize) -> Result<(String, NbtTag)> {
    let tag_type = read_u8(reader)?;
    if tag_type == TAG_END {
        return Ok((String::new(), NbtTag::End));
    }
    ensure_depth(depth, tag_type)?;
    let name = read_string(reader)?;
    let value = parse_tag_payload(reader, tag_type, depth)?;
    Ok((name, value))
}

fn parse_tag_payload(reader: &mut Decoder<'_>, tag_type: u8, depth: usize) -> Result<NbtTag> {
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
            let values = reader.take(len)?.iter().map(|value| *value as i8).collect();
            Ok(NbtTag::ByteArray(values))
        }
        TAG_STRING => {
            let bytes = read_string_bytes(reader)?;
            Ok(match String::from_utf8(bytes) {
                Ok(value) => NbtTag::String(value),
                Err(error) => NbtTag::ByteString(error.into_bytes()),
            })
        }
        TAG_LIST => parse_list(reader, depth),
        TAG_COMPOUND => parse_compound(reader, depth),
        TAG_INT_ARRAY => parse_int_array(reader),
        TAG_LONG_ARRAY => parse_long_array(reader),
        TAG_SHORT_ARRAY => parse_short_array(reader),
        _ => Err(Error::Nbt(format!("unknown NBT tag type: {tag_type}"))),
    }
}

pub(crate) fn serialize_named_tag(writer: &mut Encoder, name: &str, tag: &NbtTag) -> Result<()> {
    let tag_type = discriminant(tag);
    writer.write_all(&[tag_type])?;
    if tag_type != TAG_END {
        write_string(writer, name)?;
        serialize_tag_payload(writer, tag)?;
    }
    Ok(())
}

fn serialize_tag_payload(writer: &mut Encoder, tag: &NbtTag) -> Result<()> {
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
            writer.bytes.extend(values.iter().map(|value| *value as u8));
            Ok(())
        }
        NbtTag::String(value) => write_string(writer, value),
        NbtTag::ByteString(value) => write_string_bytes(writer, value),
        NbtTag::List(values) => {
            let element_type = values.first().map_or(TAG_END, discriminant);
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

fn read_container_length(reader: &mut Decoder<'_>, field_name: &str) -> Result<usize> {
    let len = read_i32(reader)?;
    if len < 0 {
        return Err(Error::Nbt(format!(
            "{field_name} length cannot be negative"
        )));
    }
    let len = len as usize;
    if len > MAX_NBT_CONTAINER_LENGTH {
        return Err(Error::Nbt(format!(
            "{field_name} length is too large: {len}"
        )));
    }
    Ok(len)
}

fn read_byte_length(reader: &mut Decoder<'_>, field_name: &str) -> Result<usize> {
    let len = read_container_length(reader, field_name)?;
    if len > MAX_NBT_BYTE_LENGTH {
        return Err(Error::Nbt(format!(
            "{field_name} byte length is too large: {len}"
        )));
    }
    Ok(len)
}

pub(crate) fn read_u8(reader: &mut Decoder<'_>) -> Result<u8> {
    let mut buf = [0; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_i8(reader: &mut Decoder<'_>) -> Result<i8> {
    Ok(read_u8(reader)? as i8)
}

fn read_i16(reader: &mut Decoder<'_>) -> Result<i16> {
    let mut buf = [0; 2];
    reader.read_exact(&mut buf)?;
    Ok(i16::from_le_bytes(buf))
}

fn read_i32(reader: &mut Decoder<'_>) -> Result<i32> {
    if reader.encoding == Encoding::NetworkLittleEndian {
        let value = reader.varint(32)? as u32;
        return Ok(((value >> 1) as i32) ^ -((value & 1) as i32));
    }
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_i64(reader: &mut Decoder<'_>) -> Result<i64> {
    if reader.encoding == Encoding::NetworkLittleEndian {
        let value = reader.varint(64)?;
        return Ok(((value >> 1) as i64) ^ -((value & 1) as i64));
    }
    let mut buf = [0; 8];
    reader.read_exact(&mut buf)?;
    Ok(i64::from_le_bytes(buf))
}

fn read_f32(reader: &mut Decoder<'_>) -> Result<f32> {
    let mut buf = [0; 4];
    reader.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

fn read_f64(reader: &mut Decoder<'_>) -> Result<f64> {
    let mut buf = [0; 8];
    reader.read_exact(&mut buf)?;
    Ok(f64::from_le_bytes(buf))
}

fn read_string(reader: &mut Decoder<'_>) -> Result<String> {
    Ok(String::from_utf8(read_string_bytes(reader)?)?)
}

fn read_string_bytes(reader: &mut Decoder<'_>) -> Result<Vec<u8>> {
    let len = if reader.encoding == Encoding::NetworkLittleEndian {
        reader.varint(32)? as usize
    } else {
        let mut bytes = [0; 2];
        reader.read_exact(&mut bytes)?;
        u16::from_le_bytes(bytes) as usize
    };
    if len > MAX_NBT_STRING_BYTES {
        return Err(Error::Nbt("string exceeds byte limit".into()));
    }
    Ok(reader.take(len)?.to_vec())
}

fn write_string(writer: &mut Encoder, value: &str) -> Result<()> {
    validate_string(value, "<string>")?;
    write_string_bytes(writer, value.as_bytes())
}

fn write_string_bytes(writer: &mut Encoder, value: &[u8]) -> Result<()> {
    validate_string_bytes(value, "<string>")?;
    if writer.encoding == Encoding::NetworkLittleEndian {
        writer.varint(value.len() as u64);
    } else {
        writer.write_all(&(value.len() as u16).to_le_bytes())?;
    }
    writer.write_all(value)?;
    Ok(())
}

fn write_i16(writer: &mut Encoder, value: i16) -> Result<()> {
    writer.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn write_i32(writer: &mut Encoder, value: i32) -> Result<()> {
    if writer.encoding == Encoding::NetworkLittleEndian {
        writer.varint(((value as u32) << 1 ^ ((value >> 31) as u32)) as u64);
    } else {
        writer.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

fn write_i64(writer: &mut Encoder, value: i64) -> Result<()> {
    if writer.encoding == Encoding::NetworkLittleEndian {
        writer.varint(((value as u64) << 1 ^ ((value >> 63) as u64)) as u64);
    } else {
        writer.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

fn write_i32_len(writer: &mut Encoder, len: usize) -> Result<()> {
    validate_array_len(len, "<len>")?;
    write_i32(writer, len as i32)
}

fn parse_list(reader: &mut Decoder<'_>, depth: usize) -> Result<NbtTag> {
    let element_type = read_u8(reader)?;
    let raw_len = read_i32(reader)?;
    let len = raw_len.max(0) as usize;
    if len > MAX_NBT_CONTAINER_LENGTH || len > reader.remaining() {
        return Err(Error::Nbt(
            "List length exceeds available input or limit".into(),
        ));
    }
    if len > 0 && element_type == TAG_END {
        return Err(Error::Nbt("nonempty List cannot contain End".into()));
    }
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        values.push(parse_tag_payload(reader, element_type, depth + 1)?);
    }
    Ok(NbtTag::List(values))
}

fn parse_compound(reader: &mut Decoder<'_>, depth: usize) -> Result<NbtTag> {
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
            return Err(Error::Nbt("Compound is too large".to_string()));
        }
    }
    Ok(NbtTag::Compound(map))
}

fn parse_int_array(reader: &mut Decoder<'_>) -> Result<NbtTag> {
    let len = read_container_length(reader, "IntArray")?;
    let values = if reader.encoding == Encoding::LittleEndian {
        let bytes = reader.take_array(len, 4)?;
        bytes
            .chunks_exact(4)
            .map(|part| i32::from_le_bytes(std::array::from_fn(|index| part[index])))
            .collect()
    } else {
        if len > reader.remaining() {
            return Err(Error::Nbt("array length exceeds available input".into()));
        }
        (0..len)
            .map(|_| read_i32(reader))
            .collect::<Result<Vec<_>>>()?
    };
    Ok(NbtTag::IntArray(values))
}

fn parse_long_array(reader: &mut Decoder<'_>) -> Result<NbtTag> {
    let len = read_container_length(reader, "LongArray")?;
    let values = if reader.encoding == Encoding::LittleEndian {
        let bytes = reader.take_array(len, 8)?;
        bytes
            .chunks_exact(8)
            .map(|part| i64::from_le_bytes(std::array::from_fn(|index| part[index])))
            .collect()
    } else {
        if len > reader.remaining() {
            return Err(Error::Nbt("array length exceeds available input".into()));
        }
        (0..len)
            .map(|_| read_i64(reader))
            .collect::<Result<Vec<_>>>()?
    };
    Ok(NbtTag::LongArray(values))
}

fn parse_short_array(reader: &mut Decoder<'_>) -> Result<NbtTag> {
    let len = read_container_length(reader, "ShortArray")?;
    let bytes = reader.take_array(len, 2)?;
    let values = bytes
        .chunks_exact(2)
        .map(|part| i16::from_le_bytes(std::array::from_fn(|index| part[index])))
        .collect();
    Ok(NbtTag::ShortArray(values))
}
