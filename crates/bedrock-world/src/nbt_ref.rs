use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtRef;
use std::borrow::Cow;

const MAX_DEPTH: usize = 128;
const MAX_CONTAINER_LEN: usize = 134_217_728;
const MAX_BYTE_LEN: usize = 32 * 1024 * 1024;

const TAG_END: u8 = 0;
const TAG_BYTE: u8 = 1;
const TAG_SHORT: u8 = 2;
const TAG_INT: u8 = 3;
const TAG_LONG: u8 = 4;
const TAG_FLOAT: u8 = 5;
const TAG_DOUBLE: u8 = 6;
const TAG_BYTE_ARRAY: u8 = 7;
const TAG_STRING: u8 = 8;
const TAG_LIST: u8 = 9;
const TAG_COMPOUND: u8 = 10;
const TAG_INT_ARRAY: u8 = 11;
const TAG_LONG_ARRAY: u8 = 12;
const TAG_SHORT_ARRAY: u8 = 100;

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
    if depth > MAX_DEPTH {
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
        TAG_STRING => Ok(NbtRef::String(Cow::Borrowed(take_string(input)?))),
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
    let values = (0..len)
        .map(|_| take_array(input).map(i16::from_le_bytes))
        .collect::<Result<Vec<_>>>()?;
    Ok(NbtRef::ShortArray(Cow::Owned(values)))
}

fn parse_i32_array<'a>(input: &mut &'a [u8]) -> Result<NbtRef<'a>> {
    let len = take_len(input, "IntArray")?;
    let values = (0..len)
        .map(|_| take_array(input).map(i32::from_le_bytes))
        .collect::<Result<Vec<_>>>()?;
    Ok(NbtRef::IntArray(Cow::Owned(values)))
}

fn parse_i64_array<'a>(input: &mut &'a [u8]) -> Result<NbtRef<'a>> {
    let len = take_len(input, "LongArray")?;
    let values = (0..len)
        .map(|_| take_array(input).map(i64::from_le_bytes))
        .collect::<Result<Vec<_>>>()?;
    Ok(NbtRef::LongArray(Cow::Owned(values)))
}

fn take_string<'a>(input: &mut &'a [u8]) -> Result<&'a str> {
    let len = usize::from(u16::from_le_bytes(take_array(input)?));
    std::str::from_utf8(take(input, len)?).map_err(|error| nbt_error(&error.to_string()))
}

fn take_len(input: &mut &[u8], name: &str) -> Result<usize> {
    let len = i32::from_le_bytes(take_array(input)?);
    let len = usize::try_from(len).map_err(|_| nbt_error(&format!("{name} length is negative")))?;
    if len > MAX_CONTAINER_LEN {
        return Err(nbt_error(&format!("{name} length exceeds limit")));
    }
    Ok(len)
}

fn take_byte_len(input: &mut &[u8], name: &str) -> Result<usize> {
    let len = take_len(input, name)?;
    if len > MAX_BYTE_LEN {
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

fn nbt_error(message: &str) -> BedrockWorldError {
    BedrockWorldError::Nbt(message.to_string())
}
