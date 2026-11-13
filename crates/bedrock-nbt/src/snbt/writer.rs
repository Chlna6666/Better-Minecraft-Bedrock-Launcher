use crate::{Error, NbtTag, Result, constants::MAX_NBT_DEPTH};
use std::fmt::Write;

/// Formats a value as classic SNBT, quoting all keys and strings.
///
/// # Errors
/// Rejects malformed lists, limits exceeded, End, raw byte strings and ShortArray;
/// these binary extensions have no standard SNBT spelling.
pub fn to_string(tag: &NbtTag) -> Result<String> {
    crate::validation::validate_nbt_tag_for_write(tag, 0, "<root>")?;
    let mut output = String::new();
    value(&mut output, tag, 0)?;
    Ok(output)
}

fn value(output: &mut String, tag: &NbtTag, depth: usize) -> Result<()> {
    if depth > MAX_NBT_DEPTH {
        return Err(Error::Validation("SNBT exceeds depth limit".into()));
    }
    match tag {
        NbtTag::Byte(n) => {
            let _ = write!(output, "{n}b");
        }
        NbtTag::Short(n) => {
            let _ = write!(output, "{n}s");
        }
        NbtTag::Int(n) => {
            let _ = write!(output, "{n}");
        }
        NbtTag::Long(n) => {
            let _ = write!(output, "{n}L");
        }
        NbtTag::Float(n) => {
            let _ = write!(output, "{n}f");
        }
        NbtTag::Double(n) => {
            let _ = write!(output, "{n}d");
        }
        NbtTag::String(text) => quoted(output, text),
        NbtTag::ByteArray(values) => array(output, "B", "b", values),
        NbtTag::IntArray(values) => array(output, "I", "", values),
        NbtTag::LongArray(values) => array(output, "L", "L", values),
        NbtTag::List(values) => {
            output.push('[');
            for (index, item) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                value(output, item, depth + 1)?;
            }
            output.push(']');
        }
        NbtTag::Compound(values) => {
            output.push('{');
            for (index, (key, item)) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                quoted(output, key);
                output.push(':');
                value(output, item, depth + 1)?;
            }
            output.push('}');
        }
        NbtTag::ByteString(_) | NbtTag::ShortArray(_) | NbtTag::End => {
            return Err(Error::Validation(
                "binary tag has no standard SNBT representation".into(),
            ));
        }
    }
    Ok(())
}

fn quoted(output: &mut String, text: &str) {
    output.push('"');
    for part in text.split_inclusive(['"', '\\']) {
        if let Some(body) = part.strip_suffix(['"', '\\']) {
            output.push_str(body);
            output.push('\\');
            output.push_str(&part[body.len()..]);
        } else {
            output.push_str(part);
        }
    }
    output.push('"');
}

fn array(output: &mut String, kind: &str, suffix: &str, values: &[impl std::fmt::Display]) {
    let _ = write!(output, "[{kind};");
    for (index, number) in values.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(output, "{number}{suffix}");
    }
    output.push(']');
}
