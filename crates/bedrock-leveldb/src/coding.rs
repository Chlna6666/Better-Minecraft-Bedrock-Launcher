use crate::error::{LevelDbError, Result};

pub(crate) const VALUE_TYPE_DELETION: u8 = 0;
pub(crate) const VALUE_TYPE_VALUE: u8 = 1;

pub(crate) fn put_varint32(mut value: u32, out: &mut Vec<u8>) {
    while value >= 0x80 {
        let byte = u8::try_from(value & 0x7f).expect("masked varint32 byte fits in u8");
        out.push(byte | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).expect("final varint32 byte fits in u8"));
}

pub(crate) fn get_varint32(input: &mut &[u8]) -> Result<u32> {
    let mut result = 0_u32;
    for index in 0..5 {
        let Some((&byte, rest)) = input.split_first() else {
            return Err(LevelDbError::corruption("truncated varint32".to_string()));
        };
        *input = rest;
        if index == 4 && byte > 0x0f {
            return Err(LevelDbError::corruption(
                "varint32 overflows u32".to_string(),
            ));
        }
        result |= u32::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok(result);
        }
    }
    Err(LevelDbError::corruption("varint32 is too long".to_string()))
}

pub(crate) fn get_varint64(input: &mut &[u8]) -> Result<u64> {
    let mut result = 0_u64;
    for index in 0..10 {
        let Some((&byte, rest)) = input.split_first() else {
            return Err(LevelDbError::corruption("truncated varint64".to_string()));
        };
        *input = rest;
        if index == 9 && byte > 0x01 {
            return Err(LevelDbError::corruption(
                "varint64 overflows u64".to_string(),
            ));
        }
        result |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok(result);
        }
    }
    Err(LevelDbError::corruption("varint64 is too long".to_string()))
}

pub(crate) fn put_varint64(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        let byte = u8::try_from(value & 0x7f).expect("masked varint64 byte fits in u8");
        out.push(byte | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).expect("final varint64 byte fits in u8"));
}

pub(crate) fn put_length_prefixed_slice(value: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| LevelDbError::invalid_argument("slice is too large".to_string()))?;
    put_varint32(len, out);
    out.extend_from_slice(value);
    Ok(())
}

pub(crate) fn get_length_prefixed_slice<'a>(input: &mut &'a [u8]) -> Result<&'a [u8]> {
    let len = usize::try_from(get_varint32(input)?)
        .map_err(|_| LevelDbError::corruption("length does not fit usize".to_string()))?;
    if input.len() < len {
        return Err(LevelDbError::corruption(
            "truncated length-prefixed slice".to_string(),
        ));
    }
    let (value, rest) = input.split_at(len);
    *input = rest;
    Ok(value)
}

pub(crate) fn crc32c(bytes: &[u8]) -> u32 {
    finalize_crc32c(update_crc32c(!0_u32, bytes))
}

pub(crate) fn masked_crc32c(chunks: &[&[u8]]) -> u32 {
    let mut crc = !0_u32;
    for chunk in chunks {
        crc = update_crc32c(crc, chunk);
    }
    mask_crc(finalize_crc32c(crc))
}

fn update_crc32c(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0x82f6_3b78 & mask);
        }
    }
    crc
}

const fn finalize_crc32c(crc: u32) -> u32 {
    !crc
}

pub(crate) const fn mask_crc(crc: u32) -> u32 {
    crc.rotate_right(15).wrapping_add(0xa282_ead8)
}

pub(crate) const fn unmask_crc(masked: u32) -> u32 {
    masked.wrapping_sub(0xa282_ead8).rotate_left(15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrips() {
        for value in [0, 1, 127, 128, 16_384, u32::MAX] {
            let mut encoded = Vec::new();
            put_varint32(value, &mut encoded);
            let mut input = encoded.as_slice();
            assert_eq!(get_varint32(&mut input).expect("decode"), value);
            assert!(input.is_empty());
        }
    }

    #[test]
    fn varint32_rejects_overflow_and_truncation() {
        let mut overflow = [0xff, 0xff, 0xff, 0xff, 0x10].as_slice();
        assert!(get_varint32(&mut overflow).is_err());

        let mut truncated = [0x80].as_slice();
        assert!(get_varint32(&mut truncated).is_err());
    }

    #[test]
    fn varint64_roundtrips() {
        for value in [0, 1, 127, 128, 16_384, u64::MAX] {
            let mut encoded = Vec::new();
            put_varint64(value, &mut encoded);
            let mut input = encoded.as_slice();
            assert_eq!(get_varint64(&mut input).expect("decode"), value);
            assert!(input.is_empty());
        }
    }

    #[test]
    fn varint64_rejects_overflow_and_truncation() {
        let mut overflow = [0xff; 10].as_slice();
        assert!(get_varint64(&mut overflow).is_err());

        let mut truncated = [0x80].as_slice();
        assert!(get_varint64(&mut truncated).is_err());
    }

    #[test]
    fn crc_mask_roundtrips() {
        let crc = crc32c(b"abc");
        assert_eq!(unmask_crc(mask_crc(crc)), crc);
    }
}
