//! 可靠传输引擎（sans-io）。
//!
//! [`ReliabilityEngine`] 聚合出站（拆分、合帧、重传、拥塞控制）与入站
//! （去重、重组、排序、ACK/NACK 生成）两个方向的状态机。它不做任何 IO：
//! 驱动层（raknet-tokio）负责把 `Vec<Bytes>` 里的数据报写入套接字、
//! 把收到的数据报喂给 [`ReliabilityEngine::ingest`]，并按
//! [`RakSessionConfig::autoflush_interval_ms`] 周期调用
//! [`ReliabilityEngine::tick`]。

mod inbound;
mod outbound;

pub use outbound::OutFrame;

use crate::config::RakSessionConfig;
use crate::consts::*;
use crate::error::{RakCodecError, RakSessionError};
use crate::types::{RakPriority, RakReliability};
use crate::wire::acknack::AckRanges;
use crate::wire::connected::{ConnectedPing, ConnectedPong};
use crate::wire::frame::FrameSet;
use bytes::Bytes;
use inbound::Inbound;
use outbound::Outbound;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// 引擎产生的事件。
#[derive(Debug)]
pub enum SessionEvent {
    /// 交付给上层的完整消息（零拷贝：单帧消息为入站数据报切片）。
    Deliver(Bytes),
    /// 对端主动断开（收到 Disconnect）。
    PeerDisconnected,
    /// 链路死亡（静默超时或重传耗尽）。
    Dead,
}

/// 把 u24 线上值展开为最接近 `near` 的逻辑 u64 序号。
pub(crate) fn unwrap24(wire: u32, near: u64) -> u64 {
    const SPAN: i128 = 1 << 24;
    let low = (wire & 0x00FF_FFFF) as i128;
    let near = near as i128;
    let base = near - near % SPAN;
    let mut best: Option<i128> = None;
    for cand in [base + low - SPAN, base + low, base + low + SPAN] {
        if cand < 0 {
            continue;
        }
        match best {
            Some(b) if (cand - near).abs() >= (b - near).abs() => {}
            _ => best = Some(cand),
        }
    }
    best.unwrap_or(low) as u64
}

/// 可靠传输引擎。
pub struct ReliabilityEngine {
    out: Outbound,
    inn: Inbound,
    open: bool,
    last_recv: Instant,
    last_ping: Instant,
    ping_interval: Duration,
    session_timeout: Duration,
}

impl ReliabilityEngine {
    /// `negotiated_mtu` 为握手协商出的 MTU（含 IP/UDP 头开销口径，
    /// 与 OpenConnectionReply 中的取值一致）。
    pub fn new(cfg: &RakSessionConfig, negotiated_mtu: u16, peer: &SocketAddr, now: Instant) -> Self {
        let overhead = UDP_HEADER_SIZE + ip_overhead(peer);
        let mtu_dgram = negotiated_mtu.saturating_sub(overhead).max(128) as usize;
        let channels = (cfg.ordering_channels.clamp(1, MAX_ORDERING_CHANNELS as i32)) as usize;
        let max_queued = if cfg.max_queued_bytes > 0 {
            cfg.max_queued_bytes as usize
        } else {
            MAX_QUEUED_BYTES
        };
        Self {
            out: Outbound::new(mtu_dgram, max_queued),
            inn: Inbound::new(channels),
            open: true,
            last_recv: now,
            last_ping: now,
            ping_interval: cfg.ping_interval,
            session_timeout: cfg.session_timeout,
        }
    }

    #[inline]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 当前平滑 RTT 估计。
    pub fn rtt(&self) -> Duration {
        self.out.rtt()
    }

    /// 入队一条用户消息。之后需调用 [`Self::pump`]（或等下一次 tick）
    /// 才会真正产出数据报。
    pub fn send(
        &mut self,
        payload: Bytes,
        reliability: RakReliability,
        priority: RakPriority,
    ) -> Result<(), RakSessionError> {
        if !self.open {
            return Err(RakSessionError::Closed);
        }
        self.out.enqueue(payload, reliability, priority, 0)
    }

    /// 将可发送的数据报打包进 `out_dgrams`。
    pub fn pump(&mut self, now: Instant, out_dgrams: &mut Vec<Bytes>) {
        self.out.pump(now, out_dgrams);
    }

    /// 处理一个入站在线数据报。
    ///
    /// 返回 `Err` 仅表示该数据报格式非法（丢弃即可），会话不受影响。
    pub fn ingest(
        &mut self,
        datagram: Bytes,
        now: Instant,
        wall_ms: u64,
        out_dgrams: &mut Vec<Bytes>,
        events: &mut Vec<SessionEvent>,
    ) -> Result<(), RakCodecError> {
        if !self.open {
            return Ok(());
        }
        let Some(&first) = datagram.first() else {
            return Ok(());
        };
        if first & FLAG_VALID == 0 {
            return Err(RakCodecError::Malformed("在线数据报缺少 VALID 标志"));
        }
        self.last_recv = now;

        if first & (FLAG_ACK | FLAG_NACK) != 0 {
            let (ranges, is_nack) = AckRanges::decode(datagram)?;
            if is_nack {
                self.out.on_nack(&ranges);
            } else {
                self.out.on_ack(&ranges, now);
            }
            self.pump(now, out_dgrams);
            return Ok(());
        }

        let set = FrameSet::decode(datagram)?;
        let mut deliveries = Vec::new();
        if let Err(overflow) = self.inn.ingest(set, &mut deliveries) {
            // 缓冲上限被击穿：对端异常或恶意，杀死会话。
            tracing::warn!("入站缓冲超限（{overflow}），断开会话");
            self.open = false;
            events.push(SessionEvent::Dead);
            return Ok(());
        }
        for payload in deliveries {
            self.dispatch(payload, wall_ms, events);
        }
        // 积压确认提前冲刷：不等 10ms tick，压低对端的 RTT 观测值，
        // 让拥塞窗口在高吞吐时快速增长。
        if self.inn.pending_ack_count() >= 32 {
            let acks = self.inn.take_acks();
            if !acks.is_empty() {
                out_dgrams.push(acks.encode(false));
            }
        }
        self.pump(now, out_dgrams);
        Ok(())
    }

    fn dispatch(&mut self, payload: Bytes, wall_ms: u64, events: &mut Vec<SessionEvent>) {
        match payload.first().copied() {
            Some(ID_CONNECTED_PING) => {
                let Ok(ping) = ConnectedPing::decode(payload) else { return };
                let pong = ConnectedPong { ping_time_ms: ping.time_ms, pong_time_ms: wall_ms };
                let _ = self.out.enqueue(
                    pong.encode(),
                    RakReliability::Unreliable,
                    RakPriority::Immediate,
                    0,
                );
            }
            Some(ID_CONNECTED_PONG) => {
                // keep-alive：last_recv 已在 ingest 更新，无需额外处理。
            }
            Some(ID_DISCONNECT) => {
                self.open = false;
                events.push(SessionEvent::PeerDisconnected);
            }
            Some(_) => events.push(SessionEvent::Deliver(payload)),
            None => {}
        }
    }

    /// 周期性驱动：冲刷 ACK/NACK、检查重传与超时、发送 keep-alive。
    pub fn tick(
        &mut self,
        now: Instant,
        wall_ms: u64,
        out_dgrams: &mut Vec<Bytes>,
        events: &mut Vec<SessionEvent>,
    ) {
        if !self.open {
            return;
        }
        if now.duration_since(self.last_recv) > self.session_timeout {
            tracing::debug!("会话静默超时，断开");
            self.open = false;
            events.push(SessionEvent::Dead);
            return;
        }

        let acks = self.inn.take_acks();
        if !acks.is_empty() {
            out_dgrams.push(acks.encode(false));
        }
        let nacks = self.inn.take_nacks();
        if !nacks.is_empty() {
            out_dgrams.push(nacks.encode(true));
        }

        self.out.on_tick(now);
        if self.out.is_dead() {
            tracing::debug!("重传次数耗尽，断开");
            self.open = false;
            events.push(SessionEvent::Dead);
            return;
        }

        if now.duration_since(self.last_ping) >= self.ping_interval {
            self.last_ping = now;
            let ping = ConnectedPing { time_ms: wall_ms };
            let _ = self.out.enqueue(
                ping.encode(),
                RakReliability::Unreliable,
                RakPriority::Immediate,
                0,
            );
        }

        self.inn.gc(now);
        self.pump(now, out_dgrams);
    }

    /// 主动断开：发出 Disconnect（单次，不重传）并关闭引擎。
    pub fn disconnect(&mut self, now: Instant, out_dgrams: &mut Vec<Bytes>) -> Result<(), RakSessionError> {
        if !self.open {
            return Err(RakSessionError::Closed);
        }
        let _ = self.out.enqueue(
            crate::wire::connected::encode_disconnect(),
            RakReliability::Unreliable,
            RakPriority::Immediate,
            0,
        );
        self.pump(now, out_dgrams);
        self.open = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap24_basics() {
        assert_eq!(unwrap24(5, 0), 5);
        assert_eq!(unwrap24(5, 3), 5);
        // 接近回绕点：wire 已回绕，near 尚未。
        let near = (1 << 24) - 2;
        assert_eq!(unwrap24(1, near), (1 << 24) + 1);
        // wire 落后于 near（重传旧包）。
        assert_eq!(unwrap24(0xFF_FFFE, 1 << 24), (1 << 24) - 2);
        // 多轮回绕。
        let near = 5 * (1u64 << 24) + 100;
        assert_eq!(unwrap24(90, near), 5 * (1 << 24) + 90);
        assert_eq!(unwrap24(0xFF_FFF0, near), 5 * (1 << 24) - 16);
    }

    fn engine_pair() -> (ReliabilityEngine, ReliabilityEngine, Instant) {
        let cfg = RakSessionConfig::default();
        let addr: SocketAddr = "127.0.0.1:19132".parse().unwrap();
        let now = Instant::now();
        (
            ReliabilityEngine::new(&cfg, 1228, &addr, now),
            ReliabilityEngine::new(&cfg, 1228, &addr, now),
            now,
        )
    }

    /// 在两个引擎之间来回搬运数据报，直到静止。
    fn shuttle(
        a: &mut ReliabilityEngine,
        b: &mut ReliabilityEngine,
        now: Instant,
        delivered_b: &mut Vec<Bytes>,
    ) {
        let mut a_out = Vec::new();
        let mut b_out = Vec::new();
        a.pump(now, &mut a_out);
        for _ in 0..64 {
            let mut events = Vec::new();
            for d in a_out.drain(..) {
                b.ingest(d, now, 0, &mut b_out, &mut events).unwrap();
            }
            for e in events {
                if let SessionEvent::Deliver(p) = e {
                    delivered_b.push(p);
                }
            }
            let mut events = Vec::new();
            for d in b_out.drain(..) {
                a.ingest(d, now, 0, &mut a_out, &mut events).unwrap();
            }
            drop(events);
            if a_out.is_empty() {
                break;
            }
        }
    }

    #[test]
    fn round_trip_small_message() {
        let (mut a, mut b, now) = engine_pair();
        a.send(Bytes::from_static(b"\xFEhello"), RakReliability::ReliableOrdered, RakPriority::High)
            .unwrap();
        let mut got = Vec::new();
        shuttle(&mut a, &mut b, now, &mut got);
        assert_eq!(got.len(), 1);
        assert_eq!(&got[0][..], b"\xFEhello");
    }

    #[test]
    fn round_trip_large_split_message() {
        let (mut a, mut b, now) = engine_pair();
        let big: Vec<u8> = std::iter::once(0xFE)
            .chain((0..200_000u32).map(|i| i as u8))
            .collect();
        a.send(Bytes::from(big.clone()), RakReliability::ReliableOrdered, RakPriority::Normal)
            .unwrap();
        let mut got = Vec::new();
        // 大消息受拥塞窗口限制，需要多轮 tick 才能全部送达。
        let mut now = now;
        for _ in 0..2000 {
            shuttle(&mut a, &mut b, now, &mut got);
            if !got.is_empty() {
                break;
            }
            now += Duration::from_millis(10);
            let mut o = Vec::new();
            let mut e = Vec::new();
            a.tick(now, 0, &mut o, &mut e);
            let mut e2 = Vec::new();
            for d in o {
                b.ingest(d, now, 0, &mut Vec::new(), &mut e2).unwrap();
            }
            for ev in e2 {
                if let SessionEvent::Deliver(p) = ev {
                    got.push(p);
                }
            }
            let mut o2 = Vec::new();
            b.tick(now, 0, &mut o2, &mut Vec::new());
            for d in o2 {
                a.ingest(d, now, 0, &mut Vec::new(), &mut Vec::new()).unwrap();
            }
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].len(), big.len());
        assert_eq!(&got[0][..], &big[..]);
    }

    #[test]
    fn ordered_delivery_over_many_messages() {
        let (mut a, mut b, now) = engine_pair();
        let count = 500u32;
        for i in 0..count {
            let mut payload = vec![0xFEu8];
            payload.extend_from_slice(&i.to_be_bytes());
            a.send(Bytes::from(payload), RakReliability::ReliableOrdered, RakPriority::Normal)
                .unwrap();
        }
        let mut got = Vec::new();
        let mut now = now;
        for _ in 0..1000 {
            shuttle(&mut a, &mut b, now, &mut got);
            if got.len() as u32 == count {
                break;
            }
            now += Duration::from_millis(10);
            let mut o = Vec::new();
            a.tick(now, 0, &mut o, &mut Vec::new());
            let mut e = Vec::new();
            for d in o {
                b.ingest(d, now, 0, &mut Vec::new(), &mut e).unwrap();
            }
            for ev in e {
                if let SessionEvent::Deliver(p) = ev {
                    got.push(p);
                }
            }
            let mut o2 = Vec::new();
            b.tick(now, 0, &mut o2, &mut Vec::new());
            for d in o2 {
                a.ingest(d, now, 0, &mut Vec::new(), &mut Vec::new()).unwrap();
            }
        }
        assert_eq!(got.len() as u32, count);
        for (i, p) in got.iter().enumerate() {
            assert_eq!(u32::from_be_bytes([p[1], p[2], p[3], p[4]]), i as u32);
        }
    }

    #[test]
    fn retransmission_recovers_lost_datagram() {
        let (mut a, mut b, now) = engine_pair();
        a.send(Bytes::from_static(b"\xFEfirst"), RakReliability::ReliableOrdered, RakPriority::High)
            .unwrap();
        // 第一次产出的数据报全部“丢失”。
        let mut lost = Vec::new();
        a.pump(now, &mut lost);
        assert!(!lost.is_empty());
        drop(lost);

        // RTO 之后 tick 触发重传。
        let mut got = Vec::new();
        let mut now2 = now;
        for _ in 0..300 {
            now2 += Duration::from_millis(10);
            let mut o = Vec::new();
            a.tick(now2, 0, &mut o, &mut Vec::new());
            let mut e = Vec::new();
            let mut b_out = Vec::new();
            for d in o {
                b.ingest(d, now2, 0, &mut b_out, &mut e).unwrap();
            }
            for ev in e {
                if let SessionEvent::Deliver(p) = ev {
                    got.push(p);
                }
            }
            let mut o2 = Vec::new();
            b.tick(now2, 0, &mut o2, &mut Vec::new());
            for d in b_out.into_iter().chain(o2) {
                a.ingest(d, now2, 0, &mut Vec::new(), &mut Vec::new()).unwrap();
            }
            if !got.is_empty() {
                break;
            }
        }
        assert_eq!(got.len(), 1);
        assert_eq!(&got[0][..], b"\xFEfirst");
    }

    #[test]
    fn duplicate_datagram_not_delivered_twice() {
        let (mut a, mut b, now) = engine_pair();
        a.send(Bytes::from_static(b"\xFEonce"), RakReliability::ReliableOrdered, RakPriority::High)
            .unwrap();
        let mut out = Vec::new();
        a.pump(now, &mut out);
        assert_eq!(out.len(), 1);
        let dgram = out.pop().unwrap();

        let mut events = Vec::new();
        b.ingest(dgram.clone(), now, 0, &mut Vec::new(), &mut events).unwrap();
        b.ingest(dgram, now, 0, &mut Vec::new(), &mut events).unwrap();
        let delivered = events
            .iter()
            .filter(|e| matches!(e, SessionEvent::Deliver(_)))
            .count();
        assert_eq!(delivered, 1);
    }

    #[test]
    fn disconnect_notifies_peer() {
        let (mut a, mut b, now) = engine_pair();
        let mut out = Vec::new();
        a.disconnect(now, &mut out).unwrap();
        assert!(!a.is_open());
        let mut events = Vec::new();
        for d in out {
            b.ingest(d, now, 0, &mut Vec::new(), &mut events).unwrap();
        }
        assert!(matches!(events.as_slice(), [SessionEvent::PeerDisconnected]));
        assert!(!b.is_open());
    }

    #[test]
    fn silence_timeout_kills_session() {
        let (mut a, _b, now) = engine_pair();
        let mut events = Vec::new();
        a.tick(now + Duration::from_secs(60), 0, &mut Vec::new(), &mut events);
        assert!(matches!(events.as_slice(), [SessionEvent::Dead]));
        assert!(!a.is_open());
    }

    #[test]
    fn ping_answered_with_pong() {
        let (mut a, mut b, now) = engine_pair();
        // b 的 tick 在 ping_interval 之后会发出 ConnectedPing。
        let later = now + Duration::from_secs(3);
        let mut o = Vec::new();
        b.tick(later, 12345, &mut o, &mut Vec::new());
        assert!(!o.is_empty());
        // a 收到 ping 后应立即回 pong（Immediate 直接进 out_dgrams）。
        let mut a_out = Vec::new();
        for d in o {
            a.ingest(d, later, 999, &mut a_out, &mut Vec::new()).unwrap();
        }
        assert!(!a_out.is_empty());
        // pong 送回 b，不产生用户事件。
        let mut events = Vec::new();
        for d in a_out {
            b.ingest(d, later, 0, &mut Vec::new(), &mut events).unwrap();
        }
        assert!(events.is_empty());
    }
}
