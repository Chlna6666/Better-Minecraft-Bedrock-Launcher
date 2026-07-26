use super::paperconnect::{PaperConnectProtocol, ServerInfo};
use super::paperconnect_discovery::{
    RakNetServerInfo, scan_local_raknet, start_fake_raknet_server,
};
use super::paperconnect_tunnel::{TunnelDecoder, encode_packet};
use bedrock_nethernet::{
    LanSignaling, NethernetListener, NethernetSession, NethernetStream, ServerData,
};
use bytes::Bytes;
use once_cell::sync::Lazy;
use raknet_tokio::prelude::{RakClient, RakPriority, RakReliability, RakServer, RakSession};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

const NETHERNET_DISCOVERY_PORT: u16 = 7551;
const RAKNET_MTU: u16 = 1200;
const MAX_HOST_SESSIONS: usize = 20;
const LOCAL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const RAKNET_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const NETHERNET_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

struct ManagedTransport {
    cancel: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl ManagedTransport {
    fn cancel(mut self) {
        if let Some(cancel) = self.cancel.take()
            && cancel.send(()).is_err()
            && !self.task.is_finished()
        {
            self.task.abort();
        }
    }
}

static HOST_TRANSPORT: Lazy<Mutex<Option<ManagedTransport>>> = Lazy::new(|| Mutex::new(None));
static GUEST_TRANSPORT: Lazy<Mutex<Option<ManagedTransport>>> = Lazy::new(|| Mutex::new(None));

#[derive(Debug, Clone)]
pub struct HostTransportInfo {
    pub protocol: PaperConnectProtocol,
    pub game_port: u16,
}

pub async fn start_host() -> Result<HostTransportInfo, String> {
    stop_host();
    let nethernet_discovery = detect_local_nethernet();
    let raknet_discovery = scan_local_raknet(LOCAL_DISCOVERY_TIMEOUT);
    tokio::pin!(nethernet_discovery, raknet_discovery);
    tokio::select! {
        biased;
        nethernet_result = &mut nethernet_discovery => {
            if nethernet_result.is_ok() {
                return start_nethernet_host().await;
            }
            if let Ok(server) = raknet_discovery.await {
                return Ok(raknet_host_info(server));
            }
        }
        raknet_result = &mut raknet_discovery => {
            if let Ok(server) = raknet_result {
                return Ok(raknet_host_info(server));
            }
            if nethernet_discovery.await.is_ok() {
                return start_nethernet_host().await;
            }
        }
    }
    Err("未检测到本机 Minecraft 基岩版局域网世界，请先在游戏中开启局域网联机".to_string())
}

fn raknet_host_info(server: RakNetServerInfo) -> HostTransportInfo {
    tracing::info!(
        server_name = %server.server_name,
        level_name = %server.level_name,
        game_port = server.game_port,
        "PaperConnect 使用本机 RakNet 局域网世界"
    );
    HostTransportInfo {
        protocol: PaperConnectProtocol::Raknet,
        game_port: server.game_port,
    }
}

pub async fn start_guest(server: &ServerInfo, player_name: &str) -> Result<(), String> {
    stop_guest();
    let host_player = super::paperconnect::players()
        .into_iter()
        .find(|player| player.is_room_host)
        .map(|player| player.player)
        .unwrap_or_else(|| "房主".to_string());
    let display_name = format!("BMCBL | {host_player}");
    match server.protocol {
        PaperConnectProtocol::Nethernet => {
            start_nethernet_guest(server.game_port, display_name, player_name).await
        }
        PaperConnectProtocol::Raknet => start_raknet_guest(server.game_port, display_name).await,
    }
}

pub fn stop_all() {
    stop_guest();
    stop_host();
}

fn stop_host() {
    if let Ok(mut transport) = HOST_TRANSPORT.lock()
        && let Some(transport) = transport.take()
    {
        transport.cancel();
    }
}

fn stop_guest() {
    if let Ok(mut transport) = GUEST_TRANSPORT.lock()
        && let Some(transport) = transport.take()
    {
        transport.cancel();
    }
}

async fn detect_local_nethernet() -> Result<ServerData, String> {
    let signaling = LanSignaling::client(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        SocketAddr::from(([127, 0, 0, 1], NETHERNET_DISCOVERY_PORT)),
    )
    .await
    .map_err(|error| format!("启动本机 NetherNet 发现失败：{error}"))?;
    let discovered = signaling
        .discover(LOCAL_DISCOVERY_TIMEOUT)
        .await
        .map_err(|error| format!("发现本机 NetherNet 世界失败：{error}"))?;
    let server_data = discovered.server_data;
    tracing::info!(
        server_name = %server_data.server_name,
        level_name = %server_data.level_name,
        "PaperConnect 检测到本机 NetherNet 世界"
    );
    Ok(server_data)
}

async fn start_nethernet_host() -> Result<HostTransportInfo, String> {
    let game_port = super::paperconnect_pick_udp_port()?;
    let mut server = RakServer::new(SocketAddr::from(([0, 0, 0, 0], game_port)), |config| {
        config.max_connections = MAX_HOST_SESSIONS;
        config.max_mtu_size = RAKNET_MTU;
    });
    server
        .start()
        .await
        .map_err(|error| format!("启动 PaperConnect RakNet 隧道失败：{error}"))?;
    let (cancel_sender, mut cancel_receiver) = oneshot::channel();
    let task = crate::tasks::runtime::spawn_io(async move {
        let mut sessions = JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut cancel_receiver => break,
                accepted = server.accept() => {
                    match accepted {
                        Ok(session) if sessions.len() < MAX_HOST_SESSIONS => {
                            sessions.spawn(async move {
                                if let Err(error) = run_host_session(session).await {
                                    tracing::debug!("PaperConnect 房主隧道会话结束：{error}");
                                }
                            });
                        }
                        Ok(session) => {
                            if let Err(error) = session.close().await {
                                tracing::debug!("拒绝超出上限的 RakNet 会话失败：{error}");
                            }
                        }
                        Err(error) => {
                            tracing::debug!("PaperConnect RakNet 接收失败：{error}");
                            break;
                        }
                    }
                }
                Some(joined) = sessions.join_next(), if !sessions.is_empty() => {
                    if let Err(error) = joined {
                        tracing::debug!("PaperConnect 房主隧道任务异常：{error}");
                    }
                }
            }
        }
        sessions.abort_all();
        while let Some(joined) = sessions.join_next().await {
            if let Err(error) = joined
                && !error.is_cancelled()
            {
                tracing::debug!("清理 PaperConnect 房主隧道任务失败：{error}");
            }
        }
        server.stop();
    })?;
    set_transport(
        &HOST_TRANSPORT,
        ManagedTransport {
            cancel: Some(cancel_sender),
            task,
        },
        "房主",
    )?;
    Ok(HostTransportInfo {
        protocol: PaperConnectProtocol::Nethernet,
        game_port,
    })
}

async fn run_host_session(mut raknet: RakSession) -> Result<(), String> {
    let signaling = LanSignaling::client(
        SocketAddr::from(([127, 0, 0, 1], 0)),
        SocketAddr::from(([127, 0, 0, 1], NETHERNET_DISCOVERY_PORT)),
    )
    .await
    .map_err(|error| format!("启动本机 NetherNet 信令失败：{error}"))?;
    let discovered = signaling
        .discover(LOCAL_DISCOVERY_TIMEOUT)
        .await
        .map_err(|error| format!("发现本机 NetherNet 世界失败：{error}"))?;
    let nethernet = tokio::time::timeout(
        NETHERNET_CONNECT_TIMEOUT,
        NethernetStream::connect(
            Arc::new(signaling),
            discovered.network_id,
            discovered.address,
        ),
    )
    .await
    .map_err(|_| "连接本机 NetherNet 世界超时".to_string())?
    .map_err(|error| format!("连接本机 NetherNet 世界失败：{error}"))?;
    let result = proxy_stream_to_raknet(&nethernet, &mut raknet).await;
    if let Err(error) = nethernet.close().await {
        tracing::debug!("关闭房主 NetherNet 会话失败：{error}");
    }
    if let Err(error) = raknet.close().await {
        tracing::debug!("关闭房主 RakNet 会话失败：{error}");
    }
    result
}

async fn start_nethernet_guest(
    proxy_port: u16,
    display_name: String,
    player_name: &str,
) -> Result<(), String> {
    let mut raknet_client = RakClient::new(|config| {
        config.max_mtu_size = RAKNET_MTU;
    });
    raknet_client
        .start()
        .await
        .map_err(|error| format!("启动 PaperConnect RakNet 客户端失败：{error}"))?;
    let mut raknet = tokio::time::timeout(
        RAKNET_CONNECT_TIMEOUT,
        raknet_client.connect(SocketAddr::from(([127, 0, 0, 1], proxy_port))),
    )
    .await
    .map_err(|_| "连接房主 RakNet 隧道超时".to_string())?
    .map_err(|error| format!("连接房主 RakNet 隧道失败：{error}"))?;

    let server_data = ServerData {
        server_name: display_name,
        level_name: format!("PaperConnect - {player_name}"),
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
    let signaling = LanSignaling::server(
        SocketAddr::from(([0, 0, 0, 0], NETHERNET_DISCOVERY_PORT)),
        server_data,
    )
    .await
    .map_err(|error| format!("启动本机 NetherNet 7551 服务失败：{error}"))?;
    let mut listener = NethernetListener::bind(signaling, SocketAddr::from(([127, 0, 0, 1], 0)))
        .map_err(|error| format!("启动本机 NetherNet 房间失败：{error}"))?;

    let (cancel_sender, mut cancel_receiver) = oneshot::channel();
    let task = crate::tasks::runtime::spawn_io(async move {
        tokio::select! {
            _ = &mut cancel_receiver => {}
            accepted = listener.accept() => {
                match accepted {
                    Ok(nethernet) => {
                        if let Err(error) =
                            proxy_session_to_raknet(&nethernet, &mut raknet, &mut cancel_receiver).await
                        {
                            tracing::debug!("PaperConnect 客户端桥接结束：{error}");
                        }
                        if let Err(error) = nethernet.close().await {
                            tracing::debug!("关闭客户端 NetherNet 会话失败：{error}");
                        }
                    }
                    Err(error) => tracing::debug!("接受本机 NetherNet 会话失败：{error}"),
                }
            }
        }
        if let Err(error) = raknet.close().await {
            tracing::debug!("关闭客户端 RakNet 会话失败：{error}");
        }
        raknet_client.stop();
    })?;
    set_transport(
        &GUEST_TRANSPORT,
        ManagedTransport {
            cancel: Some(cancel_sender),
            task,
        },
        "客户端",
    )
}

async fn start_raknet_guest(proxy_port: u16, display_name: String) -> Result<(), String> {
    let (cancel_sender, cancel_receiver) = oneshot::channel();
    let task = start_fake_raknet_server(display_name, proxy_port, cancel_receiver).await?;
    set_transport(
        &GUEST_TRANSPORT,
        ManagedTransport {
            cancel: Some(cancel_sender),
            task,
        },
        "客户端",
    )
}

async fn proxy_stream_to_raknet(
    nethernet: &NethernetStream,
    raknet: &mut RakSession,
) -> Result<(), String> {
    let mut decoder = TunnelDecoder::default();
    loop {
        tokio::select! {
            packet = nethernet.recv() => {
                let packet = packet
                    .map_err(|error| format!("读取 NetherNet 数据失败：{error}"))?
                    .ok_or_else(|| "NetherNet 会话已关闭".to_string())?;
                send_tunnel_packet(raknet, &packet).await?;
            }
            frame = raknet.recv::<Box<[u8]>>() => {
                let frame = frame.map_err(|error| format!("读取 RakNet 隧道失败：{error}"))?;
                if let Some(packet) = decoder.push(Bytes::from(frame))? {
                    nethernet
                        .send(packet)
                        .await
                        .map_err(|error| format!("写入 NetherNet 数据失败：{error}"))?;
                }
            }
        }
    }
}

async fn proxy_session_to_raknet(
    nethernet: &Arc<NethernetSession>,
    raknet: &mut RakSession,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<(), String> {
    let mut decoder = TunnelDecoder::default();
    loop {
        tokio::select! {
            _ = &mut *cancel => return Ok(()),
            packet = nethernet.recv() => {
                let packet = packet
                    .map_err(|error| format!("读取本机 NetherNet 数据失败：{error}"))?
                    .ok_or_else(|| "本机 NetherNet 会话已关闭".to_string())?;
                send_tunnel_packet(raknet, &packet).await?;
            }
            frame = raknet.recv::<Box<[u8]>>() => {
                let frame = frame.map_err(|error| format!("读取房主 RakNet 隧道失败：{error}"))?;
                if let Some(packet) = decoder.push(Bytes::from(frame))? {
                    nethernet
                        .send(packet)
                        .await
                        .map_err(|error| format!("写入本机 NetherNet 数据失败：{error}"))?;
                }
            }
        }
    }
}

async fn send_tunnel_packet(raknet: &RakSession, packet: &Bytes) -> Result<(), String> {
    for frame in encode_packet(packet)? {
        raknet
            .send(frame, RakReliability::ReliableOrdered, RakPriority::High)
            .await
            .map_err(|error| format!("写入 RakNet 隧道失败：{error}"))?;
    }
    Ok(())
}

fn set_transport(
    slot: &Mutex<Option<ManagedTransport>>,
    transport: ManagedTransport,
    description: &str,
) -> Result<(), String> {
    let mut slot = slot
        .lock()
        .map_err(|_| format!("PaperConnect {description}传输状态锁已损坏"))?;
    if let Some(previous) = slot.replace(transport) {
        previous.cancel();
    }
    Ok(())
}
