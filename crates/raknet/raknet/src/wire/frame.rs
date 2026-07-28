//! 帧与数据报（FrameSet）编解码。
//!
//! 解码完全零拷贝：每个帧的 `payload` 是入站数据报 `Bytes` 的切片视图。

use crate::consts::*;
use crate::error::RakCodecError;
use crate::types::RakReliability;
use crate::wire::{Rd, put_u24_le};
use bytes::{BufMut, Bytes, BytesMut};

/// 拆分信息。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SplitInfo {
    /// 总片数。
    pub count: u32,
    /// 拆分组 ID。
    pub id: u16,
    /// 本片序号（0 起）。
    pub index: u32,
}

/// 单个帧。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub reliability: RakReliability,
    /// 可靠序号（u24 线上值，接收侧由上层展开为 u64）。
    pub reliable_index: u32,
    /// 序列序号。
    pub sequence_index: u32,
    /// 有序序号。
    pub order_index: u32,
    /// 有序通道。
    pub order_channel: u8,
    pub split: Option<SplitInfo>,
    pub payload: Bytes,
}

impl Frame {
    /// 编码后的总长度（帧头 + 载荷）。
    pub fn encoded_len(&self) -> usize {
        let r = self.reliability;
        1 + 2
            + if r.is_reliable() { 3 } else { 0 }
            + if r.is_sequenced() { 3 } else { 0 }
            + if r.is_ordered() || r.is_sequenced() {
                4
            } else {
                0
            }
            + if self.split.is_some() { 10 } else { 0 }
            + self.payload.len()
    }

    /// 编码进数据报缓冲。
    pub fn encode_into(&self, buf: &mut BytesMut) {
        let r = self.reliability;
        let mut flags = (r as u8) << 5;
        if self.split.is_some() {
            flags |= FLAG_SPLIT;
        }
        buf.put_u8(flags);
        buf.put_u16((self.payload.len() as u16) << 3);
        if r.is_reliable() {
            put_u24_le(buf, self.reliable_index);
        }
        if r.is_sequenced() {
            put_u24_le(buf, self.sequence_index);
        }
        if r.is_ordered() || r.is_sequenced() {
            put_u24_le(buf, self.order_index);
            buf.put_u8(self.order_channel);
        }
        if let Some(split) = &self.split {
            buf.put_u32(split.count);
            buf.put_u16(split.id);
            buf.put_u32(split.index);
        }
        buf.put_slice(&self.payload);
    }

    fn decode(rd: &mut Rd) -> Result<Self, RakCodecError> {
        let header = rd.u8()?;
        let reliability = RakReliability::try_from((header & 0xE0) >> 5)
            .map_err(|_| RakCodecError::Malformed("帧可靠性等级非法"))?;
        let payload_len = (rd.u16_be()? as usize).div_ceil(8);

        let mut reliable_index = 0;
        if reliability.is_reliable() {
            reliable_index = rd.u24_le()?;
        }
        let mut sequence_index = 0;
        if reliability.is_sequenced() {
            sequence_index = rd.u24_le()?;
        }
        let mut order_index = 0;
        let mut order_channel = 0;
        if reliability.is_ordered() || reliability.is_sequenced() {
            order_index = rd.u24_le()?;
            order_channel = rd.u8()?;
        }
        let split = if header & FLAG_SPLIT != 0 {
            let count = rd.u32_be()?;
            let id = rd.u16_be()?;
            let index = rd.u32_be()?;
            Some(SplitInfo { count, id, index })
        } else {
            None
        };
        let payload = rd.take(payload_len)?;

        Ok(Self {
            reliability,
            reliable_index,
            sequence_index,
            order_index,
            order_channel,
            split,
            payload,
        })
    }
}

/// 解码后的数据报。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameSet {
    /// 数据报序号（u24 线上值）。
    pub sequence: u32,
    pub frames: Vec<Frame>,
}

impl FrameSet {
    /// 判断数据报类别（首字节）。
    #[inline]
    pub fn is_frame_set(first: u8) -> bool {
        first & FLAG_VALID != 0 && first & (FLAG_ACK | FLAG_NACK) == 0
    }

    /// 零拷贝解码整个数据报。
    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        let flags = rd.u8()?;
        if !Self::is_frame_set(flags) {
            return Err(RakCodecError::Malformed("非 FrameSet 数据报"));
        }
        let sequence = rd.u24_le()?;
        let mut frames = Vec::with_capacity(4);
        while rd.remaining() > 0 {
            frames.push(Frame::decode(&mut rd)?);
        }
        Ok(Self { sequence, frames })
    }
}

/// 写入数据报头（标志位 + u24 序号）。
pub fn put_datagram_header(buf: &mut BytesMut, sequence: u32) {
    buf.put_u8(FLAG_VALID | FLAG_NEEDS_B_AND_AS);
    put_u24_le(buf, sequence);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(reliability: RakReliability, payload: &'static [u8]) -> Frame {
        Frame {
            reliability,
            reliable_index: 7,
            sequence_index: 8,
            order_index: 9,
            order_channel: 3,
            split: None,
            payload: Bytes::from_static(payload),
        }
    }

    #[test]
    fn frame_set_round_trip_all_reliabilities() {
        for rel in 0..=7u8 {
            let reliability = RakReliability::try_from(rel).unwrap();
            let f = frame(reliability, b"hello world");
            let mut buf = BytesMut::new();
            put_datagram_header(&mut buf, 0x123456);
            f.encode_into(&mut buf);
            assert_eq!(buf.len(), DGRAM_HEADER_SIZE + f.encoded_len());

            let set = FrameSet::decode(buf.freeze()).unwrap();
            assert_eq!(set.sequence, 0x123456);
            assert_eq!(set.frames.len(), 1);
            let d = &set.frames[0];
            assert_eq!(d.reliability, reliability);
            assert_eq!(&d.payload[..], b"hello world");
            if reliability.is_reliable() {
                assert_eq!(d.reliable_index, 7);
            }
            if reliability.is_sequenced() {
                assert_eq!(d.sequence_index, 8);
            }
            if reliability.is_ordered() || reliability.is_sequenced() {
                assert_eq!(d.order_index, 9);
                assert_eq!(d.order_channel, 3);
            }
        }
    }

    #[test]
    fn split_frame_round_trip() {
        let mut f = frame(RakReliability::Reliable, b"part");
        f.split = Some(SplitInfo {
            count: 3,
            id: 55,
            index: 2,
        });
        let mut buf = BytesMut::new();
        put_datagram_header(&mut buf, 1);
        f.encode_into(&mut buf);
        let set = FrameSet::decode(buf.freeze()).unwrap();
        assert_eq!(
            set.frames[0].split,
            Some(SplitInfo {
                count: 3,
                id: 55,
                index: 2
            })
        );
    }

    #[test]
    fn multiple_frames_in_one_datagram() {
        let mut buf = BytesMut::new();
        put_datagram_header(&mut buf, 2);
        for payload in [b"aaa".as_slice(), b"bbbb", b"c"] {
            let f = Frame {
                reliability: RakReliability::ReliableOrdered,
                reliable_index: 1,
                sequence_index: 0,
                order_index: 1,
                order_channel: 0,
                split: None,
                payload: Bytes::copy_from_slice(payload),
            };
            f.encode_into(&mut buf);
        }
        let set = FrameSet::decode(buf.freeze()).unwrap();
        assert_eq!(set.frames.len(), 3);
        assert_eq!(&set.frames[1].payload[..], b"bbbb");
    }

    #[test]
    fn decoded_payload_is_zero_copy() {
        let f = frame(RakReliability::Unreliable, b"zero-copy-payload");
        let mut buf = BytesMut::new();
        put_datagram_header(&mut buf, 3);
        f.encode_into(&mut buf);
        let datagram = buf.freeze();
        let set = FrameSet::decode(datagram.clone()).unwrap();
        let payload = &set.frames[0].payload;
        let d_ptr = datagram.as_ptr() as usize;
        let p_ptr = payload.as_ptr() as usize;
        assert!(p_ptr >= d_ptr && p_ptr < d_ptr + datagram.len());
    }

    #[test]
    fn truncated_frame_rejected() {
        let mut buf = BytesMut::new();
        put_datagram_header(&mut buf, 4);
        let f = frame(RakReliability::Reliable, b"data");
        f.encode_into(&mut buf);
        let full = buf.freeze();
        let cut = full.slice(..full.len() - 2);
        assert!(FrameSet::decode(cut).is_err());
    }
}
