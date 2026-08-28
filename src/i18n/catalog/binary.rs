//! BMCLANG1: little-endian header, shared sorted keys, locale-major values,
//! placeholder offsets, then a deduplicated UTF-8 pool. The encoder lives in
//! `build/locales.rs`. No alignment, native layout, or pointer casts are used.
//!
//! After the 8-byte magic, all fields are u32: header (key count, locale count,
//! pool offset), key (pool offset, length), value (pool offset, length, first
//! part, part count), part (name byte offset within the value, length, argument
//! index). A value pool offset of u32::MAX means that translation is absent.

use super::super::TemplatePart;

const HEADER_LEN: usize = 20;
const KEY_LEN: usize = 8;
const VALUE_LEN: usize = 16;
const PART_LEN: usize = 12;

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
    records: &'a [u8],
    cursor: usize,
    finished: bool,
}

fn integer(bytes: &[u8], offset: usize) -> Option<usize> {
    let end = offset.checked_add(4)?;
    Some(u32::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?) as usize)
}

impl<'a> Catalog<'a> {
    pub(super) fn read(bytes: &'a [u8]) -> Option<Self> {
        if bytes.get(..8)? != b"BMCLANG1" {
            return None;
        }
        let key_count = integer(bytes, 8)?;
        let locale_count = integer(bytes, 12)?;
        let pool_offset = integer(bytes, 16)?;
        let keys_end = HEADER_LEN.checked_add(key_count.checked_mul(KEY_LEN)?)?;
        let values_end = keys_end.checked_add(
            key_count
                .checked_mul(locale_count)?
                .checked_mul(VALUE_LEN)?,
        )?;
        let parts = bytes.get(values_end..pool_offset)?;
        if parts.len() % PART_LEN != 0 {
            return None;
        }
        Some(Self {
            keys: bytes.get(HEADER_LEN..keys_end)?,
            values: bytes.get(keys_end..values_end)?,
            parts,
            pool: std::str::from_utf8(bytes.get(pool_offset..)?).ok()?,
            locale_count,
        })
    }

    pub(super) fn key_index(&self, key: &str) -> Option<usize> {
        let mut low = 0;
        let mut high = self.keys.len() / KEY_LEN;
        while low < high {
            let middle = low + (high - low) / 2;
            let record = self.keys.get(middle * KEY_LEN..)?;
            let candidate = self.string(record)?;
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
        let offset = locale
            .checked_mul(key_count)?
            .checked_add(index)?
            .checked_mul(VALUE_LEN)?;
        let record = self.values.get(offset..offset.checked_add(VALUE_LEN)?)?;
        if integer(record, 0)? == u32::MAX as usize {
            return None;
        }
        let text = self.string(record)?;
        let start = integer(record, 8)?.checked_mul(PART_LEN)?;
        let length = integer(record, 12)?.checked_mul(PART_LEN)?;
        let records = self.parts.get(start..start.checked_add(length)?)?;
        Some(Translation {
            text,
            parts: Parts {
                text,
                records,
                cursor: 0,
                finished: false,
            },
        })
    }

    fn string(&self, record: &[u8]) -> Option<&'a str> {
        let start = integer(record, 0)?;
        let length = integer(record, 4)?;
        self.pool.get(start..start.checked_add(length)?)
    }
}

impl<'a> Parts<'a> {
    fn placeholder(&mut self) -> Option<TemplatePart<'a>> {
        let record = self.records.get(..PART_LEN)?;
        let start = integer(record, 0)?;
        let end = start.checked_add(integer(record, 4)?)?;
        let open = start.checked_sub(2)?;
        let close = end.checked_add(2)?;
        if self.text.get(open..start)? != "{{" || self.text.get(end..close)? != "}}" {
            return None;
        }
        let part = TemplatePart {
            literal: self.text.get(self.cursor..open)?,
            placeholder: Some(self.text.get(start..end)?),
            argument_index: Some(integer(record, 8)?),
        };
        self.records = self.records.get(PART_LEN..)?;
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
        if self.records.is_empty() {
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
