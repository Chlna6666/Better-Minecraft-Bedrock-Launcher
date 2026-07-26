use crate::discovery::{Signal, SignalType};
use crate::{LanSignaling, NethernetError, NethernetSession, Result};
use bytes::Bytes;
use rand::RngExt as _;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use webrtc::api::APIBuilder;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::ice::network_type::NetworkType;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

const RELIABLE_CHANNEL: &str = "ReliableDataChannel";
const UNRELIABLE_CHANNEL: &str = "UnreliableDataChannel";
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(15);
const INCOMING_CAPACITY: usize = 32;
const SIGNAL_CAPACITY: usize = 16;
const MAX_PENDING_CONNECTIONS: usize = 32;
const MAX_PENDING_SIGNALS: usize = 16;
const MAX_CANDIDATE_SIZE: usize = 4 * 1024;

#[derive(Clone)]
pub struct NethernetStream {
    session: Arc<NethernetSession>,
    remote_addr: SocketAddr,
}

impl NethernetStream {
    /// Negotiates a WebRTC session with a discovered `NetherNet` peer.
    ///
    /// # Errors
    ///
    /// Returns an error when signaling, ICE negotiation, or channel setup fails.
    pub async fn connect(
        signaling: Arc<LanSignaling>,
        remote_network_id: u64,
        remote_addr: SocketAddr,
    ) -> Result<Self> {
        let peer_connection = create_peer_connection().await?;
        let session = Arc::new(NethernetSession::new(Arc::clone(&peer_connection)));
        let reliable = peer_connection
            .create_data_channel(
                RELIABLE_CHANNEL,
                Some(RTCDataChannelInit {
                    ordered: Some(true),
                    ..Default::default()
                }),
            )
            .await?;
        let unreliable = peer_connection
            .create_data_channel(
                UNRELIABLE_CHANNEL,
                Some(RTCDataChannelInit {
                    ordered: Some(false),
                    max_retransmits: Some(0),
                    ..Default::default()
                }),
            )
            .await?;
        let (reliable_open_sender, reliable_open_receiver) = oneshot::channel();
        let (unreliable_open_sender, unreliable_open_receiver) = oneshot::channel();
        install_open_handler(&reliable, reliable_open_sender);
        install_open_handler(&unreliable, unreliable_open_sender);
        session.attach_reliable(reliable);
        session.attach_unreliable(unreliable);

        let connection_id = rand::rng().random::<u64>();
        let mut signals = signaling.subscribe();
        let offer = peer_connection.create_offer(None).await?;
        let offer = set_local_description(&peer_connection, offer).await?;
        signaling
            .send_signal(&Signal {
                kind: SignalType::Offer,
                connection_id,
                data: offer,
                network_id: remote_network_id,
            })
            .await?;

        let (answer, pending_candidates) = tokio::time::timeout(NEGOTIATION_TIMEOUT, async {
            let mut pending_candidates = Vec::new();
            loop {
                let signal = receive_signal(&mut signals).await?;
                if signal.connection_id != connection_id || signal.network_id != remote_network_id {
                    continue;
                }
                match signal.kind {
                    SignalType::Answer => return Ok((signal.data, pending_candidates)),
                    SignalType::Candidate if pending_candidates.len() < MAX_PENDING_SIGNALS => {
                        pending_candidates.push(parse_candidate(&signal.data));
                    }
                    SignalType::Error => {
                        return Err(NethernetError::Protocol(format!(
                            "NetherNet 对端拒绝连接：{}",
                            signal.data
                        )));
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| NethernetError::Timeout)??;

        let answer = RTCSessionDescription::answer(answer)?;
        peer_connection.set_remote_description(answer).await?;
        for candidate in pending_candidates {
            peer_connection.add_ice_candidate(candidate).await?;
        }
        start_candidate_receiver(
            Arc::clone(&peer_connection),
            signals,
            connection_id,
            remote_network_id,
            session.cancellation_token(),
        );
        tokio::time::timeout(NEGOTIATION_TIMEOUT, async {
            reliable_open_receiver
                .await
                .map_err(|_| NethernetError::Closed)?;
            unreliable_open_receiver
                .await
                .map_err(|_| NethernetError::Closed)?;
            wait_for_ice(&peer_connection).await
        })
        .await
        .map_err(|_| NethernetError::Timeout)??;
        Ok(Self {
            session,
            remote_addr,
        })
    }

    /// Sends one complete `NetherNet` packet.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is closed or the WebRTC send fails.
    pub async fn send(&self, data: Bytes) -> Result<()> {
        self.session.send(data).await
    }

    /// Receives the next complete `NetherNet` packet.
    ///
    /// # Errors
    ///
    /// Returns an error when the session receive path fails.
    pub async fn recv(&self) -> Result<Option<Bytes>> {
        self.session.recv().await
    }

    /// Closes the `NetherNet` session.
    ///
    /// # Errors
    ///
    /// Returns an error when WebRTC peer shutdown fails.
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

pub struct NethernetListener {
    incoming: mpsc::Receiver<Arc<NethernetSession>>,
    local_addr: SocketAddr,
    cancellation: CancellationToken,
    signal_task: JoinHandle<()>,
}

impl NethernetListener {
    /// Creates a listener over an already-bound LAN signaling endpoint.
    ///
    /// # Errors
    ///
    /// This constructor currently has no fallible setup after signaling is bound.
    pub fn bind(signaling: LanSignaling, local_addr: SocketAddr) -> Result<Self> {
        let signaling = Arc::new(signaling);
        let mut signals = signaling.subscribe();
        let (incoming_sender, incoming) = mpsc::channel(INCOMING_CAPACITY);
        let cancellation = CancellationToken::new();
        let signal_task = tokio::spawn({
            let signaling = Arc::clone(&signaling);
            let cancellation = cancellation.clone();
            async move {
                let mut dispatchers: HashMap<u64, mpsc::Sender<Signal>> = HashMap::new();
                let mut pending: HashMap<u64, Vec<Signal>> = HashMap::new();
                loop {
                    tokio::select! {
                        () = cancellation.cancelled() => break,
                        signal = receive_signal(&mut signals) => {
                            let Ok(signal) = signal else { break };
                            dispatchers.retain(|_, dispatcher| !dispatcher.is_closed());
                            if signal.kind == SignalType::Candidate
                                && signal.data.len() > MAX_CANDIDATE_SIZE
                            {
                                tracing::debug!(
                                    length = signal.data.len(),
                                    "忽略过大的 NetherNet ICE 候选"
                                );
                                continue;
                            }
                            if signal.kind == SignalType::Offer {
                                if dispatchers.contains_key(&signal.connection_id)
                                    || dispatchers.len() >= MAX_PENDING_CONNECTIONS
                                {
                                    continue;
                                }
                                let connection_id = signal.connection_id;
                                let (signal_sender, signal_receiver) = mpsc::channel(SIGNAL_CAPACITY);
                                dispatchers.insert(connection_id, signal_sender.clone());
                                if let Some(buffered) = pending.remove(&connection_id) {
                                    for buffered_signal in buffered {
                                        if signal_sender.try_send(buffered_signal).is_err() {
                                            break;
                                        }
                                    }
                                }
                                tokio::spawn(handle_offer(
                                    signal,
                                    Arc::clone(&signaling),
                                    signal_receiver,
                                    incoming_sender.clone(),
                                ));
                            } else if let Some(dispatcher) = dispatchers.get(&signal.connection_id) {
                                let connection_id = signal.connection_id;
                                if dispatcher.send(signal).await.is_err() {
                                    dispatchers.remove(&connection_id);
                                }
                            } else if signal.kind == SignalType::Candidate
                                && pending.len() < MAX_PENDING_CONNECTIONS
                            {
                                let buffered = pending.entry(signal.connection_id).or_default();
                                if buffered.len() < MAX_PENDING_SIGNALS {
                                    buffered.push(signal);
                                }
                            }
                        }
                    }
                }
            }
        });
        Ok(Self {
            incoming,
            local_addr,
            cancellation,
            signal_task,
        })
    }

    /// Waits for the next negotiated `NetherNet` session.
    ///
    /// # Errors
    ///
    /// Returns an error after the listener is closed.
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
        self.cancellation.cancel();
        self.signal_task.abort();
    }
}

async fn handle_offer(
    offer: Signal,
    signaling: Arc<LanSignaling>,
    mut signals: mpsc::Receiver<Signal>,
    incoming_sender: mpsc::Sender<Arc<NethernetSession>>,
) {
    let result = async {
        let peer_connection = create_peer_connection().await?;
        let session = Arc::new(NethernetSession::new(Arc::clone(&peer_connection)));
        let (ready_sender, ready_receiver) = oneshot::channel();
        let ready_sender = Arc::new(Mutex::new(Some(ready_sender)));
        install_incoming_channels(&peer_connection, Arc::clone(&session), ready_sender);
        let description = RTCSessionDescription::offer(offer.data)?;
        peer_connection.set_remote_description(description).await?;
        let answer = peer_connection.create_answer(None).await?;
        let answer = set_local_description(&peer_connection, answer).await?;
        signaling
            .send_signal(&Signal {
                kind: SignalType::Answer,
                connection_id: offer.connection_id,
                data: answer,
                network_id: offer.network_id,
            })
            .await?;

        let cancellation = session.cancellation_token();
        let candidate_connection = Arc::clone(&peer_connection);
        let candidate_cancellation = cancellation.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = candidate_cancellation.cancelled() => break,
                    signal = signals.recv() => {
                        let Some(signal) = signal else { break };
                        match signal.kind {
                            SignalType::Candidate => {
                                if let Err(error) = candidate_connection
                                    .add_ice_candidate(parse_candidate(&signal.data))
                                    .await
                                {
                                    tracing::debug!("添加 NetherNet ICE 候选失败：{error}");
                                }
                            }
                            SignalType::Error => break,
                            _ => {}
                        }
                    }
                }
            }
        });
        tokio::time::timeout(NEGOTIATION_TIMEOUT, async {
            ready_receiver.await.map_err(|_| NethernetError::Closed)?;
            wait_for_ice(&peer_connection).await
        })
        .await
        .map_err(|_| NethernetError::Timeout)??;
        incoming_sender
            .send(Arc::clone(&session))
            .await
            .map_err(|_| NethernetError::Closed)?;
        Ok::<(), NethernetError>(())
    }
    .await;
    if let Err(error) = result {
        tracing::debug!("接受 NetherNet WebRTC 会话失败：{error}");
    }
}

async fn create_peer_connection() -> Result<Arc<RTCPeerConnection>> {
    let media_engine = MediaEngine::default();
    let mut setting_engine = SettingEngine::default();
    setting_engine.set_network_types(vec![NetworkType::Udp4]);
    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_setting_engine(setting_engine)
        .build();
    Ok(Arc::new(
        api.new_peer_connection(RTCConfiguration::default()).await?,
    ))
}

fn install_open_handler(channel: &RTCDataChannel, sender: oneshot::Sender<()>) {
    let sender = Arc::new(Mutex::new(Some(sender)));
    channel.on_open(Box::new(move || {
        let sender = Arc::clone(&sender);
        Box::pin(async move {
            if let Some(sender) = sender.lock().await.take()
                && sender.send(()).is_err()
            {
                tracing::trace!("NetherNet 通道就绪通知已关闭");
            }
        })
    }));
}

fn install_incoming_channels(
    peer_connection: &RTCPeerConnection,
    session: Arc<NethernetSession>,
    ready_sender: Arc<Mutex<Option<oneshot::Sender<()>>>>,
) {
    peer_connection.on_data_channel(Box::new(move |channel| {
        let session = Arc::clone(&session);
        let ready_sender = Arc::clone(&ready_sender);
        Box::pin(async move {
            match channel.label() {
                RELIABLE_CHANNEL => session.attach_reliable(channel),
                UNRELIABLE_CHANNEL => session.attach_unreliable(channel),
                label => {
                    tracing::debug!("忽略未知 NetherNet 数据通道：{label}");
                    return;
                }
            }
            if session.channels_ready()
                && let Some(sender) = ready_sender.lock().await.take()
                && sender.send(()).is_err()
            {
                tracing::trace!("NetherNet 会话就绪通知已关闭");
            }
        })
    }));
}

fn start_candidate_receiver(
    peer_connection: Arc<RTCPeerConnection>,
    mut signals: tokio::sync::broadcast::Receiver<Signal>,
    connection_id: u64,
    remote_network_id: u64,
    cancellation: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancellation.cancelled() => break,
                signal = receive_signal(&mut signals) => {
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
                        tracing::debug!("添加 NetherNet ICE 候选失败：{error}");
                    }
                }
            }
        }
    });
}

async fn set_local_description(
    peer_connection: &RTCPeerConnection,
    description: RTCSessionDescription,
) -> Result<String> {
    let mut gathering_complete = peer_connection.gathering_complete_promise().await;
    peer_connection.set_local_description(description).await?;
    gathering_complete.recv().await;
    peer_connection
        .local_description()
        .await
        .map(|description| description.sdp)
        .ok_or_else(|| NethernetError::Protocol("WebRTC 本地描述缺失".to_string()))
}

async fn receive_signal(receiver: &mut tokio::sync::broadcast::Receiver<Signal>) -> Result<Signal> {
    loop {
        match receiver.recv().await {
            Ok(signal) => return Ok(signal),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!("NetherNet 信令接收落后，跳过 {skipped} 条消息");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                return Err(NethernetError::Closed);
            }
        }
    }
}

fn parse_candidate(data: &str) -> RTCIceCandidateInit {
    serde_json::from_str(data).unwrap_or_else(|_| RTCIceCandidateInit {
        candidate: data.to_string(),
        sdp_mid: None,
        sdp_mline_index: None,
        username_fragment: None,
    })
}

async fn wait_for_ice(peer_connection: &RTCPeerConnection) -> Result<()> {
    loop {
        match peer_connection.ice_connection_state() {
            RTCIceConnectionState::Connected | RTCIceConnectionState::Completed => return Ok(()),
            RTCIceConnectionState::Failed
            | RTCIceConnectionState::Disconnected
            | RTCIceConnectionState::Closed => return Err(NethernetError::Closed),
            _ => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LanSignaling, NethernetListener, NethernetStream};
    use crate::ServerData;
    use bytes::Bytes;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn loopback_discovery_and_data_channels_round_trip() {
        let server_data = ServerData {
            server_name: "BMCBL".to_string(),
            level_name: "PaperConnect".to_string(),
            game_type: 0,
            player_count: 1,
            max_player_count: 20,
            editor_world: false,
            hardcore: false,
            accepts_online_auth: true,
            accepts_self_signed_auth: true,
            transport_layer: 2,
            connection_type: 4,
        };
        let server_signaling =
            LanSignaling::server(SocketAddr::from(([127, 0, 0, 1], 0)), server_data.clone())
                .await
                .expect("bind server signaling");
        let server_address = server_signaling.local_addr().expect("server address");
        let client_signaling = Arc::new(
            LanSignaling::client(SocketAddr::from(([127, 0, 0, 1], 0)), server_address)
                .await
                .expect("bind client signaling"),
        );
        let discovered = client_signaling
            .discover(Duration::from_secs(2))
            .await
            .expect("discover server");
        assert_eq!(discovered.server_data, server_data);

        let mut listener =
            NethernetListener::bind(server_signaling, SocketAddr::from(([127, 0, 0, 1], 0)))
                .expect("bind NetherNet listener");
        let (client, server) = tokio::time::timeout(Duration::from_secs(20), async {
            tokio::try_join!(
                NethernetStream::connect(
                    Arc::clone(&client_signaling),
                    discovered.network_id,
                    discovered.address,
                ),
                listener.accept(),
            )
        })
        .await
        .expect("negotiate before timeout")
        .expect("negotiate loopback session");

        client
            .send(Bytes::from_static(b"request"))
            .await
            .expect("send request");
        assert_eq!(
            server.recv().await.expect("receive request"),
            Some(Bytes::from_static(b"request"))
        );
        server
            .send(Bytes::from_static(b"response"))
            .await
            .expect("send response");
        assert_eq!(
            client.recv().await.expect("receive response"),
            Some(Bytes::from_static(b"response"))
        );
        client.close().await.expect("close client");
        server.close().await.expect("close server");
    }

    #[test]
    fn transport_handles_support_multithreaded_runtime_tasks() {
        fn assert_send<T: Send>() {}
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send::<NethernetListener>();
        assert_send_sync::<NethernetStream>();
    }
}
