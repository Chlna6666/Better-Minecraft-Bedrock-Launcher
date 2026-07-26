//! 服务端：接受入站协商请求。

use crate::consts::{MAX_SIGNAL_SIZE, NEGOTIATION_TIMEOUT};
use crate::error::{NethernetError, Result, SignalErrorCode};
use crate::protocol::{Signal, SignalType};
use crate::session::NethernetSession;
use crate::signaling::LanSignaling;
use crate::transport::negotiate::{
    create_peer_connection, finish_local_description, parse_candidate,
};
use crate::transport::stream::{recv_signal, wait_for_ice};
use crate::{RELIABLE_CHANNEL, UNRELIABLE_CHANNEL};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

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
        let signaling = Arc::new(signaling);
        let (incoming_tx, incoming) = mpsc::channel(INCOMING_CAPACITY);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(dispatch_loop(
            Arc::clone(&signaling),
            incoming_tx,
            cancel.clone(),
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
        self.incoming.recv().await.ok_or(NethernetError::Closed)
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
    incoming_tx: mpsc::Sender<Arc<NethernetSession>>,
    cancel: CancellationToken,
) {
    let mut signals = signaling.subscribe();
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
                if routes.len() >= MAX_CONCURRENT_NEGOTIATIONS {
                    tracing::debug!("并发协商数已达上限，拒绝新 offer");
                    let _ = signaling
                        .signal(Signal::error(
                            signal.connection_id,
                            SignalErrorCode::IncomingConnectionIgnored,
                            signal.network_id,
                        ))
                        .await;
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
                    route_rx,
                    incoming_tx.clone(),
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
    signals: mpsc::Receiver<Signal>,
    incoming_tx: mpsc::Sender<Arc<NethernetSession>>,
) {
    let connection_id = offer.connection_id;
    let remote_network_id = offer.network_id;
    if let Err(error) = negotiate_incoming(offer, &signaling, signals, incoming_tx).await {
        tracing::debug!(connection_id, "接受 NetherNet 会话失败：{error}");
        let code = match &error {
            NethernetError::Timeout => SignalErrorCode::NegotiationTimeout,
            NethernetError::Refused(code) => *code,
            _ => SignalErrorCode::Ice,
        };
        let _ = signaling
            .signal(Signal::error(connection_id, code, remote_network_id))
            .await;
    }
}

async fn negotiate_incoming(
    offer: Signal,
    signaling: &Arc<LanSignaling>,
    signals: mpsc::Receiver<Signal>,
    incoming_tx: mpsc::Sender<Arc<NethernetSession>>,
) -> Result<()> {
    let peer_connection = create_peer_connection().await?;
    let session = NethernetSession::new(Arc::clone(&peer_connection));
    install_incoming_channels(&peer_connection, &session);

    // 任一步失败都必须终结会话：否则候选接收任务不会退出，
    // 它持有的路由发送端也就不会关闭，监听器的并发协商槽永久泄漏
    // ——32 个伪造 offer 即可让监听器此后拒绝一切新连接。
    let result = negotiate_answer(&peer_connection, &session, offer, signaling, signals).await;
    if result.is_err() {
        let _ = session.close().await;
        return result;
    }

    incoming_tx
        .send(session)
        .await
        .map_err(|_| NethernetError::Closed)?;
    Ok(())
}

async fn negotiate_answer(
    peer_connection: &Arc<RTCPeerConnection>,
    session: &Arc<NethernetSession>,
    offer: Signal,
    signaling: &Arc<LanSignaling>,
    mut signals: mpsc::Receiver<Signal>,
) -> Result<()> {
    peer_connection
        .set_remote_description(RTCSessionDescription::offer(offer.data)?)
        .await?;
    let answer = peer_connection.create_answer(None).await?;
    let answer = finish_local_description(peer_connection, answer).await?;
    signaling
        .signal(Signal::answer(
            offer.connection_id,
            answer,
            offer.network_id,
        ))
        .await?;

    // 协商期与建立后共用同一个候选接收任务，随会话取消令牌一起结束。
    let cancel = session.cancellation_token();
    tokio::spawn({
        let peer_connection = Arc::clone(peer_connection);
        async move {
            loop {
                let signal = tokio::select! {
                    () = cancel.cancelled() => break,
                    signal = signals.recv() => signal,
                };
                let Some(signal) = signal else { break };
                match signal.kind {
                    SignalType::Candidate => {
                        if let Err(error) = peer_connection
                            .add_ice_candidate(parse_candidate(&signal.data))
                            .await
                        {
                            tracing::debug!("添加 ICE 候选失败：{error}");
                        }
                    }
                    SignalType::Error => break,
                    _ => {}
                }
            }
        }
    });

    tokio::time::timeout(NEGOTIATION_TIMEOUT, async {
        session.wait_ready().await;
        wait_for_ice(peer_connection).await
    })
    .await
    .map_err(|_| NethernetError::Timeout)??;
    if session.is_closed() {
        return Err(NethernetError::Closed);
    }
    Ok(())
}

/// 挂接对端主动创建的数据通道。就绪判定由会话自身完成
/// （两条通道都 open 并接管读循环后触发）。
///
/// 回调只持 `Weak`：会话持有 `Arc<RTCPeerConnection>`，若回调再持强引用
/// 就形成 会话 → 对等连接 → 回调 → 会话 的环，会话永远不会 `Drop`。
fn install_incoming_channels(peer_connection: &RTCPeerConnection, session: &Arc<NethernetSession>) {
    let session = Arc::downgrade(session);
    peer_connection.on_data_channel(Box::new(move |channel| {
        let session = session.clone();
        Box::pin(async move {
            let Some(session) = session.upgrade() else {
                return;
            };
            match channel.label() {
                RELIABLE_CHANNEL => session.attach_reliable(&channel),
                UNRELIABLE_CHANNEL => session.attach_unreliable(&channel),
                label => tracing::debug!("忽略未知数据通道：{label}"),
            }
        })
    }));
}
