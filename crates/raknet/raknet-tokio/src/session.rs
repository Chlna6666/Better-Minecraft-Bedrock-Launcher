//! 会话：可靠传输引擎的异步驱动与公开句柄。
//!
//! 并发模型（go-raknet 风格）：
//! - 引擎状态由 `std::sync::Mutex` 保护，临界区内绝不 await；
//! - 入站数据报由套接字接收任务调用 [`SessionShared::ingest`]；
//! - 每会话一个 tick 任务负责 ACK 冲刷、重传与 keep-alive；
//! - [`RakSession::send`] 短暂持锁打包后直接写套接字，无 actor 往返；
//! - 需要并发收发时可把会话拆成 [`RakSendHandle`] 与 [`RakReceiver`]，发送不再
//!   经过接收端所有权或额外异步锁。

use bytes::Bytes;
use raknet::config::RakSessionConfig;
use raknet::error::RakSessionError;
use raknet::reliability::{ReliabilityEngine, SessionEvent};
use raknet::types::{RakPriority, RakReliability};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

/// 当前 Unix 毫秒时间戳（用于线上 ping/pong 时间字段）。
pub(crate) fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn lock_engine(m: &Mutex<ReliabilityEngine>) -> MutexGuard<'_, ReliabilityEngine> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 会话共享状态。驱动任务与用户句柄共同持有。
pub(crate) struct SessionShared {
    pub(crate) addr: SocketAddr,
    /// 握手协商出的 MTU（用于重发 OpenConnectionReply2）。
    pub(crate) negotiated_mtu: u16,
    socket: Arc<UdpSocket>,
    engine: Mutex<ReliabilityEngine>,
    /// 握手期间为 true：Deliver 事件交还驱动处理而非推给用户。
    handshaking: AtomicBool,
    closed: AtomicBool,
    incoming_tx: Mutex<Option<mpsc::UnboundedSender<Bytes>>>,
    incoming_rx: Mutex<Option<mpsc::UnboundedReceiver<Bytes>>>,
    /// 会话终结时通知驱动清理路由表。
    dead_tx: mpsc::UnboundedSender<SocketAddr>,
    tick_interval: Duration,
}

impl SessionShared {
    pub fn new(
        addr: SocketAddr,
        socket: Arc<UdpSocket>,
        cfg: &RakSessionConfig,
        negotiated_mtu: u16,
        dead_tx: mpsc::UnboundedSender<SocketAddr>,
    ) -> Arc<Self> {
        let engine = ReliabilityEngine::new(cfg, negotiated_mtu, &addr, Instant::now());
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            addr,
            negotiated_mtu,
            socket,
            engine: Mutex::new(engine),
            handshaking: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            incoming_tx: Mutex::new(Some(tx)),
            incoming_rx: Mutex::new(Some(rx)),
            dead_tx,
            tick_interval: cfg.autoflush_interval_ms,
        })
    }

    /// 启动本会话的 tick 任务。
    pub fn spawn_ticker(self: &Arc<Self>) {
        let shared = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(shared.tick_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if shared.is_closed() {
                    break;
                }
                let mut out = Vec::new();
                let mut events = Vec::new();
                lock_engine(&shared.engine).tick(Instant::now(), wall_ms(), &mut out, &mut events);
                shared.send_all(&out).await;
                for event in events {
                    match event {
                        SessionEvent::Deliver(p) => shared.push_incoming(p),
                        SessionEvent::PeerDisconnected | SessionEvent::Dead => shared.mark_closed(),
                    }
                }
            }
        });
    }

    /// 处理一个入站在线数据报。
    ///
    /// 返回握手期间需要驱动处理的交付（ConnectionRequest / CRA / NIC 等）；
    /// 已建立的会话返回空，交付直接进入用户队列。
    pub async fn ingest(&self, datagram: Bytes) -> Vec<Bytes> {
        if self.is_closed() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut events = Vec::new();
        if let Err(error) = lock_engine(&self.engine).ingest(
            datagram,
            Instant::now(),
            wall_ms(),
            &mut out,
            &mut events,
        ) {
            tracing::debug!(addr = %self.addr, "丢弃非法在线数据报：{error}");
        }
        self.send_all(&out).await;

        let handshaking = self.handshaking.load(Ordering::Acquire);
        let mut hs = Vec::new();
        for event in events {
            match event {
                SessionEvent::Deliver(p) => {
                    if handshaking {
                        hs.push(p);
                    } else {
                        self.push_incoming(p);
                    }
                }
                SessionEvent::PeerDisconnected | SessionEvent::Dead => self.mark_closed(),
            }
        }
        hs
    }

    /// 入队并立即冲刷一条消息。
    pub async fn send_payload(
        &self,
        payload: Bytes,
        reliability: RakReliability,
        priority: RakPriority,
    ) -> Result<(), RakSessionError> {
        if self.is_closed() {
            return Err(RakSessionError::Closed);
        }
        let mut out = Vec::new();
        {
            let mut engine = lock_engine(&self.engine);
            engine.send(payload, reliability, priority)?;
            engine.pump(Instant::now(), &mut out);
        }
        self.send_all(&out).await;
        Ok(())
    }

    /// 发送 Disconnect 并关闭会话。
    pub async fn close(&self) -> Result<(), RakSessionError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(RakSessionError::Closed);
        }
        let mut out = Vec::new();
        let result = lock_engine(&self.engine).disconnect(Instant::now(), &mut out);
        self.send_all(&out).await;
        self.mark_closed();
        result
    }

    pub async fn send_all(&self, dgrams: &[Bytes]) {
        for d in dgrams {
            // 快路径：UDP 发送极少阻塞，try_send_to 省掉 async 调度开销。
            match self.socket.try_send_to(d, self.addr) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Err(error) = self.socket.send_to(d, self.addr).await {
                        tracing::debug!(addr = %self.addr, "发送数据报失败：{error}");
                        break;
                    }
                }
                Err(error) => {
                    tracing::debug!(addr = %self.addr, "发送数据报失败：{error}");
                    break;
                }
            }
        }
    }

    pub fn push_incoming(&self, payload: Bytes) {
        let tx = self
            .incoming_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(payload);
        }
    }

    /// 握手完成：后续交付直接进入用户队列。
    pub fn set_connected(&self) {
        self.handshaking.store(false, Ordering::Release);
    }

    pub fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<Bytes>> {
        self.incoming_rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// 标记关闭：唤醒 recv（发送端置空）、通知驱动清理、停止 ticker。
    pub fn mark_closed(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.incoming_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let _ = self.dead_tx.send(self.addr);
    }

    pub fn rtt(&self) -> Duration {
        lock_engine(&self.engine).rtt()
    }
}

async fn close_shared(shared: &SessionShared) -> Result<(), RakSessionError> {
    if shared.is_closed() {
        return Ok(());
    }
    match shared.close().await {
        Ok(()) | Err(RakSessionError::Closed) => Ok(()),
        Err(error) => Err(error),
    }
}

fn close_shared_on_drop(shared: Arc<SessionShared>) {
    if shared.is_closed() {
        return;
    }
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let _ = close_shared(&shared).await;
        });
    } else {
        shared.mark_closed();
    }
}

/// 可克隆的发送侧句柄。
///
/// 只持有会话共享状态，不拥有入站 `Receiver`。因此它可以在另一个任务中并发发送，
/// 不会与 [`RakReceiver::recv_bytes`] 争夺异步互斥锁。
#[derive(Clone)]
pub struct RakSendHandle {
    shared: Arc<SessionShared>,
}

impl std::fmt::Debug for RakSendHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RakSendHandle")
            .field("addr", &self.shared.addr)
            .field("closed", &self.shared.is_closed())
            .finish()
    }
}

impl RakSendHandle {
    /// 发送一条消息。
    pub async fn send<T>(
        &self,
        buf: T,
        reliability: RakReliability,
        priority: RakPriority,
    ) -> Result<(), RakSessionError>
    where
        T: Into<Box<[u8]>>,
    {
        let boxed: Box<[u8]> = buf.into();
        self.send_bytes(Bytes::from(boxed.into_vec()), reliability, priority)
            .await
    }

    /// 零拷贝发送。
    pub async fn send_bytes(
        &self,
        payload: Bytes,
        reliability: RakReliability,
        priority: RakPriority,
    ) -> Result<(), RakSessionError> {
        self.shared
            .send_payload(payload, reliability, priority)
            .await
    }

    /// 通知对端断开并关闭会话。重复关闭按幂等成功处理。
    pub async fn close(&self) -> Result<(), RakSessionError> {
        close_shared(&self.shared).await
    }

    pub async fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }

    pub fn get_addr(&self) -> SocketAddr {
        self.shared.addr
    }

    /// 当前平滑 RTT 估计。
    pub fn rtt(&self) -> Duration {
        self.shared.rtt()
    }
}

/// 独占的接收侧句柄。
///
/// 一个会话只能有一个接收者；发送侧可通过 [`RakSendHandle`] 任意克隆并发使用。
pub struct RakReceiver {
    shared: Arc<SessionShared>,
    incoming: mpsc::UnboundedReceiver<Bytes>,
}

impl std::fmt::Debug for RakReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RakReceiver")
            .field("addr", &self.shared.addr)
            .field("closed", &self.shared.is_closed())
            .finish()
    }
}

impl RakReceiver {
    /// 接收一条完整消息。cancel-safe：可放心用于 `tokio::select!`。
    pub async fn recv<T>(&mut self) -> Result<T, RakSessionError>
    where
        Box<[u8]>: Into<T>,
    {
        match self.incoming.recv().await {
            Some(bytes) => Ok(Box::<[u8]>::from(&bytes[..]).into()),
            None => Err(RakSessionError::Closed),
        }
    }

    /// 零拷贝接收：直接返回入站数据报的切片视图。
    pub async fn recv_bytes(&mut self) -> Result<Bytes, RakSessionError> {
        self.incoming.recv().await.ok_or(RakSessionError::Closed)
    }

    pub async fn close(&self) -> Result<(), RakSessionError> {
        close_shared(&self.shared).await
    }

    pub async fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }

    pub fn get_addr(&self) -> SocketAddr {
        self.shared.addr
    }

    pub fn rtt(&self) -> Duration {
        self.shared.rtt()
    }
}

impl Drop for RakReceiver {
    fn drop(&mut self) {
        close_shared_on_drop(self.shared.clone());
    }
}

/// 已建立的 RakNet 会话句柄。
///
/// [`RakSession::recv`] 需要 `&mut self`（独占接收端），
/// [`RakSession::send`] / [`RakSession::close`] 只需 `&self`，可并发调用。
/// 如果上层需要把接收循环与发送任务彻底解耦，可使用 [`RakSession::into_split`]。
pub struct RakSession {
    shared: Arc<SessionShared>,
    incoming: Option<mpsc::UnboundedReceiver<Bytes>>,
}

impl std::fmt::Debug for RakSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RakSession")
            .field("addr", &self.shared.addr)
            .field("closed", &self.shared.is_closed())
            .finish()
    }
}

impl RakSession {
    pub(crate) fn from_shared(
        shared: Arc<SessionShared>,
        incoming: mpsc::UnboundedReceiver<Bytes>,
    ) -> Self {
        Self {
            shared,
            incoming: Some(incoming),
        }
    }

    fn incoming_mut(&mut self) -> Result<&mut mpsc::UnboundedReceiver<Bytes>, RakSessionError> {
        self.incoming.as_mut().ok_or(RakSessionError::Closed)
    }

    /// 创建一个可克隆的发送侧句柄，原会话仍保留接收所有权。
    pub fn send_handle(&self) -> RakSendHandle {
        RakSendHandle {
            shared: self.shared.clone(),
        }
    }

    /// 把会话拆成可克隆发送端与独占接收端。
    ///
    /// 拆分后不再保留 `RakSession` 本体，因此没有额外代理任务或 channel 转发。
    pub fn into_split(mut self) -> (RakSendHandle, RakReceiver) {
        let incoming = self
            .incoming
            .take()
            .expect("RakSession receiver must exist before into_split");
        let sender = self.send_handle();
        let receiver = RakReceiver {
            shared: self.shared.clone(),
            incoming,
        };
        (sender, receiver)
    }

    /// 接收一条完整消息。cancel-safe：可放心用于 `tokio::select!`。
    pub async fn recv<T>(&mut self) -> Result<T, RakSessionError>
    where
        Box<[u8]>: Into<T>,
    {
        match self.incoming_mut()?.recv().await {
            Some(bytes) => Ok(Box::<[u8]>::from(&bytes[..]).into()),
            None => Err(RakSessionError::Closed),
        }
    }

    /// 零拷贝接收：直接返回入站数据报的切片视图。
    pub async fn recv_bytes(&mut self) -> Result<Bytes, RakSessionError> {
        self.incoming_mut()?
            .recv()
            .await
            .ok_or(RakSessionError::Closed)
    }

    /// 发送一条消息。
    pub async fn send<T>(
        &self,
        buf: T,
        reliability: RakReliability,
        priority: RakPriority,
    ) -> Result<(), RakSessionError>
    where
        T: Into<Box<[u8]>>,
    {
        let boxed: Box<[u8]> = buf.into();
        self.send_bytes(Bytes::from(boxed.into_vec()), reliability, priority)
            .await
    }

    /// 零拷贝发送。
    pub async fn send_bytes(
        &self,
        payload: Bytes,
        reliability: RakReliability,
        priority: RakPriority,
    ) -> Result<(), RakSessionError> {
        self.shared
            .send_payload(payload, reliability, priority)
            .await
    }

    /// 通知对端断开并关闭会话。重复关闭返回 `Err(Closed)`，保持旧 API 语义。
    pub async fn close(&self) -> Result<(), RakSessionError> {
        self.shared.close().await
    }

    pub async fn is_closed(&self) -> bool {
        self.shared.is_closed()
    }

    pub fn get_addr(&self) -> SocketAddr {
        self.shared.addr
    }

    /// 当前平滑 RTT 估计。
    pub fn rtt(&self) -> Duration {
        self.shared.rtt()
    }
}

impl Drop for RakSession {
    fn drop(&mut self) {
        // `into_split()` 会 take 掉接收端；这时关闭职责已经移交给 RakReceiver。
        if self.incoming.is_none() {
            return;
        }
        close_shared_on_drop(self.shared.clone());
    }
}
