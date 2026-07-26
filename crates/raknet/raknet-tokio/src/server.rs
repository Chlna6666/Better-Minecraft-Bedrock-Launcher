//! 服务端：监听、离线握手、会话路由与接入队列。

use crate::session::{RakSession, SessionShared, wall_ms};
use bytes::{Bytes, BytesMut};
use raknet::config::{RakServerConfig, RakSessionConfig};
use raknet::consts::*;
use raknet::wire::connected::{ConnectionRequest, ConnectionRequestAccepted, NewIncomingConnection};
use raknet::wire::offline::{
    IncompatibleProtocol, OpenConnectionReply1, OpenConnectionReply2, OpenConnectionRequest1,
    OpenConnectionRequest2, UnconnectedPing, UnconnectedPong, encode_simple_refusal,
};
use raknet::types::{RakPriority, RakReliability};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// 服务端错误。
#[derive(Debug, Error)]
pub enum RakServerError {
    #[error("服务端已关闭")]
    Closed,
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

type SessionMap = Arc<Mutex<HashMap<SocketAddr, Arc<SessionShared>>>>;

enum Ctl {
    SetMessage(Box<[u8]>),
    SetMaxConnections(usize),
}

enum State {
    Init { addr: SocketAddr, config: RakServerConfig },
    Running(Running),
    Stopped,
}

struct Running {
    handle: JoinHandle<()>,
    accept_rx: mpsc::UnboundedReceiver<RakSession>,
    ctl_tx: mpsc::UnboundedSender<Ctl>,
    sessions: SessionMap,
}

/// RakNet 服务端。
pub struct RakServer {
    state: State,
}

impl RakServer {
    pub fn new<F>(addr: SocketAddr, conf: F) -> Self
    where
        F: FnOnce(&mut RakServerConfig),
    {
        let mut config = RakServerConfig::default();
        conf(&mut config);
        Self { state: State::Init { addr, config } }
    }

    /// 更新 Unconnected Pong 返回的 MOTD。
    pub fn set_message<T>(&mut self, val: T)
    where
        T: Into<Box<[u8]>>,
    {
        let message = val.into();
        match &mut self.state {
            State::Init { config, .. } => config.message = message,
            State::Running(running) => {
                let _ = running.ctl_tx.send(Ctl::SetMessage(message));
            }
            State::Stopped => {}
        }
    }

    pub fn set_max_connections(&mut self, val: usize) {
        match &mut self.state {
            State::Init { config, .. } => config.max_connections = val,
            State::Running(running) => {
                let _ = running.ctl_tx.send(Ctl::SetMaxConnections(val));
            }
            State::Stopped => {}
        }
    }

    /// 绑定套接字并启动驱动任务。
    pub async fn start(&mut self) -> Result<(), RakServerError> {
        let State::Init { addr, config } = &self.state else {
            return Ok(());
        };
        let socket = Arc::new(crate::net::bind_udp(*addr)?);
        let (accept_tx, accept_rx) = mpsc::unbounded_channel();
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));

        let driver = Driver::new(socket, config.clone(), sessions.clone(), accept_tx, ctl_rx);
        let handle = tokio::spawn(driver.run());

        self.state = State::Running(Running { handle, accept_rx, ctl_tx, sessions });
        Ok(())
    }

    /// 接受一个完成握手的会话。
    pub async fn accept(&mut self) -> Result<RakSession, RakServerError> {
        let State::Running(running) = &mut self.state else {
            return Err(RakServerError::Closed);
        };
        running.accept_rx.recv().await.ok_or(RakServerError::Closed)
    }

    /// 停止服务端并关闭全部会话。
    pub fn stop(&mut self) {
        if let State::Running(running) = &self.state {
            let sessions = running
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain()
                .map(|(_, s)| s)
                .collect::<Vec<_>>();
            for shared in sessions {
                shared.mark_closed();
            }
            running.handle.abort();
        }
        self.state = State::Stopped;
    }
}

impl Drop for RakServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 离线/握手报文限速器（10ms 窗口）。
struct Limiter {
    window_start: Instant,
    per_addr: HashMap<SocketAddr, u32>,
    total: u32,
    per_limit: Option<u32>,
    total_limit: Option<u32>,
}

impl Limiter {
    fn new(per_limit: i32, total_limit: i32) -> Self {
        Self {
            window_start: Instant::now(),
            per_addr: HashMap::new(),
            total: 0,
            per_limit: u32::try_from(per_limit).ok().filter(|&v| v > 0),
            total_limit: u32::try_from(total_limit).ok().filter(|&v| v > 0),
        }
    }

    fn allow(&mut self, addr: SocketAddr, now: Instant) -> bool {
        if now.duration_since(self.window_start) > Duration::from_millis(10) {
            self.window_start = now;
            self.per_addr.clear();
            self.total = 0;
        }
        self.total += 1;
        if let Some(limit) = self.total_limit
            && self.total > limit
        {
            return false;
        }
        if let Some(limit) = self.per_limit {
            let count = self.per_addr.entry(addr).or_insert(0);
            *count += 1;
            if *count > limit {
                return false;
            }
        }
        true
    }
}

struct PendingCookie {
    cookie: i32,
    at: Instant,
}

struct Driver {
    socket: Arc<UdpSocket>,
    config: RakServerConfig,
    sessions: SessionMap,
    /// security 开启时：OCR1 已应答、等待 OCR2 回显 cookie 的地址。
    pending_cookies: HashMap<SocketAddr, PendingCookie>,
    accept_tx: mpsc::UnboundedSender<RakSession>,
    ctl_rx: mpsc::UnboundedReceiver<Ctl>,
    dead_tx: mpsc::UnboundedSender<SocketAddr>,
    dead_rx: mpsc::UnboundedReceiver<SocketAddr>,
    limiter: Limiter,
}

impl Driver {
    fn new(
        socket: Arc<UdpSocket>,
        config: RakServerConfig,
        sessions: SessionMap,
        accept_tx: mpsc::UnboundedSender<RakSession>,
        ctl_rx: mpsc::UnboundedReceiver<Ctl>,
    ) -> Self {
        let (dead_tx, dead_rx) = mpsc::unbounded_channel();
        let limiter = Limiter::new(config.packet_limit, config.total_packet_limit);
        Self {
            socket,
            config,
            sessions,
            pending_cookies: HashMap::new(),
            accept_tx,
            ctl_rx,
            dead_tx,
            dead_rx,
            limiter,
        }
    }

    async fn run(mut self) {
        // 接收缓冲：recv_buf_from 写入未初始化容量，无重复清零开销；
        // split().freeze() 后数据报与其中的帧载荷全程零拷贝。
        let mut buf = BytesMut::with_capacity(64 * 1024);
        let mut sweep = tokio::time::interval(Duration::from_secs(5));
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if buf.capacity() < MAX_DATAGRAM_SIZE {
                buf = BytesMut::with_capacity(64 * 1024);
            }
            tokio::select! {
                received = self.socket.recv_buf_from(&mut buf) => {
                    match received {
                        Ok((_, addr)) => {
                            let datagram = buf.split().freeze();
                            self.handle_datagram(datagram, addr).await;
                        }
                        Err(error) => {
                            // Windows 上 ICMP 端口不可达会表现为 recv 错误，忽略继续。
                            tracing::debug!("服务端接收失败：{error}");
                        }
                    }
                }
                Some(ctl) = self.ctl_rx.recv() => match ctl {
                    Ctl::SetMessage(message) => self.config.message = message,
                    Ctl::SetMaxConnections(n) => self.config.max_connections = n,
                },
                Some(addr) = self.dead_rx.recv() => {
                    // 只清理仍处于关闭状态的条目：同地址客户端可能已在
                    // 这条通知排队期间重连并写入新会话，按地址盲删会把
                    // 新会话从路由表里抹掉，导致其握手报文全部被丢弃。
                    let mut sessions = self
                        .sessions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if sessions.get(&addr).is_some_and(|s| s.is_closed()) {
                        sessions.remove(&addr);
                    }
                    drop(sessions);
                }
                _ = sweep.tick() => {
                    let now = Instant::now();
                    self.pending_cookies.retain(|_, p| now.duration_since(p.at) < Duration::from_secs(10));
                }
            }
        }
    }

    async fn handle_datagram(&mut self, datagram: Bytes, addr: SocketAddr) {
        let Some(&first) = datagram.first() else { return };
        if first & FLAG_VALID != 0 {
            let shared = self
                .sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&addr)
                .cloned();
            if let Some(shared) = shared {
                let hs = shared.ingest(datagram).await;
                if !hs.is_empty() {
                    self.handle_handshake(&shared, addr, hs).await;
                }
            }
            return;
        }

        // 离线报文：限速后处理。
        if !self.limiter.allow(addr, Instant::now()) {
            return;
        }
        match first {
            ID_UNCONNECTED_PING | ID_UNCONNECTED_PING_OPEN_CONNECTIONS => {
                self.handle_unconnected_ping(datagram, addr).await;
            }
            ID_OPEN_CONNECTION_REQUEST_1 => {
                self.handle_ocr1(datagram, addr).await;
            }
            ID_OPEN_CONNECTION_REQUEST_2 => {
                self.handle_ocr2(datagram, addr).await;
            }
            other => {
                tracing::debug!(addr = %addr, "未知离线报文 {other:#04X}");
            }
        }
    }

    async fn handle_unconnected_ping(&mut self, datagram: Bytes, addr: SocketAddr) {
        let Ok(ping) = UnconnectedPing::decode(datagram) else { return };
        let pong = UnconnectedPong {
            time_ms: ping.time_ms,
            server_guid: self.config.guid,
            motd: Bytes::copy_from_slice(&self.config.message),
        };
        let _ = self.socket.send_to(&pong.encode(), addr).await;
    }

    async fn handle_ocr1(&mut self, datagram: Bytes, addr: SocketAddr) {
        let Ok(request) = OpenConnectionRequest1::decode(datagram) else { return };
        if !self.config.protocols.contains(&request.protocol) {
            let reply = IncompatibleProtocol {
                protocol: self.config.protocols.first().copied().unwrap_or(PROTOCOL),
                server_guid: self.config.guid,
            };
            let _ = self.socket.send_to(&reply.encode(), addr).await;
            return;
        }
        // 以收到的 UDP 载荷长度反推 MTU（线上固定 +28 口径）。
        let mtu = request
            .mtu_payload
            .saturating_add(OCR1_MTU_OVERHEAD)
            .clamp(self.config.min_mtu_size, self.config.max_mtu_size);
        let cookie = if self.config.security {
            let cookie = rand::random::<i32>();
            self.pending_cookies.insert(addr, PendingCookie { cookie, at: Instant::now() });
            Some(cookie)
        } else {
            None
        };
        let reply = OpenConnectionReply1 { server_guid: self.config.guid, cookie, mtu };
        let _ = self.socket.send_to(&reply.encode(), addr).await;
    }

    async fn handle_ocr2(&mut self, datagram: Bytes, addr: SocketAddr) {
        let Ok(request) = OpenConnectionRequest2::decode(datagram) else { return };

        if self.config.security {
            let Some(pending) = self.pending_cookies.get(&addr) else {
                tracing::debug!(addr = %addr, "OCR2 缺少 cookie 握手前置，丢弃");
                return;
            };
            if request.cookie != Some(pending.cookie) {
                tracing::debug!(addr = %addr, "OCR2 cookie 不匹配，丢弃");
                return;
            }
        }

        let existing = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&addr)
            .cloned();
        if let Some(shared) = existing {
            // Reply2 丢失导致的重发：幂等地重发 Reply2；已建立的连接则拒绝。
            if shared.is_closed() {
                self.sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&addr);
            } else {
                let reply = OpenConnectionReply2 {
                    server_guid: self.config.guid,
                    client_address: addr,
                    mtu: shared.negotiated_mtu,
                    security: false,
                };
                let _ = self.socket.send_to(&reply.encode(), addr).await;
                return;
            }
        }

        let mtu = request.mtu.clamp(self.config.min_mtu_size, self.config.max_mtu_size);
        // 已关闭但尚未被 dead 通知清理的会话不占名额。
        let session_count = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|s| !s.is_closed())
            .count();
        if session_count >= self.config.max_connections {
            let refusal = encode_simple_refusal(ID_NO_FREE_INCOMING_CONNECTIONS, self.config.guid);
            let _ = self.socket.send_to(&refusal, addr).await;
            return;
        }

        let reply = OpenConnectionReply2 {
            server_guid: self.config.guid,
            client_address: addr,
            mtu,
            security: false,
        };
        let _ = self.socket.send_to(&reply.encode(), addr).await;

        let session_cfg = RakSessionConfig {
            ordering_channels: self.config.max_ordering_channels,
            ..RakSessionConfig::default()
        };
        let shared = SessionShared::new(addr, self.socket.clone(), &session_cfg, mtu, self.dead_tx.clone());
        shared.spawn_ticker();
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(addr, shared);
        self.pending_cookies.remove(&addr);
        tracing::debug!(addr = %addr, mtu, "握手进入在线阶段");
    }

    /// 处理握手期会话交付的在线控制包。
    async fn handle_handshake(&mut self, shared: &Arc<SessionShared>, addr: SocketAddr, packets: Vec<Bytes>) {
        let mut connected = false;
        for packet in packets {
            if connected {
                // NIC 之后同批到达的即为用户数据。
                shared.push_incoming(packet);
                continue;
            }
            match packet.first().copied() {
                Some(ID_CONNECTION_REQUEST) => {
                    let Ok(request) = ConnectionRequest::decode(packet) else { continue };
                    let accepted = ConnectionRequestAccepted {
                        client_address: addr,
                        system_index: 0,
                        request_time_ms: request.time_ms,
                        time_ms: wall_ms(),
                    };
                    if let Err(error) = shared
                        .send_payload(
                            accepted.encode(),
                            RakReliability::ReliableOrdered,
                            RakPriority::Immediate,
                        )
                        .await
                    {
                        tracing::debug!(addr = %addr, "发送 ConnectionRequestAccepted 失败：{error}");
                    }
                }
                Some(ID_NEW_INCOMING_CONNECTION) => {
                    if NewIncomingConnection::decode(packet).is_err() {
                        continue;
                    }
                    shared.set_connected();
                    connected = true;
                    if let Some(rx) = shared.take_receiver() {
                        let session = RakSession::from_shared(shared.clone(), rx);
                        if self.accept_tx.send(session).is_err() {
                            shared.mark_closed();
                        }
                        tracing::debug!(addr = %addr, "会话建立");
                    }
                }
                Some(other) => {
                    tracing::debug!(addr = %addr, "握手阶段收到意外报文 {other:#04X}");
                }
                None => {}
            }
        }
    }
}
