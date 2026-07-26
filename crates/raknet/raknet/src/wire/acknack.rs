//! ACK / NACK 编解码。
//!
//! 线上格式：`[flags][record_count: u16be]` 后跟记录；
//! 单值记录 `[1][u24]`，范围记录 `[0][start u24][end u24]`（含端点）。
//! 内部以「闭区间范围列表」表示，避免物化巨大的序号数组。

use crate::consts::*;
use crate::error::RakCodecError;
use crate::wire::{Rd, put_u24_le};
use bytes::{BufMut, Bytes, BytesMut};

/// 已排序、互不重叠的闭区间集合（u24 线上值）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AckRanges {
    pub ranges: Vec<(u32, u32)>,
}

impl AckRanges {
    /// 由已排序去重的序号列表构建（相邻序号合并为范围）。
    pub fn from_sorted(sorted: &[u32]) -> Self {
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for &seq in sorted {
            match ranges.last_mut() {
                Some((_, end)) if seq == *end + 1 => *end = seq,
                Some((_, end)) if seq <= *end => {}
                _ => ranges.push((seq, seq)),
            }
        }
        Self { ranges }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// 编码为 ACK（`is_nack = false`）或 NACK 数据报。
    pub fn encode(&self, is_nack: bool) -> Bytes {
        let flag = FLAG_VALID | if is_nack { FLAG_NACK } else { FLAG_ACK };
        let mut buf = BytesMut::with_capacity(3 + self.ranges.len() * 7);
        buf.put_u8(flag);
        buf.put_u16(self.ranges.len().min(u16::MAX as usize) as u16);
        for &(start, end) in self.ranges.iter().take(u16::MAX as usize) {
            if start == end {
                buf.put_u8(1);
                put_u24_le(&mut buf, start);
            } else {
                buf.put_u8(0);
                put_u24_le(&mut buf, start);
                put_u24_le(&mut buf, end);
            }
        }
        buf.freeze()
    }

    /// 解码，返回（范围集合, 是否 NACK）。
    pub fn decode(buf: Bytes) -> Result<(Self, bool), RakCodecError> {
        let mut rd = Rd::new(buf);
        let flags = rd.u8()?;
        if flags & FLAG_VALID == 0 || (flags & (FLAG_ACK | FLAG_NACK)).count_ones() != 1 {
            return Err(RakCodecError::Malformed("ACK/NACK 标志位非法"));
        }
        let is_nack = flags & FLAG_NACK != 0;
        let count = rd.u16_be()? as usize;
        if count > MAX_ACK_RECORDS {
            return Err(RakCodecError::Malformed("ACK 记录数超限"));
        }
        let mut ranges = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            if rd.u8()? != 0 {
                let seq = rd.u24_le()?;
                ranges.push((seq, seq));
            } else {
                let start = rd.u24_le()?;
                let end = rd.u24_le()?;
                if end < start {
                    return Err(RakCodecError::Malformed("ACK 范围终点小于起点"));
                }
                ranges.push((start, end));
            }
        }
        Ok((Self { ranges }, is_nack))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_adjacent_sequences() {
        let ranges = AckRanges::from_sorted(&[1, 2, 3, 5, 7, 8]);
        assert_eq!(ranges.ranges, vec![(1, 3), (5, 5), (7, 8)]);
    }

    #[test]
    fn round_trip_preserves_range_endpoints() {
        // 回归：旧实现解码 `start..end` 丢失末端序号。
        let ranges = AckRanges { ranges: vec![(10, 20), (30, 30), (40, 41)] };
        for is_nack in [false, true] {
            let (decoded, nack) = AckRanges::decode(ranges.encode(is_nack)).unwrap();
            assert_eq!(nack, is_nack);
            assert_eq!(decoded, ranges);
        }
    }

    #[test]
    fn empty_ack_round_trip() {
        let ranges = AckRanges::default();
        let (decoded, _) = AckRanges::decode(ranges.encode(false)).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn oversized_record_count_rejected() {
        let mut buf = BytesMut::new();
        buf.put_u8(FLAG_VALID | FLAG_ACK);
        buf.put_u16(u16::MAX);
        assert!(AckRanges::decode(buf.freeze()).is_err());
    }

    #[test]
    fn inverted_range_rejected() {
        let mut buf = BytesMut::new();
        buf.put_u8(FLAG_VALID | FLAG_ACK);
        buf.put_u16(1);
        buf.put_u8(0);
        put_u24_le(&mut buf, 9);
        put_u24_le(&mut buf, 3);
        assert!(AckRanges::decode(buf.freeze()).is_err());
    }
}
