//! 会话：两条 WebRTC 数据通道之上的消息收发。
//!
//! # 为什么要接管数据通道的读循环
//!
//! webrtc-rs 内置的 `RTCDataChannel` 读循环使用固定 65535 字节缓冲
//! （`webrtc-0.17.2` `data_channel/mod.rs:33`），而 `NetherNet` 的分片
//! 上限是 262143 字节。走内置路径时，任何超过 64 KiB 的入站消息都会
//! 读失败并关闭通道，且该缓冲**无法通过配置修改**。因此这里启用
//! detach，由本模块按分片上限自管读循环。
//!
//! # 并发与生命周期
//!
//! - 每条通道一个读任务与一条独立队列。可靠通道承载有序游戏报文流
//!   （上层带加密计数器），**丢一条即导致对端解密错位断线**，因此队列
//!   积压时快速失败关闭会话；不可靠通道本就允许丢，积压即丢弃并计数。
//! - 读任务只持 `Weak`，不阻止会话回收；会话 `Drop` 时统一拆除底层
//!   WebRTC 栈——webrtc-rs 没有任何 `Drop` 实现，不显式 close 会永久
//!   泄漏 ICE agent、绑定的 UDP 套接字与后台任务。
//! - 「逻辑关闭」与「资源拆除」用两个独立标志：对端先断开时逻辑关闭
//!   已置位，若复用同一标志会让随后的 `close()` 短路，底层栈永不释放。
//! - `recv` 的关闭信号是电平触发的，可安全用于 `tokio::select!`。

use crate::consts::MAX_SEGMENT_PAYLOAD;
use crate::error::{NethernetError, Result};
use crate::protocol::message::{self, Reassembler};
use crate::transport::ortc::OrtcStack;
use bytes::Bytes;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use tokio::sync::{Mutex, OnceCell, mpsc};
use tokio_util::sync::CancellationToken;
use webrtc::data::data_channel::DataChannel as DetachedDataChannel;
use webrtc::data_channel::RTCDataChannel;
use webrtc::dtls_transport::dtls_transport_state::RTCDtlsTransportState;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_transport_state::RTCIceTransportState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

/// 可靠通道的入站队列容量（条）。
const RELIABLE_QUEUE_CAPACITY: usize = 1024;
/// 不可靠通道的入站队列容量（条）。
const UNRELIABLE_QUEUE_CAPACITY: usize = 256;
/// `NetherNet` 约定的通道数。
const CHANNEL_COUNT: usize = 2;

/// 会话统计。
#[derive(Debug, Default)]
pub struct SessionStats {
    /// 已交付给上层的消息数。
    pub packets_received: AtomicU64,
    /// 已发送的消息数。
    pub packets_sent: AtomicU64,
    /// 因不可靠通道队列积压被丢弃的消息数。
    pub packets_dropped: AtomicU64,
    /// 因格式非法被丢弃的入站消息数。
    pub packets_invalid: AtomicU64,
}

/// 一条已协商完成的 `NetherNet` 会话。
pub struct NethernetSession {
    backend: SessionBackend,
    reliable: OnceCell<Arc<DetachedDataChannel>>,
    unreliable: OnceCell<Arc<DetachedDataChannel>>,
    /// 保证多分片消息在通道上不被交错。
    reliable_write: Mutex<()>,
    unreliable_write: Mutex<()>,
    /// 两条通道各自独立的队列：混流会让不可靠通道的突发挤掉可靠消息，
    /// 也会让 `recv` 返回的顺序不再对应任一通道的实际顺序。
    reliable_rx: Mutex<mpsc::Receiver<Bytes>>,
    reliable_tx: mpsc::Sender<Bytes>,
    unreliable_rx: Mutex<mpsc::Receiver<Bytes>>,
    unreliable_tx: mpsc::Sender<Bytes>,
    /// 逻辑关闭：`recv` 返回 `None`、`send` 报错。
    closed: AtomicBool,
    /// 资源已拆除：与 `closed` 分开，避免对端先断开导致 `close()` 短路。
    torn_down: AtomicBool,
    /// 电平触发的关闭信号，`select!` 中反复取消重建也不会漏。
    close_signal: CancellationToken,
    /// 已就绪（open 并接管读循环）的通道数。
    open_channels: AtomicUsize,
    ready_signal: CancellationToken,
    stats: SessionStats,
}

#[derive(Clone)]
enum SessionBackend {
    PeerConnection(Arc<RTCPeerConnection>),
    Ortc(Arc<OrtcStack>),
}

impl SessionBackend {
    async fn close(&self) -> Result<()> {
        match self {
            Self::PeerConnection(peer_connection) => {
                peer_connection.close().await?;
                Ok(())
            }
            Self::Ortc(stack) => stack.close().await,
        }
    }
}

impl NethernetSession {
    pub(crate) fn new(peer_connection: Arc<RTCPeerConnection>) -> Arc<Self> {
        Self::new_with_backend(SessionBackend::PeerConnection(peer_connection))
    }

    pub(crate) fn new_ortc(stack: Arc<OrtcStack>) -> Arc<Self> {
        Self::new_with_backend(SessionBackend::Ortc(stack))
    }

    fn new_with_backend(backend: SessionBackend) -> Arc<Self> {
        let (reliable_tx, reliable_rx) = mpsc::channel(RELIABLE_QUEUE_CAPACITY);
        let (unreliable_tx, unreliable_rx) = mpsc::channel(UNRELIABLE_QUEUE_CAPACITY);
        let session = Arc::new(Self {
            backend,
            reliable: OnceCell::new(),
            unreliable: OnceCell::new(),
            reliable_write: Mutex::new(()),
            unreliable_write: Mutex::new(()),
            reliable_rx: Mutex::new(reliable_rx),
            reliable_tx,
            unreliable_rx: Mutex::new(unreliable_rx),
            unreliable_tx,
            closed: AtomicBool::new(false),
            torn_down: AtomicBool::new(false),
            close_signal: CancellationToken::new(),
            open_channels: AtomicUsize::new(0),
            ready_signal: CancellationToken::new(),
            stats: SessionStats::default(),
        });
        session.watch_connection_state();
        session
    }

    /// 监听 ICE、DTLS 与对等连接状态：链路断开必须终结会话，否则数据通道没有 EOF，
    /// 上层会一直挂在 `recv` 上。
    fn watch_connection_state(self: &Arc<Self>) {
        match &self.backend {
            SessionBackend::PeerConnection(peer_connection) => {
                self.watch_peer_connection(peer_connection);
            }
            SessionBackend::Ortc(stack) => self.watch_ortc(stack),
        }
    }

    fn watch_peer_connection(self: &Arc<Self>, peer_connection: &RTCPeerConnection) {
        let session = Arc::downgrade(self);
        peer_connection.on_ice_connection_state_change(Box::new(move |state| {
            let session = session.clone();
            Box::pin(async move {
                tracing::info!(?state, "NetherNet ICE 状态变化");
                if matches!(
                    state,
                    RTCIceConnectionState::Failed
                        | RTCIceConnectionState::Disconnected
                        | RTCIceConnectionState::Closed
                ) && let Some(session) = session.upgrade()
                {
                    tracing::warn!(?state, "NetherNet ICE 链路终结，关闭会话");
                    session.mark_closed();
                }
            })
        }));

        let session = Arc::downgrade(self);
        peer_connection.on_peer_connection_state_change(Box::new(move |state| {
            let session = session.clone();
            Box::pin(async move {
                tracing::info!(?state, "NetherNet 对等连接状态变化");
                if matches!(
                    state,
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
                ) && let Some(session) = session.upgrade()
                {
                    session.mark_closed();
                }
            })
        }));

        let session = Arc::downgrade(self);
        peer_connection
            .dtls_transport()
            .on_state_change(Box::new(move |state| {
                let session = session.clone();
                Box::pin(async move {
                    tracing::info!(?state, "NetherNet DTLS 状态变化");
                    if matches!(
                        state,
                        RTCDtlsTransportState::Failed | RTCDtlsTransportState::Closed
                    ) && let Some(session) = session.upgrade()
                    {
                        session.mark_closed();
                    }
                })
            }));
    }

    fn watch_ortc(self: &Arc<Self>, stack: &OrtcStack) {
        let session = Arc::downgrade(self);
        stack.ice.on_connection_state_change(Box::new(move |state| {
            let session = session.clone();
            Box::pin(async move {
                tracing::info!(?state, "NetherNet ORTC ICE 状态变化");
                if matches!(
                    state,
                    RTCIceTransportState::Failed
                        | RTCIceTransportState::Disconnected
                        | RTCIceTransportState::Closed
                ) && let Some(session) = session.upgrade()
                {
                    tracing::warn!(?state, "NetherNet ORTC ICE 链路终结，关闭会话");
                    session.mark_closed();
                }
            })
        }));

        let session = Arc::downgrade(self);
        stack.dtls.on_state_change(Box::new(move |state| {
            let session = session.clone();
            Box::pin(async move {
                tracing::info!(?state, "NetherNet ORTC DTLS 状态变化");
                if matches!(
                    state,
                    RTCDtlsTransportState::Failed | RTCDtlsTransportState::Closed
                ) && let Some(session) = session.upgrade()
                {
                    session.mark_closed();
                }
            })
        }));
    }

    /// 挂接可靠通道（有序，支持多分片）。
    pub(crate) fn attach_reliable(self: &Arc<Self>, channel: &Arc<RTCDataChannel>) {
        self.attach(channel, true);
    }

    /// 挂接不可靠通道（无序、不重传，仅接受单片消息）。
    pub(crate) fn attach_unreliable(self: &Arc<Self>, channel: &Arc<RTCDataChannel>) {
        self.attach(channel, false);
    }

    /// 注册 open 回调：通道打开后 detach 并启动读循环。
    ///
    /// `on_open` 是单槽回调（webrtc-rs 内部 `take()` 后触发一次），
    /// 因此就绪通知也必须收敛在这里，不能由调用方另行注册。
    fn attach(self: &Arc<Self>, channel: &Arc<RTCDataChannel>, reliable: bool) {
        // 用 Weak 避免 会话 → 通道 → 回调 → 会话 的引用环。
        let session = Arc::downgrade(self);
        let channel_for_open = Arc::clone(channel);
        let label = channel.label().to_string();
        channel.on_open(Box::new(move || {
            let session = session.clone();
            let channel = Arc::clone(&channel_for_open);
            let label = label.clone();
            Box::pin(async move {
                let Some(session) = session.upgrade() else {
                    return;
                };
                match channel.detach().await {
                    Ok(detached) => session.start_reader(detached, reliable, label),
                    Err(error) => {
                        tracing::error!(%label, "detach 数据通道失败：{error}");
                        session.mark_closed();
                    }
                }
            })
        }));
    }

    fn start_reader(
        self: &Arc<Self>,
        channel: Arc<DetachedDataChannel>,
        reliable: bool,
        label: String,
    ) {
        let slot = if reliable {
            &self.reliable
        } else {
            &self.unreliable
        };
        if slot.set(Arc::clone(&channel)).is_err() {
            tracing::debug!(%label, "忽略重复的数据通道");
            return;
        }
        let open_channels = self.open_channels.fetch_add(1, Ordering::AcqRel) + 1;
        tracing::info!(%label, reliable, open_channels, "NetherNet 数据通道已打开");
        if open_channels >= CHANNEL_COUNT {
            self.ready_signal.cancel();
        }
        // 读任务只持 Weak：持强引用会让会话在用户丢弃最后一个 Arc 后
        // 仍被自己的读任务保活，从而永不 Drop、永不拆除底层栈。
        tokio::spawn(read_loop(
            Arc::downgrade(self),
            channel,
            reliable,
            label,
            self.close_signal.clone(),
        ));
    }

    /// 等待两条通道都就绪（或会话中途关闭）。
    pub(crate) async fn wait_ready(&self) {
        tokio::select! {
            () = self.ready_signal.cancelled() => {}
            () = self.close_signal.cancelled() => {}
        }
    }

    /// 会话终结信号，供协商期的候选接收任务等挂靠。
    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.close_signal.clone()
    }

    pub(crate) fn open_channel_count(&self) -> usize {
        self.open_channels.load(Ordering::Acquire)
    }

    /// 会话统计。
    #[must_use]
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }

    /// 通过可靠有序通道发送一条完整消息。
    ///
    /// # Errors
    ///
    /// 会话已关闭、消息为空或超过协议上限、底层通道拒绝时返回错误。
    pub async fn send(&self, data: Bytes) -> Result<()> {
        self.send_on(&data, true).await
    }

    /// 通过不可靠通道发送。消息必须能放进单个分片。
    ///
    /// # Errors
    ///
    /// 会话已关闭、消息超过单片上限、底层通道拒绝时返回错误。
    pub async fn send_unreliable(&self, data: Bytes) -> Result<()> {
        if data.len() > MAX_SEGMENT_PAYLOAD {
            return Err(NethernetError::TooLarge {
                size: data.len(),
                max: MAX_SEGMENT_PAYLOAD,
            });
        }
        self.send_on(&data, false).await
    }

    async fn send_on(&self, data: &Bytes, reliable: bool) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(NethernetError::Closed);
        }
        let segments = message::split(data)?;
        let channel = if reliable {
            self.reliable.get()
        } else {
            self.unreliable.get()
        }
        .ok_or(NethernetError::Closed)?;

        let _guard = if reliable {
            self.reliable_write.lock().await
        } else {
            self.unreliable_write.lock().await
        };
        for segment in segments {
            channel
                .write(&segment)
                .await
                .map_err(|error| NethernetError::protocol(format!("写入数据通道失败：{error}")))?;
        }
        self.stats.packets_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// 接收可靠通道上的下一条完整消息；会话关闭且缓冲取空后返回 `Ok(None)`。
    ///
    /// cancel-safe：关闭信号是电平触发的，在 `tokio::select!` 中被反复
    /// 取消重建也不会漏掉通知。
    ///
    /// # Errors
    ///
    /// 当前实现不产生错误，返回类型为将来扩展保留。
    pub async fn recv(&self) -> Result<Option<Bytes>> {
        self.recv_from(&self.reliable_rx).await
    }

    /// 接收不可靠通道上的下一条消息。
    ///
    /// # Errors
    ///
    /// 当前实现不产生错误，返回类型为将来扩展保留。
    pub async fn recv_unreliable(&self) -> Result<Option<Bytes>> {
        self.recv_from(&self.unreliable_rx).await
    }

    async fn recv_from(&self, queue: &Mutex<mpsc::Receiver<Bytes>>) -> Result<Option<Bytes>> {
        let mut rx = queue.lock().await;
        // 已关闭也要先把缓冲里的消息交付完。
        if let Ok(packet) = rx.try_recv() {
            return Ok(Some(packet));
        }
        if self.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        tokio::select! {
            packet = rx.recv() => Ok(packet),
            () = self.close_signal.cancelled() => Ok(rx.try_recv().ok()),
        }
    }

    /// 关闭会话并拆除底层 WebRTC 栈。多次调用是幂等的。
    ///
    /// 即使对端已先断开（此时会话早已逻辑关闭），本方法仍会真正执行拆除。
    ///
    /// # Errors
    ///
    /// WebRTC 关闭失败时返回错误。
    pub async fn close(&self) -> Result<()> {
        self.mark_closed();
        if self.torn_down.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        for channel in [self.reliable.get(), self.unreliable.get()]
            .into_iter()
            .flatten()
        {
            if let Err(error) = channel.close().await {
                tracing::debug!("关闭数据通道失败：{error}");
            }
        }
        self.backend.close().await
    }

    /// 会话是否已关闭。
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// 仅做逻辑关闭：唤醒 `recv`、停掉读循环与协商期任务。
    fn mark_closed(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.close_signal.cancel();
        }
    }
}

impl Drop for NethernetSession {
    fn drop(&mut self) {
        self.close_signal.cancel();
        if self.torn_down.swap(true, Ordering::AcqRel) {
            return;
        }
        // webrtc-rs 全无 Drop 实现：不显式 close 会永久泄漏 ICE agent、
        // 绑定的 UDP 套接字与若干后台任务。这里补上拆除。
        let backend = self.backend.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(error) = backend.close().await {
                    tracing::debug!("拆除 NetherNet 传输失败：{error}");
                }
            });
        }
    }
}

/// 自管读循环：按分片上限分配缓冲，逐条读出并重组。
async fn read_loop(
    session: Weak<NethernetSession>,
    channel: Arc<DetachedDataChannel>,
    reliable: bool,
    label: String,
    cancel: CancellationToken,
) {
    // +1 为分片计数字节；再多留 1 字节以便识别超限消息而非静默截断。
    let mut buffer = vec![0_u8; MAX_SEGMENT_PAYLOAD + 2];
    let mut reassembler = Reassembler::new();
    loop {
        let read = tokio::select! {
            () = cancel.cancelled() => break,
            read = channel.read(&mut buffer) => read,
        };
        let length = match read {
            Ok(0) => {
                tracing::info!(%label, "NetherNet 数据通道由对端关闭");
                break;
            }
            Ok(length) => length,
            Err(error) => {
                tracing::info!(%label, "NetherNet 数据通道读取结束：{error}");
                break;
            }
        };
        // 会话已被丢弃：无人接收，直接收尾。
        let Some(session) = session.upgrade() else {
            break;
        };

        if length > MAX_SEGMENT_PAYLOAD + 1 {
            session
                .stats
                .packets_invalid
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!(%label, length, "丢弃超过分片上限的入站消息");
            reassembler.reset();
            continue;
        }
        // 不可靠通道禁止分片：任一分片丢失都无法重组。
        if !reliable && buffer[0] != 0 {
            session
                .stats
                .packets_invalid
                .fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%label, "丢弃不可靠通道上的分片消息");
            continue;
        }
        match reassembler.push(Bytes::copy_from_slice(&buffer[..length])) {
            Ok(Some(packet)) => {
                if !deliver(&session, packet, reliable) {
                    break;
                }
            }
            Ok(None) => {}
            Err(error) => {
                session
                    .stats
                    .packets_invalid
                    .fetch_add(1, Ordering::Relaxed);
                tracing::debug!(%label, "丢弃非法数据通道消息：{error}");
                if reliable {
                    // 可靠流一旦出现无法重组的消息，后续字节的边界也不可信。
                    tracing::warn!(%label, "可靠通道消息损坏，关闭会话");
                    session.mark_closed();
                    break;
                }
            }
        }
    }
    if reliable {
        // 可靠通道断开即会话终结；不可靠通道断开不影响主链路。
        if let Some(session) = session.upgrade() {
            session.mark_closed();
        }
    }
}

/// 投递一条消息。返回 `false` 表示读循环应终止。
///
/// 可靠通道承载有序游戏报文流，丢一条就会让对端的解密计数器错位并断线，
/// 因此积压时选择**快速失败**而不是静默丢弃；不可靠通道本就允许丢。
fn deliver(session: &Arc<NethernetSession>, packet: Bytes, reliable: bool) -> bool {
    let tx = if reliable {
        &session.reliable_tx
    } else {
        &session.unreliable_tx
    };
    match tx.try_send(packet) {
        Ok(()) => {
            session
                .stats
                .packets_received
                .fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            if reliable {
                tracing::error!(
                    capacity = RELIABLE_QUEUE_CAPACITY,
                    "可靠通道积压超限，消费方读取过慢，关闭会话"
                );
                session.mark_closed();
                false
            } else {
                session
                    .stats
                    .packets_dropped
                    .fetch_add(1, Ordering::Relaxed);
                true
            }
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            session
                .stats
                .packets_dropped
                .fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NethernetSession>();
    }
}
