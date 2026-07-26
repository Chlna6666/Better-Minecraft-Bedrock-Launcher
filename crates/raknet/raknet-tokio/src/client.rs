//! 客户端：unconnected ping 与 go-raknet 风格的拨号状态机。
//!
//! 拨号完全在驱动任务内单线程推进（收包路由与重试定时都在同一个
//! `select!` 循环里），不存在跨任务的握手竞态。

use crate::session::{RakSession, SessionShared, wall_ms};
use bytes::{Bytes, BytesMut};
use raknet::config::{RakClientConfig, RakSessionConfig};
use raknet::consts::*;
use raknet::error::RakClientError;
use raknet::wire::connected::{
    ConnectionRequest, ConnectionRequestAccepted, NewIncomingConnection,
};
use raknet::wire::offline::{
    IncompatibleProtocol, OpenConnectionReply1, OpenConnectionReply2, OpenConnectionRequest1,
    OpenConnectionRequest2, UnconnectedPing, UnconnectedPong,
};
use raknet::types::{RakPriority, RakReliability};
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

type PingWaiter = (oneshot::Sender<(Box<[u8]>, Duration)>, Instant);
type SessionSlot = Arc<Mutex<Option<Arc<SessionShared>>>>;

/// unconnected ping 的等待上限；超时后调用方得到 `Closed` 而非永久挂起。
const PING_TIMEOUT: Duration = Duration::from_secs(5);

enum Ctl {
    Ping(SocketAddr, oneshot::Sender<(Box<[u8]>, Duration)>),
    Connect(SocketAddr, oneshot::Sender<Result<RakSession, RakClientError>>),
}

enum State {
    Init { config: RakClientConfig },
    Running(Running),
    Stopped,
}

struct Running {
    handle: JoinHandle<()>,
    ctl_tx: mpsc::UnboundedSender<Ctl>,
    session_slot: SessionSlot,
}

/// RakNet 客户端。
pub struct RakClient {
    state: State,
}

impl RakClient {
    pub fn new<F>(conf: F) -> Self
    where
        F: FnOnce(&mut RakClientConfig),
    {
        let mut config = RakClientConfig::default();
        conf(&mut config);
        Self { state: State::Init { config } }
    }

    /// 绑定套接字并启动驱动任务。
    pub async fn start(&mut self) -> Result<(), RakClientError> {
        let State::Init { config } = &self.state else {
            return Ok(());
        };
        let socket = Arc::new(crate::net::bind_udp(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))?);
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let session_slot: SessionSlot = Arc::new(Mutex::new(None));

        let driver = Driver::new(socket, config.clone(), session_slot.clone(), ctl_rx);
        let handle = tokio::spawn(driver.run());

        self.state = State::Running(Running { handle, ctl_tx, session_slot });
        Ok(())
    }

    /// 建立到 `addr` 的连接。
    pub async fn connect(&self, addr: SocketAddr) -> Result<RakSession, RakClientError> {
        let State::Running(running) = &self.state else {
            return Err(RakClientError::Closed);
        };
        let (tx, rx) = oneshot::channel();
        running
            .ctl_tx
            .send(Ctl::Connect(addr, tx))
            .map_err(|_| RakClientError::Closed)?;
        rx.await.map_err(|_| RakClientError::Closed)?
    }

    /// 向任意地址发送 unconnected ping，返回（MOTD, RTT）。
    pub async fn ping(&self, addr: SocketAddr) -> Result<(Box<[u8]>, Duration), RakClientError> {
        let State::Running(running) = &self.state else {
            return Err(RakClientError::Closed);
        };
        let (tx, rx) = oneshot::channel();
        running
            .ctl_tx
            .send(Ctl::Ping(addr, tx))
            .map_err(|_| RakClientError::Closed)?;
        rx.await.map_err(|_| RakClientError::Closed)
    }

    /// 停止客户端并关闭会话。
    pub fn stop(&mut self) {
        if let State::Running(running) = &self.state {
            let shared = running
                .session_slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(shared) = shared {
                shared.mark_closed();
            }
            running.handle.abort();
        }
        self.state = State::Stopped;
    }
}

impl Drop for RakClient {
    fn drop(&mut self) {
        self.stop();
    }
}

enum DialStage {
    /// 发送 OCR1，等待 Reply1（按尝试次数递减 MTU）。
    Ocr1,
    /// 发送 OCR2（回显 cookie），等待 Reply2。
    Ocr2,
    /// ConnectionRequest 已通过可靠层发出，等待 CRA。
    Request,
}

struct Dial {
    addr: SocketAddr,
    result_tx: oneshot::Sender<Result<RakSession, RakClientError>>,
    stage: DialStage,
    started: Instant,
    last_send: Instant,
    attempts: usize,
    mtu_ladder: Vec<u16>,
    /// Reply1 协商出的 MTU。
    mtu: u16,
    cookie: Option<i32>,
    shared: Option<Arc<SessionShared>>,
}

struct Driver {
    socket: Arc<UdpSocket>,
    config: RakClientConfig,
    session_slot: SessionSlot,
    dial: Option<Dial>,
    pings: HashMap<SocketAddr, VecDeque<PingWaiter>>,
    ctl_rx: mpsc::UnboundedReceiver<Ctl>,
    dead_tx: mpsc::UnboundedSender<SocketAddr>,
    dead_rx: mpsc::UnboundedReceiver<SocketAddr>,
}

impl Driver {
    fn new(
        socket: Arc<UdpSocket>,
        config: RakClientConfig,
        session_slot: SessionSlot,
        ctl_rx: mpsc::UnboundedReceiver<Ctl>,
    ) -> Self {
        let (dead_tx, dead_rx) = mpsc::unbounded_channel();
        Self {
            socket,
            config,
            session_slot,
            dial: None,
            pings: HashMap::new(),
            ctl_rx,
            dead_tx,
            dead_rx,
        }
    }

    fn active_session(&self) -> Option<Arc<SessionShared>> {
        self.session_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn run(mut self) {
        let mut buf = BytesMut::with_capacity(64 * 1024);
        let mut timer = tokio::time::interval(Duration::from_millis(50));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                            tracing::debug!("客户端接收失败：{error}");
                        }
                    }
                }
                Some(ctl) = self.ctl_rx.recv() => self.handle_ctl(ctl).await,
                Some(addr) = self.dead_rx.recv() => {
                    // 同 server：只清理仍关闭的条目，避免把重连到同地址的
                    // 新会话误清（清空后所有在线数据报都会无处路由）。
                    let mut slot = self
                        .session_slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if slot.as_ref().is_some_and(|s| s.addr == addr && s.is_closed()) {
                        *slot = None;
                    }
                    drop(slot);
                }
                _ = timer.tick() => {
                    self.expire_pings();
                    self.dial_tick().await;
                }
            }
        }
    }

    async fn handle_ctl(&mut self, ctl: Ctl) {
        match ctl {
            Ctl::Ping(addr, tx) => {
                let ping = UnconnectedPing { time_ms: wall_ms(), client_guid: self.config.guid };
                if self.socket.send_to(&ping.encode(), addr).await.is_ok() {
                    self.pings.entry(addr).or_default().push_back((tx, Instant::now()));
                }
                // 发送失败：直接丢弃 tx，调用方收到 Closed。
            }
            Ctl::Connect(addr, tx) => {
                let has_session = self.active_session().is_some_and(|s| !s.is_closed());
                if has_session || self.dial.is_some() {
                    let _ = tx.send(Err(RakClientError::AlreadyConnected));
                    return;
                }
                let ladder = mtu_ladder(&self.config);
                let mut dial = Dial {
                    addr,
                    result_tx: tx,
                    stage: DialStage::Ocr1,
                    started: Instant::now(),
                    last_send: Instant::now(),
                    attempts: 0,
                    mtu_ladder: ladder,
                    mtu: self.config.max_mtu_size,
                    cookie: None,
                    shared: None,
                };
                self.send_ocr1(&mut dial).await;
                self.dial = Some(dial);
            }
        }
    }

    async fn handle_datagram(&mut self, datagram: Bytes, addr: SocketAddr) {
        let Some(&first) = datagram.first() else { return };

        if first & FLAG_VALID != 0 {
            let session = self.active_session();
            if let Some(shared) = session
                && shared.addr == addr
            {
                let hs = shared.ingest(datagram).await;
                for packet in hs {
                    self.handle_handshake_delivery(packet).await;
                }
            }
            return;
        }

        match first {
            ID_UNCONNECTED_PONG => {
                let Ok(pong) = UnconnectedPong::decode(datagram) else { return };
                if let Some(queue) = self.pings.get_mut(&addr)
                    && let Some((tx, sent_at)) = queue.pop_front()
                {
                    let motd: Box<[u8]> = Box::from(&pong.motd[..]);
                    let _ = tx.send((motd, sent_at.elapsed()));
                }
            }
            _ => self.handle_offline_dial(datagram, addr).await,
        }
    }

    /// 拨号阶段的离线应答。
    async fn handle_offline_dial(&mut self, datagram: Bytes, addr: SocketAddr) {
        let Some(dial) = &mut self.dial else { return };
        if dial.addr != addr {
            return;
        }
        let Some(&first) = datagram.first() else { return };
        match first {
            ID_OPEN_CONNECTION_REPLY_1 => {
                if !matches!(dial.stage, DialStage::Ocr1) {
                    return;
                }
                let Ok(reply) = OpenConnectionReply1::decode(datagram) else { return };
                dial.cookie = reply.cookie;
                dial.mtu = reply.mtu.clamp(MIN_MTU_SIZE, 1600);
                dial.stage = DialStage::Ocr2;
                dial.attempts = 0;
                self.send_ocr2().await;
            }
            ID_OPEN_CONNECTION_REPLY_2 => {
                if !matches!(dial.stage, DialStage::Ocr2) {
                    return;
                }
                let Ok(reply) = OpenConnectionReply2::decode(datagram) else { return };
                if reply.security {
                    self.fail_dial(RakClientError::SecurityUnsupported);
                    return;
                }
                let mtu = reply.mtu.clamp(MIN_MTU_SIZE, 1600);
                let session_cfg = RakSessionConfig::default();
                let shared = SessionShared::new(
                    addr,
                    self.socket.clone(),
                    &session_cfg,
                    mtu,
                    self.dead_tx.clone(),
                );
                shared.spawn_ticker();
                *self
                    .session_slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(shared.clone());

                let request = ConnectionRequest {
                    client_guid: self.config.guid,
                    time_ms: wall_ms(),
                    security: false,
                };
                let send_result = shared
                    .send_payload(
                        request.encode(),
                        RakReliability::ReliableOrdered,
                        RakPriority::Immediate,
                    )
                    .await;
                let Some(dial) = &mut self.dial else { return };
                if let Err(error) = send_result {
                    tracing::debug!("发送 ConnectionRequest 失败：{error}");
                    self.fail_dial(RakClientError::Session(error));
                    return;
                }
                dial.shared = Some(shared);
                dial.stage = DialStage::Request;
                dial.last_send = Instant::now();
            }
            ID_INCOMPATIBLE_PROTOCOL => {
                let server_protocol = IncompatibleProtocol::decode(datagram)
                    .map(|p| p.protocol)
                    .unwrap_or(0);
                self.fail_dial(RakClientError::IncompatibleProtocol { server_protocol });
            }
            ID_ALREADY_CONNECTED => self.fail_dial(RakClientError::AlreadyConnected),
            ID_NO_FREE_INCOMING_CONNECTIONS => {
                self.fail_dial(RakClientError::NoFreeIncomingConnections);
            }
            ID_IP_RECENTLY_CONNECTED => self.fail_dial(RakClientError::RecentlyConnected),
            other => {
                tracing::debug!("拨号阶段收到未知离线报文 {other:#04X}");
            }
        }
    }

    /// 握手期在线交付（CRA 等）。
    async fn handle_handshake_delivery(&mut self, packet: Bytes) {
        let Some(dial) = &mut self.dial else { return };
        match packet.first().copied() {
            Some(ID_CONNECTION_REQUEST_ACCEPTED) => {
                if !matches!(dial.stage, DialStage::Request) {
                    return;
                }
                let Ok(accepted) = ConnectionRequestAccepted::decode(packet) else { return };
                let Some(shared) = dial.shared.clone() else { return };
                let nic = NewIncomingConnection {
                    server_address: dial.addr,
                    request_time_ms: accepted.time_ms,
                    time_ms: wall_ms(),
                };
                let send_result = shared
                    .send_payload(
                        nic.encode(),
                        RakReliability::ReliableOrdered,
                        RakPriority::Immediate,
                    )
                    .await;
                if let Err(error) = send_result {
                    self.fail_dial(RakClientError::Session(error));
                    return;
                }
                shared.set_connected();
                let Some(dial) = self.dial.take() else { return };
                if let Some(rx) = shared.take_receiver() {
                    let session = RakSession::from_shared(shared, rx);
                    let _ = dial.result_tx.send(Ok(session));
                } else {
                    let _ = dial.result_tx.send(Err(RakClientError::Closed));
                }
            }
            Some(ID_CONNECTION_REQUEST_FAILED) => {
                self.fail_dial(RakClientError::ConnectionRequestFailed);
            }
            Some(other) => {
                tracing::debug!("握手阶段收到意外在线报文 {other:#04X}");
            }
            None => {}
        }
    }

    /// 丢弃超时的 ping 等待者：否则对无响应地址的 `ping()` 会永久挂起，
    /// 且 `pings` 表随每次调用无界增长。
    fn expire_pings(&mut self) {
        if self.pings.is_empty() {
            return;
        }
        let now = Instant::now();
        self.pings.retain(|_, queue| {
            // 队列按发送时间递增，从头部丢弃过期项即可。
            while queue
                .front()
                .is_some_and(|(_, sent_at)| now.duration_since(*sent_at) > PING_TIMEOUT)
            {
                queue.pop_front(); // drop sender → 调用方得到 Closed
            }
            !queue.is_empty()
        });
    }

    /// 重试与超时驱动。
    async fn dial_tick(&mut self) {
        let Some(dial) = &mut self.dial else { return };
        // 调用方已取消 connect()：立刻放弃拨号，否则状态机会继续重试到
        // 总超时为止，期间所有新的 connect() 都被误报 AlreadyConnected。
        if dial.result_tx.is_closed() {
            tracing::debug!("connect() 已被调用方取消，终止拨号");
            self.fail_dial(RakClientError::Closed);
            return;
        }
        let now = Instant::now();
        if now.duration_since(dial.started) > self.config.conn_attempt_timeout {
            self.fail_dial(RakClientError::Timeout);
            return;
        }
        match dial.stage {
            DialStage::Ocr1 => {
                if now.duration_since(dial.last_send) >= self.config.conn_attempt_interval {
                    if dial.attempts >= self.config.conn_attempt_max {
                        let attempts = dial.attempts;
                        self.fail_dial(RakClientError::ConnectionFailed { attempts });
                        return;
                    }
                    self.send_ocr1_current().await;
                }
            }
            DialStage::Ocr2 => {
                if now.duration_since(dial.last_send) >= self.config.conn_attempt_interval {
                    if dial.attempts >= self.config.conn_attempt_max {
                        let attempts = dial.attempts;
                        self.fail_dial(RakClientError::ConnectionFailed { attempts });
                        return;
                    }
                    self.send_ocr2().await;
                }
            }
            DialStage::Request => {
                // ConnectionRequest 走可靠层自动重传，这里只等 CRA 或总超时。
            }
        }
    }

    async fn send_ocr1(&mut self, dial: &mut Dial) {
        let idx = dial.attempts * dial.mtu_ladder.len() / self.config.conn_attempt_max.max(1);
        let mtu = dial.mtu_ladder[idx.min(dial.mtu_ladder.len() - 1)];
        // 探测报文的 UDP 载荷 = MTU - 28，使 IP 包大小恰好等于被探测的 MTU。
        let request = OpenConnectionRequest1 {
            protocol: self.config.protocol,
            mtu_payload: mtu.saturating_sub(OCR1_MTU_OVERHEAD),
        };
        let _ = self.socket.send_to(&request.encode(), dial.addr).await;
        dial.attempts += 1;
        dial.last_send = Instant::now();
    }

    async fn send_ocr1_current(&mut self) {
        let Some(mut dial) = self.dial.take() else { return };
        self.send_ocr1(&mut dial).await;
        self.dial = Some(dial);
    }

    async fn send_ocr2(&mut self) {
        let Some(dial) = &mut self.dial else { return };
        let request = OpenConnectionRequest2 {
            cookie: dial.cookie,
            server_address: dial.addr,
            mtu: dial.mtu,
            client_guid: self.config.guid,
        };
        let addr = dial.addr;
        let encoded = request.encode();
        dial.attempts += 1;
        dial.last_send = Instant::now();
        let _ = self.socket.send_to(&encoded, addr).await;
    }

    fn fail_dial(&mut self, error: RakClientError) {
        let Some(dial) = self.dial.take() else { return };
        if let Some(shared) = dial.shared {
            shared.mark_closed();
            let mut slot = self
                .session_slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, &shared)) {
                *slot = None;
            }
        }
        let _ = dial.result_tx.send(Err(error));
    }
}

/// MTU 阶梯（go-raknet 风格：从大到小尝试）。
fn mtu_ladder(config: &RakClientConfig) -> Vec<u16> {
    let mut ladder: Vec<u16> = [config.max_mtu_size, 1200, 576]
        .into_iter()
        .filter(|&m| m >= config.min_mtu_size && m <= config.max_mtu_size)
        .collect();
    ladder.sort_unstable_by(|a, b| b.cmp(a));
    ladder.dedup();
    if ladder.is_empty() {
        ladder.push(config.max_mtu_size.max(config.min_mtu_size));
    }
    ladder
}
