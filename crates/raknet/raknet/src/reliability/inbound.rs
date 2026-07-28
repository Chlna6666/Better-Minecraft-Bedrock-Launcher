//! 入站方向：数据报确认簿记、可靠帧去重、拆分重组与有序/序列交付。

use crate::consts::*;
use crate::reliability::unwrap24;
use crate::wire::acknack::AckRanges;
use crate::wire::frame::{Frame, FrameSet};
use bytes::{BufMut, Bytes, BytesMut};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

/// 拆分重组条目的空闲存活时间（每收到新分片刷新）。
const SPLIT_IDLE_TTL: Duration = Duration::from_secs(30);
/// ACK/NACK 待发集合的容量护栏。
const MAX_PENDING_ACKS: usize = 65536;
const MAX_PENDING_NACKS: usize = 8192;
/// 单次序号间隙的 NACK 记录上限（更大的间隙视为异常，靠 RTO 兜底）。
const MAX_NACK_GAP: u64 = 4096;
/// 小于该长度的载荷在进入缓冲前拷贝一份，切断对入站数据报父缓冲的引用。
///
/// 否则一个 1 字节的帧会把整个数据报（最大 MTU）钉在内存里，
/// 使按载荷长度计费的 [`MAX_ORDERED_BYTES`] / [`MAX_SPLIT_BYTES`]
/// 上限被放大数个量级。
const DETACH_THRESHOLD: usize = 512;

/// 需要长期缓冲的载荷：过小的切片改为独立分配，避免钉住父缓冲。
#[inline]
fn detach(payload: Bytes) -> Bytes {
    if payload.len() < DETACH_THRESHOLD {
        Bytes::copy_from_slice(&payload)
    } else {
        payload
    }
}

struct SplitEntry {
    count: u32,
    /// 已收分片（惰性插入，不按 `count` 预分配——否则对端可用一个
    /// 最小帧触发 `count` 规模的分配）。
    parts: HashMap<u32, Bytes>,
    bytes: usize,
    last_activity: Instant,
}

struct OrderChannel {
    /// 下一个应交付的有序序号。
    expected: u64,
    /// 序列帧当前纪元（对应发送方未递增的有序序号）。
    seq_epoch: u64,
    /// 当前纪元内下一个可接受的序列号。
    seq_next: u64,
    /// 乱序等待缓冲。
    pending: BTreeMap<u64, Bytes>,
}

impl OrderChannel {
    fn new() -> Self {
        Self {
            expected: 0,
            seq_epoch: 0,
            seq_next: 0,
            pending: BTreeMap::new(),
        }
    }
}

/// 入站状态机。
pub(crate) struct Inbound {
    /// 下一个期望的数据报序号（逻辑值）。
    next_seq: u64,
    ack_pending: BTreeSet<u64>,
    nack_pending: BTreeSet<u64>,

    /// 可靠帧去重：`rel_base` 之前的序号均已接收，`rel_seen` 保存其后
    /// 乱序到达的序号。集合规模（而非序号跨度）受上限约束，
    /// 因此不会出现「帧被丢弃却已 ACK」导致的永久空洞。
    rel_base: u64,
    rel_hint: u64,
    rel_seen: HashSet<u64>,

    splits: HashMap<u16, SplitEntry>,
    splits_bytes: usize,

    channels: Vec<OrderChannel>,
    ordered_bytes: usize,
}

impl Inbound {
    pub fn new(channel_count: usize) -> Self {
        Self {
            next_seq: 0,
            ack_pending: BTreeSet::new(),
            nack_pending: BTreeSet::new(),
            rel_base: 0,
            rel_hint: 0,
            rel_seen: HashSet::new(),
            splits: HashMap::new(),
            splits_bytes: 0,
            channels: (0..channel_count.clamp(1, MAX_ORDERING_CHANNELS))
                .map(|_| OrderChannel::new())
                .collect(),
            ordered_bytes: 0,
        }
    }

    /// 处理一个 FrameSet，交付完整消息到 `deliveries`。
    ///
    /// `Err` 表示防护上限被击穿（对端异常/恶意），应断开会话。
    pub fn ingest(
        &mut self,
        set: FrameSet,
        deliveries: &mut Vec<Bytes>,
    ) -> Result<(), &'static str> {
        self.note_datagram(unwrap24(set.sequence, self.next_seq));
        let now = Instant::now();
        for frame in set.frames {
            self.handle_frame(frame, now, deliveries)?;
        }
        Ok(())
    }

    fn note_datagram(&mut self, seq: u64) {
        if self.ack_pending.len() < MAX_PENDING_ACKS {
            self.ack_pending.insert(seq);
        }
        if seq == self.next_seq {
            self.next_seq += 1;
        } else if seq > self.next_seq {
            let gap = seq - self.next_seq;
            if gap <= MAX_NACK_GAP {
                for missing in self.next_seq..seq {
                    if self.nack_pending.len() >= MAX_PENDING_NACKS {
                        break;
                    }
                    self.nack_pending.insert(missing);
                }
            }
            self.next_seq = seq + 1;
        } else {
            // 迟到（重传）数据报：不再视为丢失。
            self.nack_pending.remove(&seq);
        }
    }

    fn handle_frame(
        &mut self,
        frame: Frame,
        now: Instant,
        deliveries: &mut Vec<Bytes>,
    ) -> Result<(), &'static str> {
        // 可靠帧去重（重传的帧带原可靠序号，但数据报序号是新的，
        // 因此去重必须基于可靠序号而非数据报序号）。
        if frame.reliability.is_reliable() {
            let idx = unwrap24(frame.reliable_index, self.rel_hint);
            if idx < self.rel_base || self.rel_seen.contains(&idx) {
                return Ok(()); // 重复帧。
            }
            if self.rel_seen.len() >= MAX_RELIABLE_SEEN {
                // 空洞长期不被填补（对端异常或恶意），集合已达内存上限。
                return Err("可靠去重集合超限");
            }
            self.rel_seen.insert(idx);
            self.rel_hint = self.rel_hint.max(idx + 1);
            while self.rel_seen.remove(&self.rel_base) {
                self.rel_base += 1;
            }
        }

        let (payload, reliability, channel, ord_wire, seq_wire) = if frame.split.is_some() {
            match self.assemble_split(frame, now)? {
                Some(v) => v,
                None => return Ok(()), // 尚未集齐。
            }
        } else {
            (
                frame.payload,
                frame.reliability,
                frame.order_channel,
                frame.order_index,
                frame.sequence_index,
            )
        };

        self.dispatch(
            payload,
            reliability,
            channel,
            ord_wire,
            seq_wire,
            deliveries,
        )
    }

    /// 拆分重组。返回 `Some` 表示消息集齐。
    #[allow(clippy::type_complexity)]
    fn assemble_split(
        &mut self,
        frame: Frame,
        now: Instant,
    ) -> Result<Option<(Bytes, crate::types::RakReliability, u8, u32, u32)>, &'static str> {
        let Some(split) = frame.split else {
            unreachable!()
        };
        if split.count == 0 || split.count > MAX_SPLIT_PARTS || split.index >= split.count {
            return Err("拆分参数非法");
        }

        let entry = match self.splits.get_mut(&split.id) {
            // 同 ID 但总片数不符：这是陈旧组的残留或伪造帧。直接丢弃该帧，
            // 不重建条目——重建路径会被用来反复触发分配。
            Some(entry) if entry.count != split.count => return Ok(None),
            Some(entry) => entry,
            None => {
                if self.splits.len() >= MAX_ACTIVE_SPLITS {
                    return Err("并发拆分组数超限");
                }
                self.splits.entry(split.id).or_insert(SplitEntry {
                    count: split.count,
                    parts: HashMap::new(),
                    bytes: 0,
                    last_activity: now,
                })
            }
        };

        if let std::collections::hash_map::Entry::Vacant(slot) = entry.parts.entry(split.index) {
            let payload = detach(frame.payload.clone());
            entry.bytes += payload.len();
            self.splits_bytes += payload.len();
            // 传输中的大消息按活跃度续期，不因总时长超时被中途丢弃。
            entry.last_activity = now;
            slot.insert(payload);
        }
        if self.splits_bytes > MAX_SPLIT_BYTES {
            return Err("拆分重组缓冲超限");
        }
        let entry = &self.splits[&split.id];
        if entry.parts.len() as u32 != entry.count {
            return Ok(None);
        }

        let entry = self.splits.remove(&split.id).unwrap();
        self.splits_bytes -= entry.bytes;
        let mut parts = entry.parts;
        let mut assembled = BytesMut::with_capacity(entry.bytes);
        for i in 0..entry.count {
            assembled.put_slice(&parts.remove(&i).expect("片数集齐时索引连续"));
        }
        Ok(Some((
            assembled.freeze(),
            frame.reliability,
            frame.order_channel,
            frame.order_index,
            frame.sequence_index,
        )))
    }

    fn dispatch(
        &mut self,
        payload: Bytes,
        reliability: crate::types::RakReliability,
        channel: u8,
        ord_wire: u32,
        seq_wire: u32,
        deliveries: &mut Vec<Bytes>,
    ) -> Result<(), &'static str> {
        if reliability.is_sequenced() {
            let Some(ch) = self.channels.get_mut(channel as usize) else {
                return Ok(()); // 非法通道，丢弃。
            };
            let ord = unwrap24(ord_wire, ch.expected);
            if ord < ch.expected || ord < ch.seq_epoch {
                return Ok(()); // 陈旧纪元。
            }
            if ord > ch.seq_epoch {
                ch.seq_epoch = ord;
                ch.seq_next = 0;
            }
            let seq = unwrap24(seq_wire, ch.seq_next);
            if seq < ch.seq_next {
                return Ok(()); // 被更新的序列帧超越，丢弃。
            }
            ch.seq_next = seq + 1;
            deliveries.push(payload);
            return Ok(());
        }

        if reliability.is_ordered() {
            let Some(ch) = self.channels.get_mut(channel as usize) else {
                return Ok(());
            };
            let ord = unwrap24(ord_wire, ch.expected);
            if ord < ch.expected {
                return Ok(()); // 已交付过。
            }
            if ord == ch.expected {
                ch.expected += 1;
                if ch.seq_epoch < ord {
                    ch.seq_epoch = ord;
                    ch.seq_next = 0;
                }
                deliveries.push(payload);
                while let Some(next) = ch.pending.remove(&ch.expected) {
                    self.ordered_bytes -= next.len();
                    ch.expected += 1;
                    deliveries.push(next);
                }
                return Ok(());
            }
            // 乱序：缓冲等待。
            if ord - ch.expected > MAX_ORDERED_PENDING as u64 {
                return Err("有序缓冲窗口超限");
            }
            if let std::collections::btree_map::Entry::Vacant(v) = ch.pending.entry(ord) {
                let payload = detach(payload);
                self.ordered_bytes += payload.len();
                v.insert(payload);
            }
            if self.ordered_bytes > MAX_ORDERED_BYTES {
                return Err("有序缓冲字节超限");
            }
            return Ok(());
        }

        deliveries.push(payload);
        Ok(())
    }

    /// 当前待确认的数据报数量。
    pub fn pending_ack_count(&self) -> usize {
        self.ack_pending.len()
    }

    /// 取走并清空待发 ACK（逻辑序号转为 u24 线上范围，跨回绕边界自动切分）。
    pub fn take_acks(&mut self) -> AckRanges {
        Self::ranges_from_set(std::mem::take(&mut self.ack_pending))
    }

    /// 取走并清空待发 NACK。
    pub fn take_nacks(&mut self) -> AckRanges {
        Self::ranges_from_set(std::mem::take(&mut self.nack_pending))
    }

    fn ranges_from_set(set: BTreeSet<u64>) -> AckRanges {
        const SPAN: u64 = 1 << 24;
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        let mut current: Option<(u64, u64)> = None;
        let flush = |r: (u64, u64), ranges: &mut Vec<(u32, u32)>| {
            // 跨 2^24 块边界的范围切分，保证 wire 上 start <= end。
            let (mut s, e) = r;
            while s / SPAN != e / SPAN {
                let block_end = (s / SPAN + 1) * SPAN - 1;
                ranges.push(((s % SPAN) as u32, (block_end % SPAN) as u32));
                s = block_end + 1;
            }
            ranges.push(((s % SPAN) as u32, (e % SPAN) as u32));
        };
        for seq in set {
            match current {
                Some((s, e)) if seq == e + 1 => current = Some((s, seq)),
                Some(r) => {
                    flush(r, &mut ranges);
                    current = Some((seq, seq));
                }
                None => current = Some((seq, seq)),
            }
        }
        if let Some(r) = current {
            flush(r, &mut ranges);
        }
        AckRanges { ranges }
    }

    /// 清理长时间无新分片到达的重组条目。
    pub fn gc(&mut self, now: Instant) {
        if self.splits.is_empty() {
            return;
        }
        let mut reclaimed = 0usize;
        self.splits.retain(|_, entry| {
            if now.duration_since(entry.last_activity) > SPLIT_IDLE_TTL {
                reclaimed += entry.bytes;
                false
            } else {
                true
            }
        });
        self.splits_bytes -= reclaimed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RakReliability;
    use crate::wire::frame::SplitInfo;

    fn frame(rel_idx: u32, ord_idx: u32, payload: &[u8]) -> Frame {
        Frame {
            reliability: RakReliability::ReliableOrdered,
            reliable_index: rel_idx,
            sequence_index: 0,
            order_index: ord_idx,
            order_channel: 0,
            split: None,
            payload: Bytes::copy_from_slice(payload),
        }
    }

    fn set(seq: u32, frames: Vec<Frame>) -> FrameSet {
        FrameSet {
            sequence: seq,
            frames,
        }
    }

    #[test]
    fn in_order_delivery() {
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        inn.ingest(set(0, vec![frame(0, 0, b"a")]), &mut got)
            .unwrap();
        inn.ingest(set(1, vec![frame(1, 1, b"b")]), &mut got)
            .unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(&got[0][..], b"a");
        assert_eq!(&got[1][..], b"b");
    }

    #[test]
    fn out_of_order_buffered_until_gap_filled() {
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        inn.ingest(set(1, vec![frame(1, 1, b"b")]), &mut got)
            .unwrap();
        assert!(got.is_empty());
        inn.ingest(set(0, vec![frame(0, 0, b"a")]), &mut got)
            .unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(&got[0][..], b"a");
        assert_eq!(&got[1][..], b"b");
    }

    #[test]
    fn gap_generates_nack_and_late_arrival_clears() {
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        inn.ingest(set(0, vec![frame(0, 0, b"a")]), &mut got)
            .unwrap();
        inn.ingest(set(3, vec![frame(3, 3, b"d")]), &mut got)
            .unwrap();
        assert_eq!(
            inn.nack_pending.iter().copied().collect::<Vec<_>>(),
            vec![1, 2]
        );
        // 迟到的 2 号到达后不再 NACK。
        inn.ingest(set(2, vec![frame(2, 2, b"c")]), &mut got)
            .unwrap();
        assert_eq!(
            inn.nack_pending.iter().copied().collect::<Vec<_>>(),
            vec![1]
        );
        let nacks = inn.take_nacks();
        assert_eq!(nacks.ranges, vec![(1, 1)]);
        assert!(inn.take_nacks().is_empty());
    }

    #[test]
    fn duplicate_reliable_frame_dropped() {
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        inn.ingest(set(0, vec![frame(0, 0, b"a")]), &mut got)
            .unwrap();
        // 重传：新数据报序号、相同可靠序号。
        inn.ingest(set(1, vec![frame(0, 0, b"a")]), &mut got)
            .unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn split_reassembly_out_of_order_parts() {
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        let mk = |idx: u32, rel: u32, data: &[u8]| Frame {
            reliability: RakReliability::Reliable,
            reliable_index: rel,
            sequence_index: 0,
            order_index: 0,
            order_channel: 0,
            split: Some(SplitInfo {
                count: 3,
                id: 9,
                index: idx,
            }),
            payload: Bytes::copy_from_slice(data),
        };
        inn.ingest(set(0, vec![mk(2, 2, b"cc")]), &mut got).unwrap();
        inn.ingest(set(1, vec![mk(0, 0, b"aa")]), &mut got).unwrap();
        assert!(got.is_empty());
        inn.ingest(set(2, vec![mk(1, 1, b"bb")]), &mut got).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(&got[0][..], b"aabbcc");
        assert_eq!(inn.splits_bytes, 0);
    }

    #[test]
    fn split_flood_hits_guard() {
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        let mut result = Ok(());
        for id in 0..(MAX_ACTIVE_SPLITS as u16 + 2) {
            let f = Frame {
                reliability: RakReliability::Reliable,
                reliable_index: id as u32,
                sequence_index: 0,
                order_index: 0,
                order_channel: 0,
                split: Some(SplitInfo {
                    count: 2,
                    id,
                    index: 0,
                }),
                payload: Bytes::from_static(b"x"),
            };
            result = inn.ingest(set(id as u32, vec![f]), &mut got);
            if result.is_err() {
                break;
            }
        }
        assert!(result.is_err(), "超过并发拆分组上限应报错");
    }

    #[test]
    fn sequenced_stale_dropped_newer_kept() {
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        let mk = |seq_idx: u32, data: &'static [u8]| Frame {
            reliability: RakReliability::UnreliableSequenced,
            reliable_index: 0,
            sequence_index: seq_idx,
            order_index: 0,
            order_channel: 0,
            split: None,
            payload: Bytes::from_static(data),
        };
        inn.ingest(set(0, vec![mk(1, b"new")]), &mut got).unwrap();
        inn.ingest(set(1, vec![mk(0, b"old")]), &mut got).unwrap();
        inn.ingest(set(2, vec![mk(2, b"newer")]), &mut got).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(&got[0][..], b"new");
        assert_eq!(&got[1][..], b"newer");
    }

    #[test]
    fn ack_ranges_split_at_wrap_boundary() {
        let mut inn = Inbound::new(32);
        inn.ack_pending
            .extend([(1u64 << 24) - 2, (1 << 24) - 1, 1 << 24, (1 << 24) + 1]);
        let acks = inn.take_acks();
        assert_eq!(acks.ranges, vec![(0xFF_FFFE, 0xFF_FFFF), (0, 1)]);
    }

    #[test]
    fn far_ahead_reliable_frame_accepted_and_deduped() {
        // 回归：曾以序号跨度（65536）为窗口，超窗帧被丢弃却仍被 ACK，
        // 形成永不填补的空洞、后续消息全部静默丢失。
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        inn.ingest(set(0, vec![frame(0, 0, b"a")]), &mut got)
            .unwrap();
        let far = 200_000u32;
        let mut f = frame(far, 1, b"far");
        f.reliability = RakReliability::Reliable;
        inn.ingest(set(1, vec![f.clone()]), &mut got).unwrap();
        assert_eq!(got.len(), 2, "远超旧窗口的可靠帧必须被接收");
        // 重复到达仍被去重。
        inn.ingest(set(2, vec![f]), &mut got).unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn split_count_mismatch_does_not_reallocate() {
        // 回归：同 id 不同 count 曾无条件重建条目并按 count 预分配，
        // 攻击者可用一个最小帧触发上百 MB 的分配。
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        let mk = |count: u32, rel: u32| Frame {
            reliability: RakReliability::Reliable,
            reliable_index: rel,
            sequence_index: 0,
            order_index: 0,
            order_channel: 0,
            split: Some(SplitInfo {
                count,
                id: 0,
                index: 0,
            }),
            payload: Bytes::from_static(b"x"),
        };
        inn.ingest(set(0, vec![mk(2, 0)]), &mut got).unwrap();
        for i in 1..50u32 {
            inn.ingest(set(i, vec![mk(4096 - i % 2, i)]), &mut got)
                .unwrap();
        }
        assert_eq!(inn.splits.len(), 1);
        assert_eq!(inn.splits[&0].count, 2, "原有条目不应被 count 不符的帧重置");
        assert_eq!(inn.splits[&0].parts.len(), 1, "分片按需插入，不预分配");
    }

    #[test]
    fn split_ttl_refreshes_on_new_parts() {
        // 回归：TTL 曾按创建时间计算，慢速链路上的大消息会被中途清除。
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        let mk = |idx: u32, rel: u32| Frame {
            reliability: RakReliability::Reliable,
            reliable_index: rel,
            sequence_index: 0,
            order_index: 0,
            order_channel: 0,
            split: Some(SplitInfo {
                count: 3,
                id: 1,
                index: idx,
            }),
            payload: Bytes::from_static(b"pp"),
        };
        inn.ingest(set(0, vec![mk(0, 0)]), &mut got).unwrap();
        let created = inn.splits[&1].last_activity;
        // 40 秒后到达第二片：条目仍在（TTL 已随活跃度续期）。
        inn.splits.get_mut(&1).unwrap().last_activity = created;
        inn.gc(created + Duration::from_secs(20));
        assert_eq!(inn.splits.len(), 1);
        inn.ingest(set(1, vec![mk(1, 1)]), &mut got).unwrap();
        assert!(inn.splits[&1].last_activity > created, "新分片应刷新 TTL");
        // 长时间无新分片才淘汰。
        let idle = inn.splits[&1].last_activity;
        inn.gc(idle + Duration::from_secs(31));
        assert!(inn.splits.is_empty());
        assert_eq!(inn.splits_bytes, 0);
    }

    #[test]
    fn tiny_buffered_payloads_detached_from_parent() {
        // 回归：小切片会把整个数据报钉在内存，使字节上限被放大数量级。
        let datagram = Bytes::from(vec![7u8; 1400]);
        let tiny = datagram.slice(0..1);
        let detached = detach(tiny);
        let base = datagram.as_ptr() as usize;
        let ptr = detached.as_ptr() as usize;
        assert!(
            ptr < base || ptr >= base + datagram.len(),
            "小载荷必须脱离父缓冲"
        );
        // 大切片保持零拷贝。
        let big = datagram.slice(0..1000);
        let kept = detach(big);
        let ptr = kept.as_ptr() as usize;
        assert!(ptr >= base && ptr < base + datagram.len());
    }

    #[test]
    fn duplicate_datagram_still_acked() {
        let mut inn = Inbound::new(32);
        let mut got = Vec::new();
        inn.ingest(set(0, vec![frame(0, 0, b"a")]), &mut got)
            .unwrap();
        let _ = inn.take_acks();
        inn.ingest(set(0, vec![frame(0, 0, b"a")]), &mut got)
            .unwrap();
        let acks = inn.take_acks();
        assert_eq!(acks.ranges, vec![(0, 0)], "重复数据报也必须重新 ACK");
        assert_eq!(got.len(), 1);
    }
}
