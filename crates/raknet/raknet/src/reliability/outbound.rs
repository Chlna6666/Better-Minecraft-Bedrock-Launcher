//! 出站方向：消息拆分、按优先级排队、合帧打包、重传与拥塞控制。

use crate::consts::*;
use crate::error::RakSessionError;
use crate::types::{RakPriority, RakReliability};
use crate::wire::acknack::AckRanges;
use crate::wire::frame::{Frame, SplitInfo, put_datagram_header};
use bytes::{Bytes, BytesMut};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::time::{Duration, Instant};

/// 逐序号迭代的范围宽度上限；更宽的范围改为遍历实际在途集合，
/// 使代价与在途数据报数成正比，而非与对端声明的范围宽度成正比。
const RANGE_SCAN_LIMIT: u64 = 64;
/// 拥塞窗口上限。
const CWND_MAX: f64 = 8.0 * 1024.0 * 1024.0;
/// 初始拥塞窗口（MTU 倍数）。
const CWND_INITIAL_MTUS: f64 = 32.0;

/// 待发送的一帧（已完成拆分与序号分配）。
#[derive(Clone, Debug)]
pub struct OutFrame {
    pub reliability: RakReliability,
    pub reliable_index: u64,
    pub sequence_index: u64,
    pub order_index: u64,
    pub order_channel: u8,
    pub split: Option<SplitInfo>,
    pub payload: Bytes,
    /// 重发次数（用于退避）。
    pub retries: u8,
    /// 其中由 RTO 超时触发的次数（用于判定链路死亡）。
    pub timeouts: u8,
}

impl OutFrame {
    fn encoded_len(&self) -> usize {
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

    fn encode_into(&self, buf: &mut BytesMut) {
        let frame = Frame {
            reliability: self.reliability,
            reliable_index: (self.reliable_index & 0x00FF_FFFF) as u32,
            sequence_index: (self.sequence_index & 0x00FF_FFFF) as u32,
            order_index: (self.order_index & 0x00FF_FFFF) as u32,
            order_channel: self.order_channel,
            split: self.split,
            payload: self.payload.clone(),
        };
        frame.encode_into(buf);
    }
}

/// 已发送、等待 ACK 的数据报。
struct SentDatagram {
    frames: Vec<OutFrame>,
    sent_at: Instant,
    bytes: usize,
    retransmitted: bool,
}

/// RFC 6298 风格的 RTT 估计 + AIMD 拥塞窗口。
struct Congestion {
    mtu: f64,
    cwnd: f64,
    ssthresh: f64,
    srtt_ms: f64,
    rttvar_ms: f64,
    has_rtt: bool,
    /// 恢复期结束序号：该序号之前的丢包不再触发二次窗口收缩。
    recovery_end: Option<u64>,
}

impl Congestion {
    fn new(mtu: usize) -> Self {
        Self {
            mtu: mtu as f64,
            cwnd: CWND_INITIAL_MTUS * mtu as f64,
            ssthresh: CWND_MAX,
            srtt_ms: 0.0,
            rttvar_ms: 0.0,
            has_rtt: false,
            recovery_end: None,
        }
    }

    fn rto(&self) -> Duration {
        if !self.has_rtt {
            return RTO_INITIAL;
        }
        let ms = self.srtt_ms + 4.0 * self.rttvar_ms + 20.0;
        Duration::from_millis(ms as u64).clamp(RTO_MIN, RTO_MAX)
    }

    fn rtt(&self) -> Duration {
        Duration::from_micros((self.srtt_ms * 1000.0) as u64)
    }

    fn sample_rtt(&mut self, rtt: Duration) {
        let ms = rtt.as_secs_f64() * 1000.0;
        if !self.has_rtt {
            self.srtt_ms = ms;
            self.rttvar_ms = ms / 2.0;
            self.has_rtt = true;
        } else {
            let diff = (self.srtt_ms - ms).abs();
            self.rttvar_ms = 0.75 * self.rttvar_ms + 0.25 * diff;
            self.srtt_ms = 0.875 * self.srtt_ms + 0.125 * ms;
        }
    }

    fn on_ack(&mut self, seq: u64, bytes: usize) {
        if let Some(end) = self.recovery_end
            && seq >= end
        {
            self.recovery_end = None;
        }
        if self.cwnd < self.ssthresh {
            self.cwnd = (self.cwnd + bytes as f64).min(CWND_MAX);
        } else {
            self.cwnd = (self.cwnd + self.mtu * self.mtu / self.cwnd).min(CWND_MAX);
        }
    }

    fn on_rto(&mut self, next_seq: u64) {
        if self.recovery_end.is_none() {
            self.ssthresh = (self.cwnd / 2.0).max(2.0 * self.mtu);
            self.cwnd = 2.0 * self.mtu;
            self.recovery_end = Some(next_seq);
        }
    }

    fn on_nack(&mut self, next_seq: u64) {
        if self.recovery_end.is_none() {
            self.ssthresh = (self.cwnd * 0.75).max(2.0 * self.mtu);
            self.cwnd = self.ssthresh;
            self.recovery_end = Some(next_seq);
        }
    }
}

/// 出站状态机。
pub(crate) struct Outbound {
    /// 数据报最大载荷（不含 IP/UDP 头）。
    mtu_dgram: usize,
    /// 单帧最大用户载荷（超过则拆分）。
    max_frame_payload: usize,

    next_seq: u64,
    next_rel: u64,
    next_split: u16,
    ord_idx: [u64; MAX_ORDERING_CHANNELS],
    seq_idx: [u64; MAX_ORDERING_CHANNELS],

    /// 队列：`[Immediate, High, Normal, Low]`；重传队列独立且最优先。
    queues: [VecDeque<OutFrame>; 4],
    resend_queue: VecDeque<OutFrame>,
    queued_bytes: usize,
    max_queued_bytes: usize,

    unacked: HashMap<u64, SentDatagram>,
    inflight_bytes: usize,
    resend_timers: BinaryHeap<Reverse<(Instant, u64)>>,

    cc: Congestion,
    dead: bool,
}

impl Outbound {
    pub fn new(mtu_dgram: usize, max_queued_bytes: usize) -> Self {
        Self {
            mtu_dgram,
            max_frame_payload: mtu_dgram - DGRAM_HEADER_SIZE - FRAME_HEADER_MAX,
            next_seq: 0,
            next_rel: 0,
            next_split: 0,
            ord_idx: [0; MAX_ORDERING_CHANNELS],
            seq_idx: [0; MAX_ORDERING_CHANNELS],
            queues: Default::default(),
            resend_queue: VecDeque::new(),
            queued_bytes: 0,
            max_queued_bytes,
            unacked: HashMap::new(),
            inflight_bytes: 0,
            resend_timers: BinaryHeap::new(),
            cc: Congestion::new(mtu_dgram),
            dead: false,
        }
    }

    pub fn rtt(&self) -> Duration {
        self.cc.rtt()
    }

    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// 拆分并入队一条消息。载荷切片零拷贝（`Bytes::slice`）。
    pub fn enqueue(
        &mut self,
        payload: Bytes,
        reliability: RakReliability,
        priority: RakPriority,
        order_channel: u8,
    ) -> Result<(), RakSessionError> {
        let max_message = self.max_frame_payload * MAX_SPLIT_PARTS as usize;
        if payload.len() > max_message {
            return Err(RakSessionError::TooLarge {
                size: payload.len(),
                max: max_message,
            });
        }
        if priority != RakPriority::Immediate
            && self.queued_bytes + payload.len() > self.max_queued_bytes
        {
            return Err(RakSessionError::QueueFull {
                max: self.max_queued_bytes,
            });
        }

        let channel = (order_channel as usize).min(MAX_ORDERING_CHANNELS - 1);
        let needs_split = payload.len() > self.max_frame_payload;
        let reliability = if needs_split {
            reliability.upgrade_for_split()
        } else {
            reliability
        };

        // 序号分配与 RakNet 语义一致：
        // - sequenced：使用当前有序序号（不递增），序列号递增；
        // - ordered：有序序号递增，序列号清零。
        let (order_index, sequence_index) = if reliability.is_sequenced() {
            let seq = self.seq_idx[channel];
            self.seq_idx[channel] += 1;
            (self.ord_idx[channel], seq)
        } else if reliability.is_ordered() {
            let ord = self.ord_idx[channel];
            self.ord_idx[channel] += 1;
            self.seq_idx[channel] = 0;
            (ord, 0)
        } else {
            (0, 0)
        };

        let queue = priority.queue_index();
        if needs_split {
            let count = payload.len().div_ceil(self.max_frame_payload) as u32;
            let split_id = self.next_split;
            self.next_split = self.next_split.wrapping_add(1);
            for index in 0..count {
                let start = index as usize * self.max_frame_payload;
                let end = (start + self.max_frame_payload).min(payload.len());
                let chunk = payload.slice(start..end);
                let reliable_index = self.next_rel;
                self.next_rel += 1;
                let frame = OutFrame {
                    reliability,
                    reliable_index,
                    sequence_index,
                    order_index,
                    order_channel: channel as u8,
                    split: Some(SplitInfo {
                        count,
                        id: split_id,
                        index,
                    }),
                    payload: chunk,
                    retries: 0,
                    timeouts: 0,
                };
                if priority != RakPriority::Immediate {
                    self.queued_bytes += frame.payload.len();
                }
                self.queues[queue].push_back(frame);
            }
        } else {
            let reliable_index = if reliability.is_reliable() {
                let idx = self.next_rel;
                self.next_rel += 1;
                idx
            } else {
                0
            };
            let frame = OutFrame {
                reliability,
                reliable_index,
                sequence_index,
                order_index,
                order_channel: channel as u8,
                split: None,
                payload,
                retries: 0,
                timeouts: 0,
            };
            if priority != RakPriority::Immediate {
                self.queued_bytes += frame.payload.len();
            }
            self.queues[queue].push_back(frame);
        }
        Ok(())
    }

    /// 打包可发送的数据报。
    ///
    /// Immediate 队列与重传队列不受拥塞窗口约束；其余队列按
    /// `cwnd - inflight` 预算出队。
    pub fn pump(&mut self, now: Instant, out: &mut Vec<Bytes>) {
        // 1) Immediate + 重传：全量冲刷。
        while !self.queues[0].is_empty() || !self.resend_queue.is_empty() {
            self.pack_one(now, out, usize::MAX);
        }

        // 2) 其余优先级按预算发送。
        loop {
            let budget = (self.cc.cwnd as usize).saturating_sub(self.inflight_bytes);
            if budget < self.mtu_dgram && self.inflight_bytes > 0 {
                break;
            }
            if self.queues[1..].iter().all(VecDeque::is_empty) {
                break;
            }
            self.pack_one(now, out, budget.max(self.mtu_dgram));
        }
    }

    /// 打包一个数据报：重传帧最优先，然后按优先级顺序取帧填满 MTU。
    fn pack_one(&mut self, now: Instant, out: &mut Vec<Bytes>, budget: usize) {
        let cap = self.mtu_dgram.min(budget);
        let mut buf = BytesMut::with_capacity(self.mtu_dgram);
        let seq = self.next_seq;
        put_datagram_header(&mut buf, (seq & 0x00FF_FFFF) as u32);

        let mut frames: Vec<OutFrame> = Vec::new();
        let mut reliable_bytes = 0usize;

        'fill: loop {
            let candidate = if let Some(f) = self.resend_queue.front() {
                Some((f.encoded_len(), true, 0usize))
            } else {
                let mut found = None;
                for qi in 0..self.queues.len() {
                    if let Some(f) = self.queues[qi].front() {
                        found = Some((f.encoded_len(), false, qi));
                        break;
                    }
                }
                found
            };
            let Some((len, from_resend, qi)) = candidate else {
                break 'fill;
            };
            if buf.len() + len > cap {
                break 'fill;
            }
            let frame = if from_resend {
                self.resend_queue.pop_front().unwrap()
            } else {
                let f = self.queues[qi].pop_front().unwrap();
                if qi != 0 {
                    self.queued_bytes = self.queued_bytes.saturating_sub(f.payload.len());
                }
                f
            };
            frame.encode_into(&mut buf);
            if frame.reliability.is_reliable() {
                reliable_bytes += len;
                frames.push(frame);
            }
        }

        if buf.len() == DGRAM_HEADER_SIZE {
            return; // 没有装进任何帧（单帧超出预算时留待下轮）。
        }

        self.next_seq += 1;
        let bytes = buf.len();
        out.push(buf.freeze());

        if !frames.is_empty() {
            let retries = frames.iter().map(|f| f.retries).max().unwrap_or(0);
            let backoff = self.cc.rto() * 2u32.saturating_pow(retries.min(5) as u32);
            self.inflight_bytes += reliable_bytes;
            self.resend_timers.push(Reverse((now + backoff, seq)));
            self.unacked.insert(
                seq,
                SentDatagram {
                    frames,
                    sent_at: now,
                    bytes: reliable_bytes,
                    retransmitted: retries > 0,
                },
            );
            let _ = bytes;
        }
    }

    /// 求出 ACK/NACK 范围与实际在途数据报的交集。
    ///
    /// 窄范围直接逐序号查表；宽范围（可由 11 字节的伪造 ACK 声明整个
    /// u24 空间）改为遍历 `unacked`，代价与在途数据报数成正比。
    fn matching_unacked(&self, ranges: &AckRanges) -> Vec<u64> {
        let mut wide: Vec<(u64, u64)> = Vec::new();
        let mut hits = Vec::new();
        for &(start, end) in &ranges.ranges {
            let start = super::unwrap24(start, self.next_seq);
            let end = super::unwrap24(end, self.next_seq);
            if end < start {
                continue;
            }
            if end - start < RANGE_SCAN_LIMIT {
                hits.extend((start..=end).filter(|seq| self.unacked.contains_key(seq)));
            } else {
                wide.push((start, end));
            }
        }
        if !wide.is_empty() {
            hits.extend(
                self.unacked
                    .keys()
                    .copied()
                    .filter(|seq| wide.iter().any(|&(s, e)| (s..=e).contains(seq))),
            );
            hits.sort_unstable();
            hits.dedup();
        }
        hits
    }

    /// 处理对端 ACK。
    pub fn on_ack(&mut self, ranges: &AckRanges, now: Instant) {
        for seq in self.matching_unacked(ranges) {
            let Some(sent) = self.unacked.remove(&seq) else {
                continue;
            };
            self.inflight_bytes = self.inflight_bytes.saturating_sub(sent.bytes);
            // Karn 算法：重传过的数据报不用于 RTT 采样。
            if !sent.retransmitted {
                self.cc.sample_rtt(now.duration_since(sent.sent_at));
            }
            self.cc.on_ack(seq, sent.bytes);
        }
    }

    /// 处理对端 NACK：立即重排队。
    pub fn on_nack(&mut self, ranges: &AckRanges) {
        let mut lost_any = false;
        for seq in self.matching_unacked(ranges) {
            let Some(sent) = self.unacked.remove(&seq) else {
                continue;
            };
            self.inflight_bytes = self.inflight_bytes.saturating_sub(sent.bytes);
            lost_any = true;
            // NACK 不计入死亡预算：NACK 未经认证，否则伪造十余个报文
            // 就能把 retries 顶过上限、判死一条健康会话。链路是否死亡
            // 只由 RTO 连续超时判定。
            self.requeue(sent.frames, false);
        }
        if lost_any {
            self.cc.on_nack(self.next_seq);
        }
    }

    /// 周期检查：RTO 到期的数据报重新入队。
    pub fn on_tick(&mut self, now: Instant) {
        let mut fired = false;
        while let Some(&Reverse((due, seq))) = self.resend_timers.peek() {
            if due > now {
                break;
            }
            self.resend_timers.pop();
            let Some(sent) = self.unacked.remove(&seq) else {
                continue; // 已被 ACK/NACK 处理。
            };
            self.inflight_bytes = self.inflight_bytes.saturating_sub(sent.bytes);
            fired = true;
            self.requeue(sent.frames, true);
        }
        if fired {
            self.cc.on_rto(self.next_seq);
        }
    }

    /// 重新入队。`count_toward_death` 为真时累计超时次数，
    /// 连续超时耗尽预算即判定链路死亡。
    fn requeue(&mut self, frames: Vec<OutFrame>, count_toward_death: bool) {
        for mut frame in frames {
            frame.retries = frame.retries.saturating_add(1);
            if count_toward_death {
                frame.timeouts = frame.timeouts.saturating_add(1);
                if frame.timeouts > MAX_RETRIES {
                    self.dead = true;
                    return;
                }
            }
            self.resend_queue.push_back(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbound() -> Outbound {
        Outbound::new(1200, MAX_QUEUED_BYTES)
    }

    #[test]
    fn small_messages_coalesce_into_one_datagram() {
        let mut out = outbound();
        for _ in 0..10 {
            out.enqueue(
                Bytes::from_static(b"0123456789"),
                RakReliability::ReliableOrdered,
                RakPriority::Normal,
                0,
            )
            .unwrap();
        }
        let mut dgrams = Vec::new();
        out.pump(Instant::now(), &mut dgrams);
        assert_eq!(dgrams.len(), 1, "10 条小消息应合并进单个数据报");
    }

    #[test]
    fn large_message_splits_and_respects_mtu() {
        let mut out = outbound();
        let payload = Bytes::from(vec![7u8; 5000]);
        out.enqueue(
            payload,
            RakReliability::ReliableOrdered,
            RakPriority::Normal,
            0,
        )
        .unwrap();
        let mut dgrams = Vec::new();
        out.pump(Instant::now(), &mut dgrams);
        assert!(dgrams.len() >= 5);
        for d in &dgrams {
            assert!(d.len() <= 1200, "数据报 {} 字节超过 MTU 载荷", d.len());
        }
    }

    #[test]
    fn split_upgrades_unreliable_to_reliable() {
        let mut out = outbound();
        out.enqueue(
            Bytes::from(vec![1u8; 3000]),
            RakReliability::Unreliable,
            RakPriority::Normal,
            0,
        )
        .unwrap();
        for frame in &out.queues[RakPriority::Normal.queue_index()] {
            assert_eq!(frame.reliability, RakReliability::Reliable);
            assert!(frame.split.is_some());
        }
    }

    #[test]
    fn rto_requeues_then_ack_clears() {
        let mut out = outbound();
        let t0 = Instant::now();
        out.enqueue(
            Bytes::from_static(b"x"),
            RakReliability::Reliable,
            RakPriority::High,
            0,
        )
        .unwrap();
        let mut dgrams = Vec::new();
        out.pump(t0, &mut dgrams);
        assert_eq!(out.unacked.len(), 1);

        // RTO 到期 → 重新入队并再次发出。
        out.on_tick(t0 + RTO_INITIAL + Duration::from_millis(50));
        assert!(out.unacked.is_empty());
        assert_eq!(out.resend_queue.len(), 1);
        let mut dgrams2 = Vec::new();
        out.pump(t0 + RTO_INITIAL + Duration::from_millis(60), &mut dgrams2);
        assert_eq!(dgrams2.len(), 1);
        assert_eq!(out.unacked.len(), 1);

        // ACK 序号 1（重传使用了新序号）。
        let ranges = AckRanges {
            ranges: vec![(1, 1)],
        };
        out.on_ack(&ranges, t0 + RTO_INITIAL + Duration::from_millis(80));
        assert!(out.unacked.is_empty());
        assert_eq!(out.inflight_bytes, 0);
    }

    #[test]
    fn nack_triggers_fast_resend_with_new_sequence() {
        let mut out = outbound();
        let t0 = Instant::now();
        out.enqueue(
            Bytes::from_static(b"y"),
            RakReliability::Reliable,
            RakPriority::High,
            0,
        )
        .unwrap();
        let mut dgrams = Vec::new();
        out.pump(t0, &mut dgrams);

        out.on_nack(&AckRanges {
            ranges: vec![(0, 0)],
        });
        let mut dgrams2 = Vec::new();
        out.pump(t0, &mut dgrams2);
        assert_eq!(dgrams2.len(), 1);
        // 新数据报使用新序号 1。
        assert_eq!(dgrams2[0][1], 1);
        assert!(out.unacked.contains_key(&1));
    }

    #[test]
    fn retries_exhaustion_marks_dead() {
        let mut out = outbound();
        let mut t = Instant::now();
        out.enqueue(
            Bytes::from_static(b"z"),
            RakReliability::Reliable,
            RakPriority::High,
            0,
        )
        .unwrap();
        for _ in 0..(MAX_RETRIES as usize + 2) {
            let mut dgrams = Vec::new();
            out.pump(t, &mut dgrams);
            t += RTO_MAX * 40;
            out.on_tick(t);
            if out.is_dead() {
                return;
            }
        }
        panic!("重传耗尽后应标记链路死亡");
    }

    #[test]
    fn wide_ack_range_costs_only_inflight_size() {
        // 回归：曾对声明范围逐序号迭代，11 字节的伪造 ACK 能强制
        // 65536 次哈希查找。现在代价与在途数据报数成正比。
        let mut out = outbound();
        let t0 = Instant::now();
        for _ in 0..3 {
            out.enqueue(
                Bytes::from_static(b"x"),
                RakReliability::Reliable,
                RakPriority::High,
                0,
            )
            .unwrap();
            let mut dgrams = Vec::new();
            out.pump(t0, &mut dgrams);
        }
        assert_eq!(out.unacked.len(), 3);
        // 覆盖整个 u24 空间的单条记录。
        let hits = out.matching_unacked(&AckRanges {
            ranges: vec![(0, 0xFF_FFFF)],
        });
        assert_eq!(hits.len(), 3, "只应命中实际在途的 3 个数据报");
        out.on_ack(
            &AckRanges {
                ranges: vec![(0, 0xFF_FFFF)],
            },
            t0,
        );
        assert!(out.unacked.is_empty());
        assert_eq!(out.inflight_bytes, 0);
    }

    #[test]
    fn forged_nacks_do_not_kill_session() {
        // 回归：NACK 重传曾计入死亡预算，十余个伪造 NACK 即可判死会话。
        let mut out = outbound();
        let t0 = Instant::now();
        out.enqueue(
            Bytes::from_static(b"y"),
            RakReliability::Reliable,
            RakPriority::High,
            0,
        )
        .unwrap();
        for round in 0..(MAX_RETRIES as u64 * 3) {
            let mut dgrams = Vec::new();
            out.pump(t0, &mut dgrams);
            out.on_nack(&AckRanges {
                ranges: vec![(0, 0xFF_FFFF)],
            });
            assert!(!out.is_dead(), "第 {round} 轮伪造 NACK 后不应判死");
        }
    }

    #[test]
    fn queue_full_rejected() {
        let mut out = Outbound::new(1200, 1024);
        let r = out.enqueue(
            Bytes::from(vec![0u8; 2048]),
            RakReliability::Reliable,
            RakPriority::Normal,
            0,
        );
        assert!(matches!(r, Err(RakSessionError::QueueFull { .. })));
    }

    #[test]
    fn cwnd_limits_burst_but_immediate_bypasses() {
        let mut out = outbound();
        // 用 Normal 优先级塞入远超初始 cwnd 的数据。
        for _ in 0..200 {
            out.enqueue(
                Bytes::from(vec![0u8; 1000]),
                RakReliability::Reliable,
                RakPriority::Normal,
                0,
            )
            .unwrap();
        }
        let mut dgrams = Vec::new();
        out.pump(Instant::now(), &mut dgrams);
        let sent: usize = dgrams.iter().map(Bytes::len).sum();
        assert!(sent < 200 * 1000, "拥塞窗口应限制突发（实发 {sent}）");

        // Immediate 无视窗口。
        out.enqueue(
            Bytes::from(vec![0u8; 1000]),
            RakReliability::Reliable,
            RakPriority::Immediate,
            0,
        )
        .unwrap();
        let before = dgrams.len();
        out.pump(Instant::now(), &mut dgrams);
        assert!(dgrams.len() > before);
    }
}
