use crate::{Error, Result, constants::*, validation::ensure_depth};
use std::ops::ControlFlow;

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
    /// Non-UTF-8 `TAG_String` variant.
    ByteString {
        /// Named Bedrock value or identifier.
        name: Option<&'a str>,
        /// Exact bytes stored in the string payload.
        value: &'a [u8],
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

pub(crate) fn parse_nbt_events(data: &[u8]) -> Result<Vec<NbtEvent<'_>>> {
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
        return Err(Error::Nbt("root NBT tag must be Compound".to_string()));
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
        TAG_STRING => {
            let value = reader.read_string_bytes_ref()?;
            if let Ok(value) = std::str::from_utf8(value) {
                emit!(NbtEvent::String { name, value });
            } else {
                emit!(NbtEvent::ByteString { name, value });
            }
        }
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
            return Err(Error::Nbt(format!("unknown NBT tag type: {tag_type}")));
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
    if len > reader.data.len() - reader.offset || (len > 0 && element_type == TAG_END) {
        return Err(Error::Nbt(
            "invalid List length or nonempty End list".into(),
        ));
    }
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
            .ok_or_else(|| Error::Nbt("NBT slice range overflowed".to_string()))?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| Error::Nbt("NBT payload is truncated".to_string()))?;
        self.offset = end;
        Ok(bytes)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(*self
            .take(1)?
            .first()
            .ok_or_else(|| Error::Nbt("NBT byte is truncated".to_string()))?)
    }

    fn read_i8(&mut self) -> Result<i8> {
        Ok(self.read_u8()? as i8)
    }

    fn read_i16(&mut self) -> Result<i16> {
        let bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| Error::Nbt("NBT i16 is truncated".to_string()))?;
        Ok(i16::from_le_bytes(bytes))
    }

    fn read_i32(&mut self) -> Result<i32> {
        let bytes: [u8; 4] = self
            .take(4)?
            .try_into()
            .map_err(|_| Error::Nbt("NBT i32 is truncated".to_string()))?;
        Ok(i32::from_le_bytes(bytes))
    }

    fn read_i64(&mut self) -> Result<i64> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| Error::Nbt("NBT i64 is truncated".to_string()))?;
        Ok(i64::from_le_bytes(bytes))
    }

    fn read_f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.read_i32()? as u32))
    }

    fn read_f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.read_i64()? as u64))
    }

    fn read_string_ref(&mut self) -> Result<&'a str> {
        Ok(std::str::from_utf8(self.read_string_bytes_ref()?)?)
    }

    fn read_string_bytes_ref(&mut self) -> Result<&'a [u8]> {
        let len_bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| Error::Nbt("NBT string length is truncated".to_string()))?;
        let len = u16::from_le_bytes(len_bytes) as usize;
        self.take(len)
    }

    fn read_container_length(&mut self, name: &str) -> Result<usize> {
        let len = self.read_i32()?;
        if name == "List" && len <= 0 {
            return Ok(0);
        }
        if len < 0 {
            return Err(Error::Nbt(format!("{name} has negative length")));
        }
        let len =
            usize::try_from(len).map_err(|_| Error::Nbt(format!("{name} length overflow")))?;
        if len > MAX_NBT_CONTAINER_LENGTH {
            return Err(Error::Nbt(format!("{name} is too large")));
        }
        Ok(len)
    }

    fn read_byte_length(&mut self, name: &str) -> Result<usize> {
        let len = self.read_container_length(name)?;
        if len > MAX_NBT_BYTE_LENGTH {
            return Err(Error::Nbt(format!("{name} is too large")));
        }
        Ok(len)
    }

    fn take_array_bytes(&mut self, len: usize, width: usize, name: &str) -> Result<&'a [u8]> {
        let byte_len = len
            .checked_mul(width)
            .ok_or_else(|| Error::Nbt(format!("{name} byte length overflow")))?;
        self.take(byte_len)
    }
}
