//! BMCLANG2: little-endian header, shared sorted keys, locale-major values,
//! placeholder records, then a deduplicated NUL-terminated UTF-8 pool. The
//! encoder lives in `build/locales.rs`. No alignment, native layout, or pointer
//! casts are used.
//!
//! Header (32 bytes): magic, key count, locale count, part count, pool offset,
//! pool length, reserved (zero). Keys are 3-byte pool offsets to NUL-terminated
//! strings sorted by text. Values are 7 bytes: 3-byte text pool offset, 3-byte
//! first part index, and a 1-byte part count; a text offset of 0x00FF_FFFF
//! means the translation is absent. Parts are 6 bytes: 3-byte placeholder-name
//! pool offset and 3-byte placeholder byte offset inside the value text. The
//! runtime derives the positional argument index from the first occurrence of
//! the name within the value, which is the order the encoder used.

use super::super::TemplatePart;

const MAGIC: &[u8; 8] = b"BMCLANG2";
const HEADER_LEN: usize = 32;
const KEY_LEN: usize = 3;
const VALUE_LEN: usize = 7;
const PART_LEN: usize = 6;
const MISSING_OFFSET: usize = 0x00FF_FFFF;

pub(super) struct Catalog<'a> {
    keys: &'a [u8],
    values: &'a [u8],
    parts: &'a [u8],
    pool: &'a str,
    locale_count: usize,
}

pub(crate) struct Translation<'a> {
    pub text: &'a str,
    pub parts: Parts<'a>,
}

#[derive(Clone)]
pub(crate) struct Parts<'a> {
    text: &'a str,
    pool: &'a str,
    records: &'a [u8],
    record_index: usize,
    cursor: usize,
    finished: bool,
}

fn integer(bytes: &[u8], offset: usize) -> Option<usize> {
    let end = offset.checked_add(4)?;
    Some(u32::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?) as usize)
}

fn offset24(bytes: &[u8], offset: usize) -> Option<usize> {
    let end = offset.checked_add(3)?;
    let raw = bytes.get(offset..end)?;
    Some(u32::from_le_bytes([raw[0], raw[1], raw[2], 0]) as usize)
}

fn pool_string(pool: &str, offset: usize) -> Option<&str> {
    let bytes = pool.as_bytes();
    let tail = bytes.get(offset..)?;
    let length = tail.iter().position(|byte| *byte == 0)?;
    pool.get(offset..offset.checked_add(length)?)
}

impl<'a> Catalog<'a> {
    pub(super) fn read(bytes: &'a [u8]) -> Option<Self> {
        if bytes.get(..8)? != MAGIC {
            return None;
        }
        let key_count = integer(bytes, 8)?;
        let locale_count = integer(bytes, 12)?;
        let part_count = integer(bytes, 16)?;
        let pool_offset = integer(bytes, 20)?;
        let pool_len = integer(bytes, 24)?;
        if integer(bytes, 28)? != 0 {
            return None;
        }
        let keys_end = HEADER_LEN.checked_add(key_count.checked_mul(KEY_LEN)?)?;
        let values_end = keys_end.checked_add(
            key_count
                .checked_mul(locale_count)?
                .checked_mul(VALUE_LEN)?,
        )?;
        let parts_end = values_end.checked_add(part_count.checked_mul(PART_LEN)?)?;
        if pool_offset != parts_end {
            return None;
        }
        let pool_end = pool_offset.checked_add(pool_len)?;
        if pool_end != bytes.len() {
            return None;
        }
        let pool_bytes = bytes.get(pool_offset..pool_end)?;
        if pool_bytes.last().is_some_and(|last| *last != 0) {
            return None;
        }
        let pool = std::str::from_utf8(pool_bytes).ok()?;
        let keys = bytes.get(HEADER_LEN..keys_end)?;
        let mut previous: Option<&str> = None;
        for record in keys.chunks_exact(KEY_LEN) {
            let key = pool_string(pool, offset24(record, 0)?)?;
            if previous.is_some_and(|previous| previous >= key) {
                return None;
            }
            previous = Some(key);
        }
        Some(Self {
            keys,
            values: bytes.get(keys_end..values_end)?,
            parts: bytes.get(values_end..parts_end)?,
            pool,
            locale_count,
        })
    }

    pub(super) fn key_index(&self, key: &str) -> Option<usize> {
        let mut low = 0;
        let mut high = self.keys.len() / KEY_LEN;
        while low < high {
            let middle = low + (high - low) / 2;
            let start = middle.checked_mul(KEY_LEN)?;
            let record = self.keys.get(start..start.checked_add(KEY_LEN)?)?;
            let candidate = self.string(offset24(record, 0)?)?;
            match candidate.cmp(key) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    pub(super) fn translation(&self, locale: usize, index: usize) -> Option<Translation<'a>> {
        let key_count = self.keys.len() / KEY_LEN;
        if locale >= self.locale_count || index >= key_count {
            return None;
        }
        let value_index = locale.checked_mul(key_count)?.checked_add(index)?;
        let start = value_index.checked_mul(VALUE_LEN)?;
        let record = self.values.get(start..start.checked_add(VALUE_LEN)?)?;
        let text_offset = offset24(record, 0)?;
        if text_offset == MISSING_OFFSET {
            return None;
        }
        let text = self.string(text_offset)?;
        let first_part = offset24(record, 3)?;
        let part_count = usize::from(record[6]);
        let parts_start = first_part.checked_mul(PART_LEN)?;
        let parts_len = part_count.checked_mul(PART_LEN)?;
        let records = self
            .parts
            .get(parts_start..parts_start.checked_add(parts_len)?)?;
        Some(Translation {
            text,
            parts: Parts {
                text,
                pool: self.pool,
                records,
                record_index: 0,
                cursor: 0,
                finished: false,
            },
        })
    }

    fn string(&self, offset: usize) -> Option<&'a str> {
        pool_string(self.pool, offset)
    }
}

impl<'a> Parts<'a> {
    fn record(&self, index: usize) -> Option<&'a [u8]> {
        let start = index.checked_mul(PART_LEN)?;
        self.records.get(start..start.checked_add(PART_LEN)?)
    }

    fn name(&self, index: usize) -> Option<&'a str> {
        let record = self.record(index)?;
        pool_string(self.pool, offset24(record, 0)?)
    }

    fn name_start(&self, index: usize) -> Option<usize> {
        let record = self.record(index)?;
        offset24(record, 3)
    }

    /// Positional arguments follow the first occurrence of each placeholder
    /// name inside the value, matching the encoder that used to store the
    /// index explicitly.
    fn argument_index(&self, name: &str, current: usize) -> Option<usize> {
        let mut first = None;
        for index in 0..=current {
            if self.name(index)? == name {
                first = Some(index);
                break;
            }
        }
        let first = first?;
        let mut distinct = 0usize;
        for index in 0..first {
            let candidate = self.name(index)?;
            let mut repeated = false;
            for earlier in 0..index {
                if self.name(earlier)? == candidate {
                    repeated = true;
                    break;
                }
            }
            if !repeated {
                distinct = distinct.checked_add(1)?;
            }
        }
        Some(distinct)
    }

    fn placeholder(&mut self) -> Option<TemplatePart<'a>> {
        let name = self.name(self.record_index)?;
        let start = self.name_start(self.record_index)?;
        let end = start.checked_add(name.len())?;
        let open = start.checked_sub(2)?;
        let close = end.checked_add(2)?;
        if self.text.get(open..start)? != "{{"
            || self.text.get(end..close)? != "}}"
            || self.text.get(start..end)? != name
        {
            return None;
        }
        let part = TemplatePart {
            literal: self.text.get(self.cursor..open)?,
            placeholder: Some(name),
            argument_index: Some(self.argument_index(name, self.record_index)?),
        };
        self.record_index = self.record_index.checked_add(1)?;
        self.cursor = close;
        Some(part)
    }
}

impl<'a> Iterator for Parts<'a> {
    type Item = TemplatePart<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        if self.record_index >= self.records.len() / PART_LEN {
            self.finished = true;
            return Some(TemplatePart {
                literal: self.text.get(self.cursor..)?,
                placeholder: None,
                argument_index: None,
            });
        }
        let part = self.placeholder();
        if part.is_none() {
            self.finished = true;
        }
        part
    }
}

#[cfg(test)]
mod tests;
