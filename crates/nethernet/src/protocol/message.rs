//! 数据通道消息的分片与重组。
//!
//! 每条数据通道消息以一个 u8 起始，表示**其后还剩多少片**：
//! 首片为 `总片数 - 1`，末片为 `0`。因此单条消息最多 256 片。

use crate::consts::{MAX_MESSAGE_SIZE, MAX_SEGMENT_PAYLOAD, MAX_SEGMENTS};
use crate::error::{NethernetError, Result};
use bytes::{BufMut, Bytes, BytesMut};

/// 重组缓冲一次性预留的字节上限。
///
/// 预留量由对端声明的分片数推算，而该值不可信；封顶后即使对端谎报也
/// 只会多分配这么多，代价是真·大消息多做几次扩容。
const MAX_PREALLOC: usize = 1024 * 1024;

/// 把一条完整消息切成线上分片。
///
/// 单片消息零拷贝：直接复用调用方的 `Bytes`（仅前置 1 字节头）。
///
/// # Errors
///
/// 消息为空或超过 [`MAX_MESSAGE_SIZE`] 时返回错误。
pub fn split(payload: &Bytes) -> Result<Vec<Bytes>> {
    if payload.is_empty() {
        return Err(NethernetError::protocol("不允许发送空消息"));
    }
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(NethernetError::TooLarge {
            size: payload.len(),
            max: MAX_MESSAGE_SIZE,
        });
    }
    let count = payload.len().div_ceil(MAX_SEGMENT_PAYLOAD);
    debug_assert!(count <= MAX_SEGMENTS);

    let mut segments = Vec::with_capacity(count);
    for index in 0..count {
        let start = index * MAX_SEGMENT_PAYLOAD;
        let end = (start + MAX_SEGMENT_PAYLOAD).min(payload.len());
        let chunk = payload.slice(start..end);
        #[allow(clippy::cast_possible_truncation)]
        let remaining = (count - 1 - index) as u8;
        let mut frame = BytesMut::with_capacity(1 + chunk.len());
        frame.put_u8(remaining);
        frame.put_slice(&chunk);
        segments.push(frame.freeze());
    }
    Ok(segments)
}

/// 分片重组器。
///
/// 单片消息走零拷贝快路径，直接把入站缓冲切片交给上层。
#[derive(Debug, Default)]
pub struct Reassembler {
    /// 下一片应声明的剩余片数；`None` 表示尚未开始一条多片消息。
    expected: Option<u8>,
    buffer: BytesMut,
}

impl Reassembler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前缓冲的字节数。
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// 喂入一条数据通道消息，集齐时返回完整消息。
    ///
    /// # Errors
    ///
    /// 消息过短、分片顺序错乱或重组结果超限时返回错误，并重置内部状态。
    pub fn push(&mut self, mut frame: Bytes) -> Result<Option<Bytes>> {
        if frame.len() < 2 {
            self.reset();
            return Err(NethernetError::protocol("数据通道消息过短"));
        }
        let remaining = frame[0];
        // 零拷贝：payload 是入站缓冲的切片。
        frame = frame.slice(1..);

        // 快路径：完整的单片消息，直接交付，不经过重组缓冲。
        if remaining == 0 && self.expected.is_none() && self.buffer.is_empty() {
            return Ok(Some(frame));
        }

        match self.expected {
            None => {
                // 一条多片消息的首片。
                self.expected = remaining.checked_sub(1);
            }
            Some(expected) if expected == remaining => {
                self.expected = remaining.checked_sub(1);
            }
            Some(expected) => {
                self.reset();
                return Err(NethernetError::protocol(format!(
                    "分片顺序错乱：期望剩余 {expected}，收到 {remaining}"
                )));
            }
        }

        if self.buffer.len() + frame.len() > MAX_MESSAGE_SIZE {
            self.reset();
            return Err(NethernetError::TooLarge {
                size: self.buffer.len() + frame.len(),
                max: MAX_MESSAGE_SIZE,
            });
        }
        // 首次扩容按已知总片数预留，避免逐片重分配。
        //
        // 预留量必须封顶：对端声明的片数完全不可信，一个 256 KiB 的分片
        // 配上 count=255 就会诱导 64 MiB 的分配（256 倍放大）。这里只按
        // 一个保守上限预留，后续按 BytesMut 的正常增长策略扩容。
        if self.buffer.is_empty()
            && let Some(expected) = self.expected
        {
            let parts = usize::from(expected) + 2;
            let hint = parts.saturating_mul(frame.len()).min(MAX_PREALLOC);
            self.buffer.reserve(hint);
        }
        self.buffer.put_slice(&frame);

        if remaining == 0 {
            self.expected = None;
            return Ok(Some(self.buffer.split().freeze()));
        }
        Ok(None)
    }

    /// 清空重组状态。
    pub fn reset(&mut self) {
        self.expected = None;
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_segment_is_zero_copy() {
        let mut reassembler = Reassembler::new();
        let frame = Bytes::from_static(b"\x00payload");
        let expected = frame[1..].as_ptr();
        let packet = reassembler.push(frame).unwrap().unwrap();
        assert_eq!(packet.as_ptr(), expected, "单片消息不应发生拷贝");
        assert_eq!(&packet[..], b"payload");
    }

    #[test]
    fn reassembles_in_order() {
        let mut reassembler = Reassembler::new();
        assert_eq!(
            reassembler.push(Bytes::from_static(b"\x02aa")).unwrap(),
            None
        );
        assert_eq!(
            reassembler.push(Bytes::from_static(b"\x01bb")).unwrap(),
            None
        );
        assert_eq!(
            reassembler.push(Bytes::from_static(b"\x00cc")).unwrap(),
            Some(Bytes::from_static(b"aabbcc"))
        );
        assert_eq!(reassembler.buffered(), 0);
    }

    #[test]
    fn rejects_out_of_order_and_recovers() {
        let mut reassembler = Reassembler::new();
        reassembler.push(Bytes::from_static(b"\x02a")).unwrap();
        assert!(reassembler.push(Bytes::from_static(b"\x00b")).is_err());
        // 出错后状态已重置，可继续处理新消息。
        assert_eq!(
            reassembler.push(Bytes::from_static(b"\x00ok")).unwrap(),
            Some(Bytes::from_static(b"ok"))
        );
    }

    #[test]
    fn rejects_short_frame() {
        let mut reassembler = Reassembler::new();
        assert!(reassembler.push(Bytes::from_static(b"\x00")).is_err());
        assert!(reassembler.push(Bytes::new()).is_err());
    }

    #[test]
    fn split_single_segment() {
        let payload = Bytes::from_static(b"hello");
        let segments = split(&payload).unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0][0], 0);
        assert_eq!(&segments[0][1..], b"hello");
    }

    #[test]
    fn split_multi_segment_counts_down() {
        let payload = Bytes::from(vec![9_u8; MAX_SEGMENT_PAYLOAD * 2 + 10]);
        let segments = split(&payload).unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0][0], 2);
        assert_eq!(segments[1][0], 1);
        assert_eq!(segments[2][0], 0);
        assert_eq!(segments[0].len(), MAX_SEGMENT_PAYLOAD + 1);
        assert_eq!(segments[2].len(), 11);
    }

    #[test]
    fn split_then_reassemble_round_trips() {
        let payload = Bytes::from(
            (0..MAX_SEGMENT_PAYLOAD * 2 + 1234)
                .map(|i| u8::try_from(i % 251).unwrap())
                .collect::<Vec<_>>(),
        );
        let mut reassembler = Reassembler::new();
        let mut result = None;
        for segment in split(&payload).unwrap() {
            result = reassembler.push(segment).unwrap();
        }
        assert_eq!(result.unwrap(), payload);
    }

    /// 回归：预留量由对端声明的分片数推算，必须封顶，
    /// 否则单个大分片配上谎报的片数即可诱导 64 MiB 分配。
    #[test]
    fn preallocation_is_capped() {
        let mut reassembler = Reassembler::new();
        let mut frame = BytesMut::with_capacity(MAX_SEGMENT_PAYLOAD + 1);
        frame.put_u8(255); // 声明后面还有 255 片
        frame.resize(MAX_SEGMENT_PAYLOAD + 1, 0);
        reassembler.push(frame.freeze()).unwrap();
        assert!(
            reassembler.buffer.capacity() <= MAX_PREALLOC + MAX_SEGMENT_PAYLOAD * 2,
            "预留了 {} 字节，未封顶",
            reassembler.buffer.capacity()
        );
    }

    #[test]
    fn split_rejects_empty_and_oversized() {
        assert!(split(&Bytes::new()).is_err());
        // 超过 256 片的消息无法用 u8 计数表示。
        let too_big = Bytes::from(vec![0_u8; MAX_MESSAGE_SIZE + 1]);
        assert!(split(&too_big).is_err());
    }

    #[test]
    fn max_size_is_exactly_representable() {
        assert_eq!(MAX_MESSAGE_SIZE, MAX_SEGMENT_PAYLOAD * 256);
        let biggest = Bytes::from(vec![0_u8; MAX_MESSAGE_SIZE]);
        assert_eq!(split(&biggest).unwrap().len(), MAX_SEGMENTS);
    }
}
