//! 客户端：向已发现的对端拨号。

use crate::consts::NEGOTIATION_TIMEOUT;
use crate::error::{NethernetError, Result, SignalErrorCode};
use crate::protocol::{Signal, SignalType};
use crate::session::NethernetSession;
use crate::signaling::LanSignaling;
use crate::transport::negotiate::{
    create_data_channels, create_peer_connection, finish_local_description, parse_candidate,
};
use bytes::Bytes;
use rand::RngExt as _;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

/// 协商期间允许缓存的 ICE 候选数。
const MAX_PENDING_CANDIDATES: usize = 64;

/// 一条已建立的客户端会话。
#[derive(Clone)]
pub struct NethernetStream {
    session: Arc<NethernetSession>,
    remote_addr: SocketAddr,
    /// 持有信令端点，保证其接收任务在会话存活期间不被 drop——
    /// 否则连接建立后到达的 ICE 候选会无人接收。
    _signaling: Arc<LanSignaling>,
}

impl NethernetStream {
    /// 与已发现的对端协商建立会话。
    ///
    /// # Errors
    ///
    /// 信令、ICE 协商或数据通道建立失败时返回错误；对端主动拒绝时
    /// 返回 [`NethernetError::Refused`] 并携带错误码。
    pub async fn connect(
        signaling: Arc<LanSignaling>,
        remote_network_id: u64,
        remote_addr: SocketAddr,
    ) -> Result<Self> {
        let connection_id = rand::rng().random::<u64>();
        tracing::info!(
            connection_id,
            remote_network_id,
            %remote_addr,
            local_network_id = signaling.network_id(),
            "开始连接 NetherNet 对端"
        );
        let peer_connection = create_peer_connection().await?;
        let session = NethernetSession::new(Arc::clone(&peer_connection));
        let result = Self::negotiate(
            &peer_connection,
            &session,
            &signaling,
            remote_network_id,
            remote_addr,
            connection_id,
        )
        .await;
        if let Err(error) = &result {
            // 拆除半开的对等连接：webrtc-rs 无 Drop 实现，仅丢弃句柄
            // 会永久泄漏 ICE agent、UDP 套接字与候选接收任务。
            if let Err(close_error) = session.close().await {
                tracing::debug!(
                    connection_id,
                    "清理失败的 NetherNet 出站会话失败：{close_error}"
                );
            }
            // 尽力把失败原因告知对端，让它及时释放半开状态。
            let code = match error {
                NethernetError::Timeout => SignalErrorCode::NegotiationTimeout,
                NethernetError::Refused(code) => *code,
                _ => SignalErrorCode::Ice,
            };
            if let Err(signal_error) = signaling
                .signal(Signal::error(connection_id, code, remote_network_id))
                .await
            {
                tracing::warn!(
                    connection_id,
                    remote_network_id,
                    "发送 NetherNet 协商错误失败：{signal_error}"
                );
            }
            tracing::warn!(connection_id, remote_network_id, %remote_addr, "连接 NetherNet 对端失败：{error}");
        } else {
            tracing::info!(connection_id, remote_network_id, %remote_addr, "NetherNet 对端连接成功");
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn negotiate(
        peer_connection: &Arc<RTCPeerConnection>,
        session: &Arc<NethernetSession>,
        signaling: &Arc<LanSignaling>,
        remote_network_id: u64,
        remote_addr: SocketAddr,
        connection_id: u64,
    ) -> Result<Self> {
        let (reliable, unreliable) = create_data_channels(peer_connection).await?;
        session.attach_reliable(&reliable);
        session.attach_unreliable(&unreliable);

        // 先订阅再发 offer，避免应答早于订阅造成丢信令。
        let mut signals = signaling.subscribe();
        let offer = peer_connection.create_offer(None).await?;
        let offer = finish_local_description(peer_connection, offer).await?;
        signaling
            .signal(Signal::offer(connection_id, offer, remote_network_id))
            .await?;
        tracing::info!(
            connection_id,
            remote_network_id,
            %remote_addr,
            "NetherNet Offer 已发送，等待 Answer"
        );

        let (answer, pending) = tokio::time::timeout(
            NEGOTIATION_TIMEOUT,
            wait_for_answer(&mut signals, connection_id, remote_network_id),
        )
        .await
        .map_err(|_| NethernetError::Timeout)??;
        tracing::info!(
            connection_id,
            remote_network_id,
            pending_candidates = pending.len(),
            "NetherNet Answer 已收到"
        );

        peer_connection
            .set_remote_description(RTCSessionDescription::answer(answer)?)
            .await?;
        for candidate in pending {
            if let Err(error) = peer_connection.add_ice_candidate(candidate).await {
                tracing::debug!("添加缓存的 ICE 候选失败：{error}");
            }
        }
        spawn_candidate_receiver(
            Arc::clone(peer_connection),
            signals,
            connection_id,
            remote_network_id,
            session.cancellation_token(),
        );

        tokio::time::timeout(NEGOTIATION_TIMEOUT, async {
            session.wait_ready().await;
            wait_for_ice(peer_connection).await
        })
        .await
        .map_err(|_| NethernetError::Timeout)??;
        if session.is_closed() {
            return Err(NethernetError::Closed);
        }
        tracing::info!(
            connection_id,
            remote_network_id,
            %remote_addr,
            "NetherNet ICE 与数据通道已就绪"
        );

        Ok(Self {
            session: Arc::clone(session),
            remote_addr,
            _signaling: Arc::clone(signaling),
        })
    }

    /// 发送一条完整消息（可靠有序）。
    ///
    /// # Errors
    ///
    /// 会话已关闭或底层发送失败时返回错误。
    pub async fn send(&self, data: Bytes) -> Result<()> {
        self.session.send(data).await
    }

    /// 接收下一条完整消息；会话关闭后返回 `Ok(None)`。
    ///
    /// # Errors
    ///
    /// 当前实现不产生错误，返回类型为将来扩展保留。
    pub async fn recv(&self) -> Result<Option<Bytes>> {
        self.session.recv().await
    }

    /// 关闭会话。
    ///
    /// # Errors
    ///
    /// WebRTC 关闭失败时返回错误。
    pub async fn close(&self) -> Result<()> {
        self.session.close().await
    }

    #[must_use]
    pub const fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    #[must_use]
    pub fn session(&self) -> Arc<NethernetSession> {
        Arc::clone(&self.session)
    }
}

/// 等待 answer，同时缓存先到的候选。
async fn wait_for_answer(
    signals: &mut broadcast::Receiver<Signal>,
    connection_id: u64,
    remote_network_id: u64,
) -> Result<(
    String,
    Vec<webrtc::ice_transport::ice_candidate::RTCIceCandidateInit>,
)> {
    let mut pending = Vec::new();
    loop {
        let signal = recv_signal(signals).await?;
        if signal.connection_id != connection_id || signal.network_id != remote_network_id {
            continue;
        }
        match signal.kind {
            SignalType::Answer => return Ok((signal.data, pending)),
            SignalType::Candidate if pending.len() < MAX_PENDING_CANDIDATES => {
                pending.push(parse_candidate(&signal.data));
            }
            SignalType::Candidate => {
                tracing::debug!("缓存的 ICE 候选已达上限，丢弃");
            }
            SignalType::Error => {
                return Err(NethernetError::Refused(
                    signal
                        .error_code()
                        .unwrap_or(SignalErrorCode::SignalingUnknownError),
                ));
            }
            SignalType::Offer => {}
        }
    }
}

pub(crate) fn spawn_candidate_receiver(
    peer_connection: Arc<RTCPeerConnection>,
    mut signals: broadcast::Receiver<Signal>,
    connection_id: u64,
    remote_network_id: u64,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let signal = tokio::select! {
                () = cancel.cancelled() => break,
                signal = recv_signal(&mut signals) => signal,
            };
            let Ok(signal) = signal else { break };
            if signal.connection_id != connection_id
                || signal.network_id != remote_network_id
                || signal.kind != SignalType::Candidate
            {
                continue;
            }
            if let Err(error) = peer_connection
                .add_ice_candidate(parse_candidate(&signal.data))
                .await
            {
                tracing::debug!("添加 ICE 候选失败：{error}");
            }
        }
    });
}

/// 从广播通道读取下一条信令，跳过因落后而丢失的部分。
pub(crate) async fn recv_signal(receiver: &mut broadcast::Receiver<Signal>) -> Result<Signal> {
    loop {
        match receiver.recv().await {
            Ok(signal) => return Ok(signal),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!("信令接收落后，跳过 {skipped} 条");
            }
            Err(broadcast::error::RecvError::Closed) => return Err(NethernetError::Closed),
        }
    }
}

/// 轮询 ICE 状态直至连接建立或判定失败。
pub(crate) async fn wait_for_ice(peer_connection: &RTCPeerConnection) -> Result<()> {
    loop {
        match peer_connection.ice_connection_state() {
            RTCIceConnectionState::Connected | RTCIceConnectionState::Completed => return Ok(()),
            RTCIceConnectionState::Failed
            | RTCIceConnectionState::Disconnected
            | RTCIceConnectionState::Closed => return Err(NethernetError::Closed),
            _ => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
}
