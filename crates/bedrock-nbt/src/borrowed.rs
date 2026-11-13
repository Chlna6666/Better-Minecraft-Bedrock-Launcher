//! Borrowed Bedrock little-endian NBT decoding.

use crate::{Error, NbtRef, Result, constants::*};
use std::borrow::Cow;

pub(crate) fn parse_root(data: &[u8]) -> Result<NbtRef<'_>> {
    let mut input = data;
    let tag_type = take_u8(&mut input)?;
    if tag_type != TAG_COMPOUND {
        return Err(nbt_error("root NBT tag must be Compound"));
    }
    let _name = take_string(&mut input)?;
    parse_payload(&mut input, tag_type, 0)
}

fn parse_payload<'a>(input: &mut &'a [u8], tag_type: u8, depth: usize) -> Result<NbtRef<'a>> {
    if depth > MAX_NBT_DEPTH {
        return Err(nbt_error("NBT nesting exceeds maximum depth"));
    }
    match tag_type {
        TAG_END => Ok(NbtRef::End),
        TAG_BYTE => Ok(NbtRef::Byte(take_u8(input)? as i8)),
        TAG_SHORT => Ok(NbtRef::Short(i16::from_le_bytes(take_array(input)?))),
        TAG_INT => Ok(NbtRef::Int(i32::from_le_bytes(take_array(input)?))),
        TAG_LONG => Ok(NbtRef::Long(i64::from_le_bytes(take_array(input)?))),
        TAG_FLOAT => Ok(NbtRef::Float(f32::from_le_bytes(take_array(input)?))),
        TAG_DOUBLE => Ok(NbtRef::Double(f64::from_le_bytes(take_array(input)?))),
        TAG_BYTE_ARRAY => parse_byte_array(input),
        TAG_STRING => {
            let bytes = take_string_bytes(input)?;
            Ok(match std::str::from_utf8(bytes) {
                Ok(value) => NbtRef::String(Cow::Borrowed(value)),
                Err(_) => NbtRef::ByteString(Cow::Borrowed(bytes)),
            })
        }
        TAG_LIST => parse_list(input, depth),
        TAG_COMPOUND => parse_compound(input, depth),
        TAG_INT_ARRAY => parse_i32_array(input),
        TAG_LONG_ARRAY => parse_i64_array(input),
        TAG_SHORT_ARRAY => parse_i16_array(input),
        other => Err(nbt_error(&format!("unknown NBT tag type {other}"))),
    }
}

fn parse_list<'a>(input: &mut &'a [u8], depth: usize) -> Result<NbtRef<'a>> {
    let element_type = take_u8(input)?;
    let len = take_len(input, "list")?;
    if len > input.len() || (len > 0 && element_type == TAG_END) {
        return Err(nbt_error("invalid List length or nonempty End list"));
    }
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        values.push(parse_payload(input, element_type, depth.saturating_add(1))?);
    }
    Ok(NbtRef::List(values))
}

fn parse_compound<'a>(input: &mut &'a [u8], depth: usize) -> Result<NbtRef<'a>> {
    let mut values = Vec::new();
    loop {
        let tag_type = take_u8(input)?;
        if tag_type == TAG_END {
            return Ok(NbtRef::Compound(values));
        }
        let name = Cow::Borrowed(take_string(input)?);
        let value = parse_payload(input, tag_type, depth.saturating_add(1))?;
        values.push((name, value));
    }
}

fn parse_byte_array<'a>(input: &mut &'a [u8]) -> Result<NbtRef<'a>> {
    let len = take_byte_len(input, "ByteArray")?;
    let values = take(input, len)?.iter().map(|byte| *byte as i8).collect();
    Ok(NbtRef::ByteArray(Cow::Owned(values)))
}

fn parse_i16_array<'a>(input: &mut &'a [u8]) -> Result<NbtRef<'a>> {
    let len = take_len(input, "ShortArray")?;
    let bytes = take(
        input,
        len.checked_mul(2)
            .ok_or_else(|| nbt_error("array length overflow"))?,
    )?;
    let values = bytes
        .chunks_exact(2)
        .map(|part| i16::from_le_bytes(std::array::from_fn(|index| part[index])))
        .collect();
    Ok(NbtRef::ShortArray(Cow::Owned(values)))
}

fn parse_i32_array<'a>(input: &mut &'a [u8]) -> Result<NbtRef<'a>> {
    let len = take_len(input, "IntArray")?;
    let bytes = take(
        input,
        len.checked_mul(4)
            .ok_or_else(|| nbt_error("array length overflow"))?,
    )?;
    let values = bytes
        .chunks_exact(4)
        .map(|part| i32::from_le_bytes(std::array::from_fn(|index| part[index])))
        .collect();
    Ok(NbtRef::IntArray(Cow::Owned(values)))
}

fn parse_i64_array<'a>(input: &mut &'a [u8]) -> Result<NbtRef<'a>> {
    let len = take_len(input, "LongArray")?;
    let bytes = take(
        input,
        len.checked_mul(8)
            .ok_or_else(|| nbt_error("array length overflow"))?,
    )?;
    let values = bytes
        .chunks_exact(8)
        .map(|part| i64::from_le_bytes(std::array::from_fn(|index| part[index])))
        .collect();
    Ok(NbtRef::LongArray(Cow::Owned(values)))
}

fn take_string<'a>(input: &mut &'a [u8]) -> Result<&'a str> {
    std::str::from_utf8(take_string_bytes(input)?).map_err(|error| nbt_error(&error.to_string()))
}

fn take_string_bytes<'a>(input: &mut &'a [u8]) -> Result<&'a [u8]> {
    let len = usize::from(u16::from_le_bytes(take_array(input)?));
    take(input, len)
}

fn take_len(input: &mut &[u8], name: &str) -> Result<usize> {
    let len = i32::from_le_bytes(take_array(input)?);
    if name == "list" && len <= 0 {
        return Ok(0);
    }
    let len = usize::try_from(len).map_err(|_| nbt_error(&format!("{name} length is negative")))?;
    if len > MAX_NBT_CONTAINER_LENGTH {
        return Err(nbt_error(&format!("{name} length exceeds limit")));
    }
    Ok(len)
}

fn take_byte_len(input: &mut &[u8], name: &str) -> Result<usize> {
    let len = take_len(input, name)?;
    if len > MAX_NBT_BYTE_LENGTH {
        return Err(nbt_error(&format!("{name} length exceeds byte limit")));
    }
    Ok(len)
}

fn take_u8(input: &mut &[u8]) -> Result<u8> {
    take(input, 1)?
        .first()
        .copied()
        .ok_or_else(|| nbt_error("NBT byte is truncated"))
}

fn take_array<const N: usize>(input: &mut &[u8]) -> Result<[u8; N]> {
    take(input, N)?
        .try_into()
        .map_err(|_| nbt_error("numeric NBT value is truncated"))
}

fn take<'a>(input: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
    if input.len() < len {
        return Err(nbt_error("NBT payload is truncated"));
    }
    let (value, rest) = input.split_at(len);
    *input = rest;
    Ok(value)
}

fn nbt_error(message: &str) -> Error {
    Error::Nbt(message.to_string())
}
