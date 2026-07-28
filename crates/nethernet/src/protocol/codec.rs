//! 零拷贝读写基元。
//!
//! [`Rd`] 基于 `Bytes`：`take` 返回底层缓冲的引用计数视图，不发生拷贝。
//! 所有读取都做边界检查，绝不 panic。

use crate::error::{NethernetError, Result};
use bytes::{BufMut, Bytes, BytesMut};

/// 零拷贝读取器。
pub struct Rd {
    buf: Bytes,
    pos: usize,
}

impl Rd {
    #[must_use]
    pub fn new(buf: Bytes) -> Self {
        Self { buf, pos: 0 }
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    fn check(&self, needed: usize) -> Result<()> {
        if self.remaining() < needed {
            Err(NethernetError::Truncated {
                needed,
                remaining: self.remaining(),
            })
        } else {
            Ok(())
        }
    }

    /// 零拷贝取 `len` 字节。
    pub fn take(&mut self, len: usize) -> Result<Bytes> {
        self.check(len)?;
        let out = self.buf.slice(self.pos..self.pos + len);
        self.pos += len;
        Ok(out)
    }

    /// 跳过 `len` 字节。
    pub fn skip(&mut self, len: usize) -> Result<()> {
        self.check(len)?;
        self.pos += len;
        Ok(())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.check(N)?;
        let mut out = [0_u8; N];
        out.copy_from_slice(&self.buf[self.pos..self.pos + N]);
        self.pos += N;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        self.check(1)?;
        let value = self.buf[self.pos];
        self.pos += 1;
        Ok(value)
    }

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn u16_le(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array::<2>()?))
    }

    pub fn u32_le(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array::<4>()?))
    }

    pub fn i32_le(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array::<4>()?))
    }

    pub fn u64_le(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array::<8>()?))
    }

    /// LEB128 varuint32（最多 5 字节）。
    pub fn var_u32(&mut self) -> Result<u32> {
        let mut value = 0_u32;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(NethernetError::protocol("varuint32 未在五字节内结束"))
    }

    /// zigzag varint32。
    pub fn var_i32(&mut self) -> Result<i32> {
        let value = self.var_u32()?;
        #[allow(clippy::cast_possible_wrap)]
        let magnitude = (value >> 1) as i32;
        let sign = 0_i32.wrapping_sub(i32::from(value & 1 != 0));
        Ok(magnitude ^ sign)
    }

    /// varuint32 长度前缀的 UTF-8 字符串。
    pub fn var_string(&mut self) -> Result<String> {
        let length = self.var_u32()? as usize;
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| NethernetError::protocol(format!("字符串不是 UTF-8：{error}")))
    }

    /// u8 长度前缀的 UTF-8 字符串（ServerData v4）。
    pub fn u8_string(&mut self) -> Result<String> {
        let length = usize::from(self.u8()?);
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| NethernetError::protocol(format!("字符串不是 UTF-8：{error}")))
    }
}

/// 写入 LEB128 varuint32。
pub fn put_var_u32(buf: &mut BytesMut, mut value: u32) {
    while value >= 0x80 {
        #[allow(clippy::cast_possible_truncation)]
        buf.put_u8((value as u8) | 0x80);
        value >>= 7;
    }
    #[allow(clippy::cast_possible_truncation)]
    buf.put_u8(value as u8);
}

/// 写入 zigzag varint32。
pub fn put_var_i32(buf: &mut BytesMut, value: i32) {
    #[allow(clippy::cast_sign_loss)]
    let mut encoded = (value as u32) << 1;
    if value < 0 {
        encoded = !encoded;
    }
    put_var_u32(buf, encoded);
}

/// 写入 varuint32 长度前缀的字节串。
pub fn put_var_bytes(buf: &mut BytesMut, data: &[u8]) -> Result<()> {
    let length =
        u32::try_from(data.len()).map_err(|_| NethernetError::protocol("字节串长度超过 u32"))?;
    put_var_u32(buf, length);
    buf.put_slice(data);
    Ok(())
}

/// 写入 u32 长度前缀的字节串。
pub fn put_u32_bytes(buf: &mut BytesMut, data: &[u8]) -> Result<()> {
    let length =
        u32::try_from(data.len()).map_err(|_| NethernetError::protocol("字节串长度超过 u32"))?;
    buf.put_u32_le(length);
    buf.put_slice(data);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_checked() {
        let mut rd = Rd::new(Bytes::from_static(&[1, 2]));
        assert_eq!(rd.u8().unwrap(), 1);
        assert!(rd.u32_le().is_err());
        assert_eq!(rd.remaining(), 1);
    }

    #[test]
    fn varint_round_trip_boundaries() {
        for value in [0_i32, 1, -1, i32::MAX, i32::MIN, 12345, -12345] {
            let mut buf = BytesMut::new();
            put_var_i32(&mut buf, value);
            let mut rd = Rd::new(buf.freeze());
            assert_eq!(rd.var_i32().unwrap(), value, "值 {value} 未能往返");
            assert!(rd.is_empty());
        }
    }

    #[test]
    fn varuint_round_trip() {
        for value in [0_u32, 127, 128, 16383, 16384, u32::MAX] {
            let mut buf = BytesMut::new();
            put_var_u32(&mut buf, value);
            let mut rd = Rd::new(buf.freeze());
            assert_eq!(rd.var_u32().unwrap(), value);
        }
    }

    #[test]
    fn varuint_rejects_overlong_encoding() {
        let mut rd = Rd::new(Bytes::from_static(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]));
        assert!(rd.var_u32().is_err());
    }

    #[test]
    fn take_is_zero_copy() {
        let src = Bytes::from(vec![7_u8; 64]);
        let mut rd = Rd::new(src.clone());
        let slice = rd.take(16).unwrap();
        let base = src.as_ptr() as usize;
        let ptr = slice.as_ptr() as usize;
        assert!(ptr >= base && ptr < base + src.len());
    }

    #[test]
    fn var_string_round_trip() {
        let mut buf = BytesMut::new();
        put_var_bytes(&mut buf, "世界".as_bytes()).unwrap();
        let mut rd = Rd::new(buf.freeze());
        assert_eq!(rd.var_string().unwrap(), "世界");
    }
}
