use crate::{Error, NbtTag, Result, constants::*, tag::discriminant};

/// Validates that a tag can be written as a root compound.
///
/// # Errors
/// Rejects non-compound roots, invalid lists/End children and values exceeding limits.
pub fn validate_root_for_write(tag: &NbtTag) -> Result<()> {
    match tag {
        NbtTag::Compound(_) => validate_nbt_tag_for_write(tag, 0, "<root>"),
        _ => Err(Error::Validation(
            "root NBT tag must be Compound".to_string(),
        )),
    }
}

pub(crate) fn validate_nbt_tag_for_write(tag: &NbtTag, depth: usize, path: &str) -> Result<()> {
    if depth > MAX_NBT_DEPTH {
        return Err(Error::Validation(format!(
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
        NbtTag::ByteString(value) => validate_string_bytes(value, path),
        NbtTag::ByteArray(values) => {
            if values.len() > MAX_NBT_BYTE_LENGTH {
                return Err(Error::Validation(format!(
                    "ByteArray exceeds byte limit: {path}"
                )));
            }
            Ok(())
        }
        NbtTag::IntArray(values) => validate_array_len(values.len(), path),
        NbtTag::LongArray(values) => validate_array_len(values.len(), path),
        NbtTag::ShortArray(values) => validate_array_len(values.len(), path),
        NbtTag::List(values) => {
            validate_array_len(values.len(), path)?;
            let Some(first) = values.first() else {
                return Ok(());
            };
            let first_type = discriminant(first);
            if first_type == TAG_END {
                return Err(Error::Validation(format!(
                    "NBT List cannot contain End: {path}"
                )));
            }
            for (index, value) in values.iter().enumerate() {
                if discriminant(value) != first_type {
                    return Err(Error::Validation(format!(
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
                if matches!(value, NbtTag::End) {
                    return Err(Error::Validation(format!(
                        "Compound cannot contain End: {path}.{key}"
                    )));
                }
                validate_nbt_tag_for_write(value, depth + 1, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
    }
}

pub(crate) fn validate_string(value: &str, path: &str) -> Result<()> {
    validate_string_bytes(value.as_bytes(), path)
}

pub(crate) fn validate_string_bytes(value: &[u8], path: &str) -> Result<()> {
    if value.len() > MAX_NBT_STRING_BYTES {
        return Err(Error::Validation(format!(
            "NBT string is too long at {path}"
        )));
    }
    Ok(())
}

pub(crate) fn validate_array_len(len: usize, path: &str) -> Result<()> {
    if len > MAX_NBT_CONTAINER_LENGTH {
        return Err(Error::Validation(format!(
            "NBT container is too large at {path}"
        )));
    }
    if len > i32::MAX as usize {
        return Err(Error::Validation(format!(
            "NBT container length exceeds i32 at {path}"
        )));
    }
    Ok(())
}

pub(crate) fn ensure_depth(depth: usize, tag_type: u8) -> Result<()> {
    if depth > MAX_NBT_DEPTH {
        return Err(Error::Nbt(format!(
            "NBT nesting is too deep at tag {tag_type}"
        )));
    }
    Ok(())
}
