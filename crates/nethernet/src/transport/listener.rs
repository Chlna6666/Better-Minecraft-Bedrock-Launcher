//! 服务端：接受入站协商请求。

use crate::consts::{MAX_SIGNAL_SIZE, NEGOTIATION_TIMEOUT};
use crate::error::{NethernetError, Result, SignalErrorCode};
use crate::identity::ServerIdentity;
use crate::protocol::{Signal, SignalType};
use crate::session::NethernetSession;
use crate::signaling::LanSignaling;
use crate::transport::candidate::{
    IceExchangeMode, NegotiationConfig, RemoteCandidateReceiver, gather_local_candidates,
    install_ortc_candidate_queue, spawn_local_candidate_sender, spawn_remote_candidate_receiver,
};
use crate::transport::diagnostics::{
    log_candidate_summary, log_negotiation_timeout, log_offer_summary,
};
use crate::transport::negotiate::summarize_sdp;
use crate::transport::ortc::{OrtcStack, build_answer, parse_offer};
use crate::transport::stream::recv_signal;
use crate::{RELIABLE_CHANNEL, UNRELIABLE_CHANNEL};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// 待 accept 的会话队列容量。
const INCOMING_CAPACITY: usize = 32;
/// 单个协商的信令队列容量。
const SIGNAL_CAPACITY: usize = 32;
/// 同时进行的协商数上限。
const MAX_CONCURRENT_NEGOTIATIONS: usize = 32;
/// offer 到达前允许为某个连接编号缓存的候选数。
const MAX_BUFFERED_SIGNALS: usize = 16;
/// 监听入站 `NetherNet` 会话。
pub struct NethernetListener {
    incoming: mpsc::Receiver<Arc<NethernetSession>>,
    local_addr: SocketAddr,
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

impl NethernetListener {
    /// 在已绑定的局域网信令端点上开始监听。
    ///
    /// # Errors
    ///
    /// 当前实现在信令绑定之后没有会失败的初始化步骤。
    pub fn bind(signaling: LanSignaling, local_addr: SocketAddr) -> Result<Self> {
        Self::bind_with_config(signaling, local_addr, NegotiationConfig::default())
    }

    /// 使用指定 ICE 交换方式开始监听。
    ///
    /// # Errors
    ///
    /// 身份初始化失败时返回错误。
    pub fn bind_with_config(
        signaling: LanSignaling,
        local_addr: SocketAddr,
        config: NegotiationConfig,
    ) -> Result<Self> {
        tracing::info!(
            signaling_addr = ?signaling.local_addr().ok(),
            network_id = signaling.network_id(),
            %local_addr,
            ice_exchange = config.ice_exchange.as_str(),
            candidate_encoding = config.candidate_encoding.as_str(),
            "NetherNet 会话监听器已启动"
        );
        let signaling = Arc::new(signaling);
        let identity = Arc::new(ServerIdentity::generate()?);
        // 必须在返回 Listener 前完成订阅。若把 subscribe 放进 spawn 的
        // dispatch_loop，Minecraft 可能在发现响应后立刻发送 Offer，而后台
        // 任务尚未获得调度，broadcast 会因零订阅者直接丢弃该 Offer。
        let signals = signaling.subscribe();
        let (incoming_tx, incoming) = mpsc::channel(INCOMING_CAPACITY);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(dispatch_loop(
            Arc::clone(&signaling),
            identity,
            signals,
            incoming_tx,
            cancel.clone(),
            config,
        ));
        Ok(Self {
            incoming,
            local_addr,
            cancel,
            task,
        })
    }

    /// 等待下一条协商完成的会话。
    ///
    /// # Errors
    ///
    /// 监听器已关闭时返回错误。
    pub async fn accept(&mut self) -> Result<Arc<NethernetSession>> {
        let session = self.incoming.recv().await.ok_or(NethernetError::Closed)?;
        tracing::info!(local_addr = %self.local_addr, "NetherNet 会话已进入 accept 队列");
        Ok(session)
    }

    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for NethernetListener {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

/// 信令分发：按连接编号把信令路由到各自的协商任务。
async fn dispatch_loop(
    signaling: Arc<LanSignaling>,
    identity: Arc<ServerIdentity>,
    mut signals: broadcast::Receiver<Signal>,
    incoming_tx: mpsc::Sender<Arc<NethernetSession>>,
    cancel: CancellationToken,
    config: NegotiationConfig,
) {
    let mut routes: HashMap<u64, mpsc::Sender<Signal>> = HashMap::new();
    let mut buffered: HashMap<u64, Vec<Signal>> = HashMap::new();

    loop {
        let signal = tokio::select! {
            () = cancel.cancelled() => break,
            signal = recv_signal(&mut signals) => signal,
        };
        let Ok(signal) = signal else { break };

        routes.retain(|_, route| !route.is_closed());
        if signal.data.len() > MAX_SIGNAL_SIZE {
            tracing::debug!(length = signal.data.len(), "忽略过大的信令");
            continue;
        }

        match signal.kind {
            SignalType::Offer => {
                if routes.contains_key(&signal.connection_id) {
                    continue; // 重复 offer。
                }
                tracing::info!(
                    connection_id = signal.connection_id,
                    remote_network_id = signal.network_id,
                    "开始接受 NetherNet Offer"
                );
                if routes.len() >= MAX_CONCURRENT_NEGOTIATIONS {
                    tracing::warn!(
                        connection_id = signal.connection_id,
                        remote_network_id = signal.network_id,
                        "NetherNet 并发协商数已达上限，拒绝新 Offer"
                    );
                    if let Err(error) = signaling
                        .signal(Signal::error(
                            signal.connection_id,
                            SignalErrorCode::IncomingConnectionIgnored,
                            signal.network_id,
                        ))
                        .await
                    {
                        tracing::warn!("发送 NetherNet 拒绝信令失败：{error}");
                    }
                    continue;
                }
                let connection_id = signal.connection_id;
                let (route_tx, route_rx) = mpsc::channel(SIGNAL_CAPACITY);
                // offer 之前到达的候选先补发给协商任务。
                if let Some(pending) = buffered.remove(&connection_id) {
                    for candidate in pending {
                        if route_tx.try_send(candidate).is_err() {
                            break;
                        }
                    }
                }
                routes.insert(connection_id, route_tx);
                tokio::spawn(handle_offer(
                    signal,
                    Arc::clone(&signaling),
                    Arc::clone(&identity),
                    route_rx,
                    incoming_tx.clone(),
                    config,
                ));
            }
            _ => {
                if let Some(route) = routes.get(&signal.connection_id) {
                    let connection_id = signal.connection_id;
                    // 用 try_send 而非 send().await：某个协商任务卡住
                    // 不应阻塞整个分发循环。
                    if route.try_send(signal).is_err() {
                        tracing::debug!(connection_id, "协商信令队列已满或已关闭");
                    }
                } else if signal.kind == SignalType::Candidate
                    && buffered.len() < MAX_CONCURRENT_NEGOTIATIONS
                {
                    let slot = buffered.entry(signal.connection_id).or_default();
                    if slot.len() < MAX_BUFFERED_SIGNALS {
                        slot.push(signal);
                    }
                }
            }
        }
    }
}

async fn handle_offer(
    offer: Signal,
    signaling: Arc<LanSignaling>,
    identity: Arc<ServerIdentity>,
    signals: mpsc::Receiver<Signal>,
    incoming_tx: mpsc::Sender<Arc<NethernetSession>>,
    config: NegotiationConfig,
) {
    let connection_id = offer.connection_id;
    let remote_network_id = offer.network_id;
    if let Err(error) =
        negotiate_incoming(offer, &signaling, &identity, signals, incoming_tx, config).await
    {
        tracing::warn!(
            connection_id,
            remote_network_id,
            "接受 NetherNet 会话失败：{error}"
        );
        let code = match &error {
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
    }
}

async fn negotiate_incoming(
    offer: Signal,
    signaling: &Arc<LanSignaling>,
    identity: &ServerIdentity,
    signals: mpsc::Receiver<Signal>,
    incoming_tx: mpsc::Sender<Arc<NethernetSession>>,
    config: NegotiationConfig,
) -> Result<()> {
    let stack = OrtcStack::new().await?;
    let session = NethernetSession::new_ortc(Arc::clone(&stack));
    install_incoming_channels(&stack, &session);

    // 任一步失败都必须终结会话：否则候选接收任务不会退出，
    // 它持有的路由发送端也就不会关闭，监听器的并发协商槽永久泄漏
    // ——32 个伪造 offer 即可让监听器此后拒绝一切新连接。
    let result = negotiate_answer(
        &stack, &session, offer, signaling, identity, signals, config,
    )
    .await;
    if result.is_err() {
        if let Err(error) = session.close().await {
            tracing::debug!("清理失败的 NetherNet 入站会话失败：{error}");
        }
        return result;
    }

    incoming_tx
        .send(session)
        .await
        .map_err(|_| NethernetError::Closed)?;
    Ok(())
}

async fn negotiate_answer(
    stack: &Arc<OrtcStack>,
    session: &Arc<NethernetSession>,
    offer: Signal,
    signaling: &Arc<LanSignaling>,
    identity: &ServerIdentity,
    signals: mpsc::Receiver<Signal>,
    config: NegotiationConfig,
) -> Result<()> {
    log_offer_summary(&offer);
    let remote = parse_offer(&offer.data)?;
    log_candidate_summary(
        "NetherNet Offer 远端 ICE 候选摘要",
        offer.connection_id,
        offer.network_id,
        &remote.candidates,
    );
    stack.add_remote_candidates(&remote.candidates).await?;
    let remote_candidate_count = Arc::new(AtomicUsize::new(remote.candidates.len()));
    let remote_candidate_ready = CancellationToken::new();
    if !remote.candidates.is_empty() {
        remote_candidate_ready.cancel();
    }
    let (local_ice, local_dtls) = stack.local_parameters().await?;
    let (signaled_answer, local_candidates) = match config.ice_exchange {
        IceExchangeMode::Trickle => {
            let local_candidates = install_ortc_candidate_queue(
                &stack.gatherer,
                local_ice.username_fragment.clone(),
                config.candidate_encoding,
                offer.connection_id,
                offer.network_id,
            );
            let answer = build_answer(&local_ice, &local_dtls, &[])?;
            let signaled_answer = identity.attach_to_answer(&answer)?;
            (signaled_answer, Some(local_candidates))
        }
        IceExchangeMode::NonTrickle => {
            let candidates = gather_local_candidates(stack).await?;
            let answer = build_answer(&local_ice, &local_dtls, &candidates)?;
            (identity.attach_to_answer(&answer)?, None)
        }
    };
    let answer_summary = summarize_sdp(&signaled_answer);
    signaling
        .signal(Signal::answer(
            offer.connection_id,
            signaled_answer,
            offer.network_id,
        ))
        .await?;
    tracing::info!(
        connection_id = offer.connection_id,
        remote_network_id = offer.network_id,
        identity_algorithm = "ES384",
        setup_role = answer_summary.setup_role,
        inline_candidates = answer_summary.inline_candidates,
        has_identity = answer_summary.has_identity,
        max_message_size = ?answer_summary.max_message_size,
        candidate_delivery = config.ice_exchange.as_str(),
        candidate_encoding = config.candidate_encoding.as_str(),
        "NetherNet Answer 已发送，等待 ICE 和数据通道"
    );
    if let Some(local_candidates) = local_candidates {
        spawn_local_candidate_sender(
            Arc::clone(signaling),
            local_candidates,
            offer.connection_id,
            offer.network_id,
            session.cancellation_token(),
        );
        stack.gatherer.gather().await?;
    }

    spawn_remote_candidate_receiver(RemoteCandidateReceiver {
        stack: Arc::clone(stack),
        signals,
        cancel: session.cancellation_token(),
        connection_id: offer.connection_id,
        remote_network_id: offer.network_id,
        remote_candidate_count: Arc::clone(&remote_candidate_count),
        remote_candidate_ready: remote_candidate_ready.clone(),
    });

    let negotiation = tokio::time::timeout(NEGOTIATION_TIMEOUT, async {
        // GravityCone starts the controlled transport only after the first
        // remote candidate has entered the ICE agent.
        remote_candidate_ready.cancelled().await;
        stack.start(&remote).await?;
        session.wait_ready().await;
        if session.is_closed() {
            return Err(NethernetError::Closed);
        }
        Ok(())
    })
    .await;
    let Ok(result) = negotiation else {
        log_negotiation_timeout(
            stack,
            session,
            offer.connection_id,
            offer.network_id,
            remote_candidate_count.load(Ordering::Relaxed),
        )
        .await;
        return Err(NethernetError::Timeout);
    };
    result?;
    if session.is_closed() {
        return Err(NethernetError::Closed);
    }
    tracing::info!(
        connection_id = offer.connection_id,
        remote_network_id = offer.network_id,
        "NetherNet ICE 与数据通道已就绪"
    );
    Ok(())
}

/// 挂接对端主动创建的数据通道。就绪判定由会话自身完成
/// （两条通道都 open 并接管读循环后触发）。
///
/// 回调只持 `Weak`：会话持有 ORTC 栈，若回调再持强引用会形成引用环，
/// 会话永远不会 `Drop`。
fn install_incoming_channels(stack: &OrtcStack, session: &Arc<NethernetSession>) {
    let session = Arc::downgrade(session);
    stack.install_incoming_channel_handler(move |channel| {
        let session = session.clone();
        Box::pin(async move {
            let Some(session) = session.upgrade() else {
                return;
            };
            tracing::info!(
                label = channel.label(),
                ready_state = ?channel.ready_state(),
                ordered = channel.ordered(),
                max_retransmits = ?channel.max_retransmits(),
                "NetherNet 收到远端数据通道"
            );
            match channel.label() {
                RELIABLE_CHANNEL => session.attach_reliable(&channel),
                UNRELIABLE_CHANNEL => session.attach_unreliable(&channel),
                label => tracing::debug!("忽略未知数据通道：{label}"),
            }
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::MAX_DISCOVERY_PACKET;
    use crate::protocol::DiscoveryPacket;
    use std::net::Ipv4Addr;
    use std::time::Duration;
    use tokio::net::UdpSocket;

    #[tokio::test(flavor = "current_thread")]
    async fn immediate_offer_after_bind_reaches_dispatcher() {
        let client = std::net::UdpSocket::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        let server = LanSignaling::server(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            crate::protocol::ServerData::default(),
        )
        .await
        .unwrap();
        let server_addr = server.local_addr().unwrap();
        let server_id = server.network_id();
        let client_id = server_id.wrapping_add(1);
        let _listener =
            NethernetListener::bind(server, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();

        let offer = Signal::offer(77, String::new(), server_id).to_string();
        let packet = DiscoveryPacket::Message {
            recipient_id: server_id,
            data: offer,
        }
        .encode(client_id)
        .unwrap();
        client.send_to(&packet, server_addr).unwrap();
        client.set_nonblocking(true).unwrap();
        let client = UdpSocket::from_std(client).unwrap();

        let mut buffer = vec![0_u8; MAX_DISCOVERY_PACKET];
        let (length, source) =
            tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buffer))
                .await
                .expect("立即发送的 Offer 不应因订阅竞态而丢失")
                .unwrap();
        assert_eq!(source, server_addr);

        let (response, sender_id) = DiscoveryPacket::decode(&buffer[..length]).unwrap();
        let DiscoveryPacket::Message { recipient_id, data } = response else {
            panic!("无效 Offer 应收到 CONNECTERROR");
        };
        let signal = Signal::parse(&data, sender_id).unwrap();
        assert_eq!(sender_id, server_id);
        assert_eq!(recipient_id, client_id);
        assert_eq!(signal.kind, SignalType::Error);
        assert_eq!(signal.connection_id, 77);
    }
}
