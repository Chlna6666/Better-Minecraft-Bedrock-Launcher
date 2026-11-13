use crate::constants::*;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

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
    /// `TAG_String` payload containing non-UTF-8 bytes, used by fields such as actor `StorageKey`.
    ByteString(Vec<u8>),
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

impl NbtTag {
    /// Returns a byte, short, int, or in-range long tag as an `i32`.
    ///
    /// Other tag types and out-of-range long values return `None`.
    #[must_use]
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Byte(value) => Some(i32::from(*value)),
            Self::Short(value) => Some(i32::from(*value)),
            Self::Int(value) => Some(*value),
            Self::Long(value) => i32::try_from(*value).ok(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Borrow-friendly view of an NBT tag.
pub enum NbtRef<'a> {
    /// `TAG_End`.
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
    /// Non-UTF-8 `TAG_String` payload.
    ByteString(Cow<'a, [u8]>),
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
            Self::ByteString(value) => NbtTag::ByteString(value.to_vec()),
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

/// Nbt tags equal for write.
pub fn tags_equal_for_write(left: &NbtTag, right: &NbtTag) -> bool {
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
        (NbtTag::ByteString(left), NbtTag::ByteString(right)) => left == right,
        (NbtTag::List(left), NbtTag::List(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| tags_equal_for_write(left, right))
        }
        (NbtTag::Compound(left), NbtTag::Compound(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right
                        .get(key)
                        .is_some_and(|right_value| tags_equal_for_write(value, right_value))
                })
        }
        (NbtTag::IntArray(left), NbtTag::IntArray(right)) => left == right,
        (NbtTag::LongArray(left), NbtTag::LongArray(right)) => left == right,
        (NbtTag::ShortArray(left), NbtTag::ShortArray(right)) => left == right,
        _ => false,
    }
}

pub(crate) fn discriminant(tag: &NbtTag) -> u8 {
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
        NbtTag::ByteString(_) => TAG_STRING,
        NbtTag::List(_) => TAG_LIST,
        NbtTag::Compound(_) => TAG_COMPOUND,
        NbtTag::IntArray(_) => TAG_INT_ARRAY,
        NbtTag::LongArray(_) => TAG_LONG_ARRAY,
        NbtTag::ShortArray(_) => TAG_SHORT_ARRAY,
    }
}
