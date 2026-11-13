use anyhow::{Context as _, anyhow};
use ::easytier::common::config::{
    ConfigFileControl, ConfigLoader as _, NetworkIdentity, PeerConfig, TomlConfigLoader,
    gen_default_flags,
};
use ::easytier::instance_manager::NetworkInstanceManager;
use ::easytier::proto::api::config::{
    ConfigPatchAction, InstanceConfigPatch, PatchConfigRequest, PortForwardPatch,
};
use ::easytier::proto::api::instance::{
    ListPeerRequest, ListRouteRequest, PeerConnInfo, PeerInfo, list_peer_route_pair,
};
use ::easytier::proto::common::{CompressionAlgoPb, PortForwardConfigPb, SocketType};
use ::easytier::proto::rpc_types::controller::BaseController;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

pub mod easytier;
pub mod paperconnect;
mod paperconnect_discovery;
pub mod paperconnect_guest;
mod paperconnect_transport;
mod paperconnect_tunnel;

use crate::http::proxy::clients::{no_proxy, no_proxy_for_host};
use crate::utils::cloudflare;

const DEFAULT_PAPERCONNECT_VIP: &str = "10.144.144.1";
const DEFAULT_BOOTSTRAP_PEERS: [&str; 2] = [
    "wss://center.node.1tmc.top",
    "tcp://public.easytier.bmcbl.com:54321",
];
const PUBLIC_BOOTSTRAP_PEERS_URL: &str = "https://et-public-node.roundstudio.top/";
const PUBLIC_BOOTSTRAP_PEERS_HOST: &str = "et-public-node.roundstudio.top";
const BOOTSTRAP_PEERS_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const BOOTSTRAP_FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const EASYTIER_API_TIMEOUT: Duration = Duration::from_secs(3);
const EASYTIER_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const PAPERCONNECT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const PAPERCONNECT_PROBE_RETRY_INTERVAL: Duration = Duration::from_millis(500);

struct BootstrapPeersCache {
    fetched_at: Instant,
    peers: Vec<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PaperConnectClientState {
    Ready,
    DiscoveryPortOccupied,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EasyTierPeer {
    pub ipv4: Option<String>,
    pub hostname: String,
    pub connection_kind: EasyTierConnectionKind,
    pub protocol: Option<String>,
    pub remote_endpoint: Option<String>,
    pub latency_ms: Option<u64>,
    pub via_hostname: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EasyTierAbandonedConnector {
    pub url: String,
    pub failed_attempts: u8,
}

#[derive(Debug, Clone, Copy, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum EasyTierConnectionKind {
    Local,
    Direct,
    Relayed,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EasyTierEmbeddedStatus {
    pub instance_id: String,
    pub hostname: String,
    pub ipv4: Option<String>,
    pub game_host: Option<String>,
    pub game_port: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EasyTierStartOptions {
    #[serde(alias = "disableP2p", alias = "disable_p2p")]
    pub disable_p2p: Option<bool>,
    #[serde(
        alias = "compression",
        alias = "dataCompressAlgo",
        alias = "data_compress_algo"
    )]
    pub compression: Option<String>,
    #[serde(alias = "ipv4")]
    pub ipv4: Option<String>,
}

#[derive(Debug)]
pub struct EasyTierStartRequest {
    pub network_name: String,
    pub network_secret: String,
    pub peers: Vec<String>,
    pub hostname: Option<String>,
    pub player_name: String,
    pub game_port: u16,
    pub options: Option<EasyTierStartOptions>,
}

#[derive(Debug, Clone)]
struct EasyTierLastStart {
    hostname: Option<String>,
    resolved_hostname: Option<String>,
    resolved_ipv4: Option<String>,
    game_port: u16,
    options: Option<EasyTierStartOptions>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EasyTierGameEndpoint {
    host: String,
    port: u16,
}

#[derive(Default)]
struct OnlineState {
    easytier_manager: Arc<NetworkInstanceManager>,
    easytier_instance_id: Mutex<Option<Uuid>>,
    easytier_last_start: Mutex<Option<EasyTierLastStart>>,
    easytier_game_endpoint: Mutex<Option<EasyTierGameEndpoint>>,
    paperconnect_guest_transport: Mutex<Option<(paperconnect::ServerInfo, String)>>,
    easytier_abandoned_connectors: Mutex<Vec<EasyTierAbandonedConnector>>,
    easytier_cleanup_in_progress: Arc<AtomicBool>,
}

static ONLINE_STATE: Lazy<OnlineState> = Lazy::new(|| OnlineState {
    easytier_manager: Arc::new(NetworkInstanceManager::new()),
    easytier_instance_id: Mutex::new(None),
    easytier_last_start: Mutex::new(None),
    easytier_game_endpoint: Mutex::new(None),
    paperconnect_guest_transport: Mutex::new(None),
    easytier_abandoned_connectors: Mutex::new(Vec::new()),
    easytier_cleanup_in_progress: Arc::new(AtomicBool::new(false)),
});
static BOOTSTRAP_PEERS_CACHE: Lazy<Mutex<Option<BootstrapPeersCache>>> =
    Lazy::new(|| Mutex::new(None));

fn fallback_bootstrap_peers() -> Vec<String> {
    DEFAULT_BOOTSTRAP_PEERS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn is_supported_bootstrap_peer(peer: &str) -> bool {
    matches!(
        url::Url::parse(peer).ok().map(|url| url.scheme().to_ascii_lowercase()),
        Some(scheme) if matches!(scheme.as_str(), "tcp" | "udp" | "ws" | "wss")
    )
}

fn sanitize_bootstrap_peers(peers: Vec<String>) -> Vec<String> {
    let mut sanitized = Vec::new();

    for peer in peers {
        let trimmed = peer.trim().to_string();
        if trimmed.is_empty() || trimmed.len() > 2048 {
            continue;
        }
        if !is_supported_bootstrap_peer(&trimmed) {
            tracing::warn!("ignore unsupported bootstrap peer: {trimmed}");
            continue;
        }
        if !sanitized.iter().any(|existing| existing == &trimmed) {
            sanitized.push(trimmed);
        }
    }

    sanitized
}

fn merge_bootstrap_peers(primary: Vec<String>, secondary: Vec<String>) -> Vec<String> {
    let mut merged = Vec::new();

    for peer in primary.into_iter().chain(secondary) {
        if !merged.iter().any(|existing| existing == &peer) {
            merged.push(peer);
        }
    }

    merged
}

async fn fetch_public_bootstrap_peers() -> anyhow::Result<Vec<String>> {
    let client = match cloudflare::race_ipv4(
        &format!("{PUBLIC_BOOTSTRAP_PEERS_HOST}:443"),
        Duration::from_secs(2),
    )
    .await
    {
        Some(ip) => no_proxy_for_host(PUBLIC_BOOTSTRAP_PEERS_HOST, ip),
        None => no_proxy(),
    };

    let response = client
        .get(PUBLIC_BOOTSTRAP_PEERS_URL)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .context("fetch public bootstrap peers failed")?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "public bootstrap peers http status={status}, body={body}"
        ));
    }

    let peers: Vec<String> =
        serde_json::from_str(&body).context("public bootstrap peers: invalid json")?;
    let peers = merge_bootstrap_peers(fallback_bootstrap_peers(), sanitize_bootstrap_peers(peers));

    if peers.is_empty() {
        return Err(anyhow!("public bootstrap peers: empty list"));
    }

    Ok(peers)
}

async fn default_bootstrap_peers() -> Vec<String> {
    if let Ok(cache_guard) = BOOTSTRAP_PEERS_CACHE.lock() {
        if let Some(cache) = cache_guard.as_ref()
            && cache.fetched_at.elapsed() < BOOTSTRAP_PEERS_CACHE_TTL
        {
            return cache.peers.clone();
        }
    }

    let peers = match tokio::time::timeout(BOOTSTRAP_FETCH_TIMEOUT, fetch_public_bootstrap_peers())
        .await
    {
        Ok(Ok(peers)) => peers,
        Ok(Err(error)) => {
            tracing::warn!("public bootstrap peer source unavailable: {error:#}; using fallback");
            fallback_bootstrap_peers()
        }
        Err(_) => {
            tracing::warn!(
                timeout = ?BOOTSTRAP_FETCH_TIMEOUT,
                "public bootstrap peer source timed out; using fallback"
            );
            fallback_bootstrap_peers()
        }
    };
    if let Ok(mut cache_guard) = BOOTSTRAP_PEERS_CACHE.lock() {
        *cache_guard = Some(BootstrapPeersCache {
            fetched_at: Instant::now(),
            peers: peers.clone(),
        });
    }
    peers
}

fn build_embedded_easytier_config(
    network_name: String,
    network_secret: String,
    peers: Vec<String>,
    hostname: Option<String>,
    options: Option<EasyTierStartOptions>,
) -> anyhow::Result<(TomlConfigLoader, Option<String>, Option<String>)> {
    let network_name_for_policy = network_name.clone();
    let cfg = TomlConfigLoader::default();
    cfg.set_network_identity(NetworkIdentity::new(network_name.clone(), network_secret));
    cfg.set_hostname(hostname);
    cfg.set_listeners(vec![
        url::Url::parse("udp://0.0.0.0:0")?,
        url::Url::parse("tcp://0.0.0.0:0")?,
    ]);

    let mut flags = gen_default_flags();
    flags.bind_device = false;
    flags.no_tun = true;
    flags.use_smoltcp = false;
    flags.disable_p2p = false;
    flags.data_compress_algo = CompressionAlgoPb::Zstd.into();

    let mut ipv4: Option<cidr::Ipv4Inet> = None;
    let mut dhcp = true;

    if let Some(opts) = options.clone() {
        if let Some(v) = opts.disable_p2p {
            flags.disable_p2p = v;
        }
        if let Some(v) = opts.compression {
            let raw = v.trim().to_ascii_lowercase();
            if !raw.is_empty() {
                flags.data_compress_algo = match raw.as_str() {
                    "zstd" => CompressionAlgoPb::Zstd.into(),
                    "none" => CompressionAlgoPb::None.into(),
                    _ => return Err(anyhow!("invalid compression: {v} (supported: none, zstd)")),
                };
            }
        }
        if let Some(v) = opts.ipv4 {
            let raw = v.trim();
            if !raw.is_empty() {
                let cidr = if raw.contains('/') {
                    raw.to_string()
                } else {
                    format!("{raw}/24")
                };
                ipv4 = Some(
                    cidr::Ipv4Inet::from_str(&cidr)
                        .with_context(|| format!("invalid ipv4 cidr: {cidr}"))?,
                );
                dhcp = false;
            }
        }
    }

    let hostname_value = cfg.get_hostname();
    let host_port_from_hostname = paperconnect::parse_server_hostname(hostname_value.trim())
        .map(|hostname| hostname.server_port);

    let is_paperconnect_network = network_name_for_policy.starts_with("paper-connect-");
    let is_paperconnect_host = is_paperconnect_network && host_port_from_hostname.is_some();

    if is_paperconnect_network
        && (hostname_value.trim().starts_with("paper-connect-server-")
            || hostname_value.trim().starts_with("pcs-"))
        && !is_paperconnect_host
    {
        return Err(anyhow!(
            "invalid PaperConnect server hostname: {}",
            hostname_value.trim()
        ));
    }

    if ipv4.is_none() && is_paperconnect_network {
        if is_paperconnect_host {
            ipv4 = Some(cidr::Ipv4Inet::from_str(&format!(
                "{DEFAULT_PAPERCONNECT_VIP}/24"
            ))?);
            dhcp = false;
        }
    }

    cfg.set_flags(flags);

    let resolved_ipv4 = ipv4.as_ref().map(|inet| {
        let s = inet.to_string();
        s.split_once('/').map(|v| v.0.to_string()).unwrap_or(s)
    });

    cfg.set_dhcp(dhcp);
    cfg.set_ipv4(ipv4);

    let mut peer_cfgs = Vec::new();
    for p in peers.into_iter().filter(|p| !p.trim().is_empty()) {
        let uri = url::Url::parse(&p).with_context(|| format!("invalid peer url: {p}"))?;
        peer_cfgs.push(PeerConfig {
            uri,
            peer_public_key: None,
        });
    }
    cfg.set_peers(peer_cfgs);

    let resolved_hostname = cfg.get_hostname().trim().to_string();
    let resolved_hostname = if resolved_hostname.is_empty() {
        None
    } else {
        Some(resolved_hostname)
    };

    Ok((cfg, resolved_hostname, resolved_ipv4))
}





async fn patch_easytier_port_forward(
    action: ConfigPatchAction,
    protocol: SocketType,
    bind_addr: SocketAddr,
    destination_addr: SocketAddr,
) -> Result<(), String> {
    let instance_id = ONLINE_STATE
        .easytier_instance_id
        .lock()
        .unwrap()
        .ok_or_else(|| "EasyTier not running".to_string())?;
    let service = ONLINE_STATE
        .easytier_manager
        .get_instance_service(&instance_id)
        .ok_or_else(|| "EasyTier API service not available".to_string())?;
    let request = PatchConfigRequest {
        patch: Some(InstanceConfigPatch {
            port_forwards: vec![PortForwardPatch {
                action: action as i32,
                cfg: Some(PortForwardConfigPb {
                    bind_addr: Some(bind_addr.into()),
                    dst_addr: Some(destination_addr.into()),
                    socket_type: protocol as i32,
                }),
            }],
            ..Default::default()
        }),
        instance: None,
    };

    tokio::time::timeout(
        EASYTIER_API_TIMEOUT,
        service
            .get_config_service()
            .patch_config(BaseController::default(), request),
    )
    .await
    .map_err(|_| "EasyTier 端口转发配置超时".to_string())?
    .map_err(|error| format!("EasyTier 端口转发配置失败：{error}"))?;
    Ok(())
}

async fn add_easytier_port_forward(
    protocol: SocketType,
    bind_addr: SocketAddr,
    destination_addr: SocketAddr,
) -> Result<(), String> {
    patch_easytier_port_forward(
        ConfigPatchAction::Add,
        protocol,
        bind_addr,
        destination_addr,
    )
    .await
}

async fn remove_easytier_port_forward(
    protocol: SocketType,
    bind_addr: SocketAddr,
    destination_addr: SocketAddr,
) -> Result<(), String> {
    patch_easytier_port_forward(
        ConfigPatchAction::Remove,
        protocol,
        bind_addr,
        destination_addr,
    )
    .await
}

#[derive(Clone, Copy)]
struct EasyTierPortForward {
    protocol: SocketType,
    bind_addr: SocketAddr,
    destination_addr: SocketAddr,
}

impl EasyTierPortForward {
    async fn add(self) -> Result<(), String> {
        add_easytier_port_forward(self.protocol, self.bind_addr, self.destination_addr).await
    }

    async fn remove(self, description: &str) {
        if let Err(error) =
            remove_easytier_port_forward(self.protocol, self.bind_addr, self.destination_addr).await
        {
            tracing::debug!("清理 PaperConnect {description}失败：{error}");
        }
    }
}

struct PaperConnectControlEndpoint {
    host: String,
    port: u16,
    remote_addr: SocketAddr,
    forward: EasyTierPortForward,
}

async fn create_paperconnect_control_endpoint(
    host_addr: IpAddr,
    server_port: u16,
) -> Result<PaperConnectControlEndpoint, String> {
    let remote_addr = SocketAddr::new(host_addr, server_port);
    let local_port = paperconnect::pick_listen_port()?;
    let forward = EasyTierPortForward {
        protocol: SocketType::Tcp,
        bind_addr: SocketAddr::from(([0, 0, 0, 0], local_port)),
        destination_addr: remote_addr,
    };
    forward.add().await?;
    Ok(PaperConnectControlEndpoint {
        host: "127.0.0.1".to_string(),
        port: local_port,
        remote_addr,
        forward,
    })
}

async fn probe_paperconnect_control_endpoint(
    endpoint: &PaperConnectControlEndpoint,
    deadline: Instant,
) -> Result<paperconnect::ServerInfo, String> {
    loop {
        match paperconnect::ping(&endpoint.host, endpoint.port).await {
            Ok(server) => {
                tracing::info!(
                    remote = %endpoint.remote_addr,
                    local_host = %endpoint.host,
                    local_port = endpoint.port,
                    "PaperConnect 联机中心连接成功"
                );
                return Ok(server);
            }
            Err(error) => {
                tracing::debug!(
                    remote = %endpoint.remote_addr,
                    local_host = %endpoint.host,
                    local_port = endpoint.port,
                    "PaperConnect 联机中心探测失败：{error}"
                );
                if Instant::now() >= deadline {
                    return Err(format!(
                        "已发现房主节点 {}，但 PaperConnect 控制端口无响应：{error}",
                        endpoint.remote_addr
                    ));
                }
                tokio::time::sleep(PAPERCONNECT_PROBE_RETRY_INTERVAL).await;
            }
        }
    }
}

fn apply_paperconnect_announcement(
    server: &mut paperconnect::ServerInfo,
    announcement: paperconnect::ServerHostname,
) {
    if let Some(protocol) = announcement.protocol {
        server.protocol = protocol;
    }
    if let Some(game_port) = announcement.game_port {
        server.game_port = game_port;
    }
}

async fn configure_paperconnect_game_endpoint(
    mut server: paperconnect::ServerInfo,
    host_addr: IpAddr,
) -> Result<paperconnect::ServerInfo, String> {
    let local_game_port = paperconnect::pick_udp_port()?;
    EasyTierPortForward {
        protocol: SocketType::Udp,
        bind_addr: SocketAddr::from(([0, 0, 0, 0], local_game_port)),
        destination_addr: SocketAddr::new(host_addr, server.game_port),
    }
    .add()
    .await?;
    tracing::info!(
        local_port = local_game_port,
        remote = %SocketAddr::new(host_addr, server.game_port),
        "PaperConnect 游戏端口转发已建立"
    );
    server.game_host = "127.0.0.1".to_string();
    server.game_port = local_game_port;

    *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() = Some(EasyTierGameEndpoint {
        host: server.game_host.clone(),
        port: server.game_port,
    });
    Ok(server)
}





fn preferred_peer_connection(peer: &PeerInfo) -> Option<&PeerConnInfo> {
    let default_connection_id = peer.default_conn_id.as_ref().map(ToString::to_string);
    peer.conns
        .iter()
        .find(|connection| {
            !connection.is_closed
                && default_connection_id.as_deref() == Some(connection.conn_id.as_str())
        })
        .or_else(|| peer.conns.iter().find(|connection| !connection.is_closed))
}

fn start_easytier_event_monitor(instance_id: Uuid) {
    let Some(mut events) = ONLINE_STATE
        .easytier_manager
        .subscribe_instance_event(&instance_id)
    else {
        tracing::warn!(%instance_id, "无法订阅 EasyTier 实例事件");
        return;
    };

    let task = crate::tasks::runtime::spawn_io(async move {
        loop {
            match events.recv().await {
                Ok(::easytier::common::global_ctx::GlobalCtxEvent::ConnectorAbandoned(
                    url,
                    _error,
                    failed_attempts,
                )) => {
                    let is_current_instance = match ONLINE_STATE.easytier_instance_id.lock() {
                        Ok(current_instance) => current_instance.as_ref() == Some(&instance_id),
                        Err(_) => {
                            tracing::warn!("EasyTier 实例状态锁已损坏");
                            return;
                        }
                    };
                    if !is_current_instance {
                        tracing::debug!(
                            %instance_id,
                            %url,
                            "忽略已结束 EasyTier 会话的节点放弃事件"
                        );
                        continue;
                    }
                    match ONLINE_STATE.easytier_abandoned_connectors.lock() {
                        Ok(mut abandoned) => {
                            if !abandoned.iter().any(|connector| connector.url == url) {
                                abandoned.push(EasyTierAbandonedConnector {
                                    url: url.clone(),
                                    failed_attempts,
                                });
                            }
                        }
                        Err(_) => {
                            tracing::warn!("EasyTier 放弃节点状态锁已损坏");
                        }
                    }
                    tracing::warn!(
                        %url,
                        failed_attempts,
                        "EasyTier 节点连续连接失败，本次联机已放弃该节点"
                    );
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "EasyTier 节点事件接收滞后");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    if let Err(error) = task {
        tracing::warn!(%instance_id, "启动 EasyTier 节点事件监听失败：{error}");
    }
}

#[cfg(test)]
mod tests {
    use ::easytier::common::config::ConfigLoader as _;

    use super::{
        EasyTierStartOptions, apply_paperconnect_announcement, build_embedded_easytier_config,
        merge_bootstrap_peers, paperconnect,
        sanitize_bootstrap_peers,
    };

    #[test]
    fn easytier_hostname_game_endpoint_overrides_ping_metadata() {
        let mut server = paperconnect::ServerInfo {
            host: "127.0.0.1".to_string(),
            server_port: 22000,
            game_host: "127.0.0.1".to_string(),
            game_port: 25000,
            game_type: "MinecraftBedrock".to_string(),
            game_protocol_type: "UDP".to_string(),
            protocol: paperconnect::PaperConnectProtocol::Raknet,
        };
        let announcement = paperconnect::parse_server_hostname("pcs-22000-g-23000")
            .expect("应解析 GravityCone NetherNet 房主节点");

        apply_paperconnect_announcement(&mut server, announcement);

        assert_eq!(server.game_port, 23000);
        assert_eq!(
            server.protocol,
            paperconnect::PaperConnectProtocol::Nethernet
        );
    }

    /// Raw-UDP diagnostic: verifies the EasyTier UDP port forward passes
    /// RakNet unconnected ping/pong to the live host, independent of any
    /// RakNet client implementation.
    ///
    /// Run with:
    /// `cargo test --lib udp_forward_diagnostic -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires live PaperConnect room and network access"]
    async fn udp_forward_diagnostic() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();
        crate::tasks::runtime::initialize_app_runtime().expect("initialize application runtime");

        let room = paperconnect::room_from_code("P/NPR4-E6J4-DYAG-VZH2")
            .expect("room code should parse");
        super::easytier::start(super::EasyTierStartRequest {
            network_name: room.network_name,
            network_secret: room.network_secret,
            peers: Vec::new(),
            hostname: Some("bmcbl-client-diag".to_string()),
            player_name: "DiagGuest".to_string(),
            game_port: 7551,
            options: None,
        })
        .await
        .expect("easytier::start should succeed");

        let server = super::paperconnect::probe_server()
            .await
            .expect("probe should find the host");
        eprintln!(
            "[diag] game endpoint {}:{} protocol={:?}",
            server.game_host, server.game_port, server.protocol
        );

        let socket = tokio::net::UdpSocket::bind(("0.0.0.0", 0))
            .await
            .expect("bind diag socket");
        socket
            .connect((server.game_host.as_str(), server.game_port))
            .await
            .expect("connect diag socket");

        // RakNet unconnected ping (0x01 + time + magic + client guid).
        let mut ping = [0_u8; 33];
        ping[0] = 0x01;
        ping[1..9].copy_from_slice(&1_u64.to_be_bytes());
        ping[9..25].copy_from_slice(&[
            0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
            0x56, 0x78,
        ]);
        ping[25..33].copy_from_slice(&7_u64.to_be_bytes());

        let mut got_reply = false;
        let mut buffer = [0_u8; 2048];
        for attempt in 0..10 {
            socket.send(&ping).await.expect("send unconnected ping");
            match tokio::time::timeout(std::time::Duration::from_secs(1), socket.recv(&mut buffer))
                .await
            {
                Ok(Ok(length)) => {
                    eprintln!(
                        "[diag] attempt {attempt}: reply {} bytes, id=0x{:02x}",
                        length, buffer[0]
                    );
                    got_reply = true;
                    break;
                }
                Ok(Err(error)) => eprintln!("[diag] attempt {attempt}: recv error {error}"),
                Err(_) => eprintln!("[diag] attempt {attempt}: no reply within 1s"),
            }
        }

        // Manual RakNet open-connection handshake, mirroring what
        // raknet-tokio's client sends, to find where the exchange stalls.
        let magic: [u8; 16] = [
            0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34,
            0x56, 0x78,
        ];
        for request_size in [400_usize, 576, 1200] {
            let mut ocr1 = vec![0_u8; request_size];
            ocr1[0] = 0x05;
            ocr1[1..17].copy_from_slice(&magic);
            ocr1[17] = 11; // RakNet protocol version
            socket.send(&ocr1).await.expect("send OCR1");
            match tokio::time::timeout(std::time::Duration::from_secs(2), socket.recv(&mut buffer))
                .await
            {
                Ok(Ok(length)) => {
                    eprintln!(
                        "[diag] OCR1(size={request_size}): reply {length} bytes id=0x{:02x} raw={:02x?}",
                        buffer[0],
                        &buffer[..length.min(40)]
                    );
                    if buffer[0] == 0x06 && length >= 28 {
                        // reply1: id(1) magic(16) guid(8) has_cookie(1) [cookie(4)] mtu(2)
                        let has_cookie = buffer[25] != 0;
                        let (cookie, mtu_offset) = if has_cookie {
                            (Some(&buffer[26..30]), 30)
                        } else {
                            (None, 26)
                        };
                        let server_mtu =
                            u16::from_be_bytes([buffer[mtu_offset], buffer[mtu_offset + 1]]);
                        eprintln!("[diag] server mtu={server_mtu} cookie={:02x?}", cookie);
                        // OCR2: id + magic + [cookie + challenge flag] + addr(v4) + mtu + guid
                        let mut ocr2 = Vec::new();
                        ocr2.push(0x07);
                        ocr2.extend_from_slice(&magic);
                        if let Some(cookie) = cookie {
                            ocr2.extend_from_slice(cookie);
                            ocr2.push(0); // no security challenge
                        }
                        ocr2.push(4); // address version
                        // RakNet encodes IPv4 bytes complemented.
                        ocr2.extend_from_slice(&[!127_u8, !0, !0, !1]);
                        ocr2.extend_from_slice(&server.game_port.to_be_bytes());
                        ocr2.extend_from_slice(&server_mtu.to_be_bytes());
                        // go-raknet requires a vanilla-style negative i64 GUID.
                        ocr2.extend_from_slice(&(-7_i64).to_be_bytes());
                        socket.send(&ocr2).await.expect("send OCR2");
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            socket.recv(&mut buffer),
                        )
                        .await
                        {
                            Ok(Ok(length)) => eprintln!(
                                "[diag] OCR2: reply {length} bytes id=0x{:02x} raw={:02x?}",
                                buffer[0],
                                &buffer[..length.min(48)]
                            ),
                            Ok(Err(error)) => eprintln!("[diag] OCR2 recv error: {error}"),
                            Err(_) => eprintln!("[diag] OCR2: no reply within 2s"),
                        }
                        break;
                    }
                }
                Ok(Err(error)) => eprintln!("[diag] OCR1(size={request_size}) recv error: {error}"),
                Err(_) => eprintln!("[diag] OCR1(size={request_size}): no reply within 2s"),
            }
        }

        let stop = super::easytier::stop().await;
        eprintln!("[diag] stop result: {stop:?}");
        assert!(got_reply, "UDP forward should pass RakNet ping/pong");
    }

    /// Manual reproduction harness for the "players cannot join" bug.
    ///
    /// Run with:
    /// `cargo test --lib join_room_reproduction -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires live PaperConnect room and network access"]
    async fn join_room_reproduction() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();
        crate::tasks::runtime::initialize_app_runtime().expect("initialize application runtime");

        let room = paperconnect::room_from_code("P/NPR4-E6J4-DYAG-VZH2")
            .expect("room code should parse");
        eprintln!("[repro] network_name={}", room.network_name);

        super::easytier::start(super::EasyTierStartRequest {
            network_name: room.network_name.clone(),
            network_secret: room.network_secret.clone(),
            peers: Vec::new(),
            hostname: Some("bmcbl-client-repro".to_string()),
            player_name: "ReproGuest".to_string(),
            game_port: 7551,
            options: Some(EasyTierStartOptions {
                disable_p2p: Some(false),
                compression: Some("zstd".to_string()),
                ipv4: None,
            }),
        })
        .await
        .expect("easytier::start should succeed");

        // Watch route table while discovery runs so we can see what the guest observes.
        let watcher = tokio::spawn(async {
            for i in 0..12 {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                match super::easytier::peers().await {
                    Ok(peers) => {
                        eprintln!("[repro] t+{}s routes:", (i + 1) * 3);
                        for peer in &peers {
                            eprintln!(
                                "[repro]   hostname={} ipv4={:?} kind={:?} proto={:?} endpoint={:?}",
                                peer.hostname,
                                peer.ipv4,
                                peer.connection_kind,
                                peer.protocol,
                                peer.remote_endpoint
                            );
                        }
                    }
                    Err(error) => eprintln!("[repro] peers error: {error}"),
                }
            }
        });

        let probe = super::paperconnect::probe_server().await;
        eprintln!("[repro] probe result: {probe:?}");
        watcher.abort();

        let joined = match &probe {
            Ok(server) => {
                // Full production guest path: c:player heartbeat + RakNet tunnel
                // connection to the host through the EasyTier port forward.
                let join =
                    super::paperconnect_guest::start(server.clone(), "ReproGuest".to_string())
                        .await;
                eprintln!("[repro] start_client result: {join:?}");
                eprintln!("[repro] players: {:?}", super::paperconnect::players());
                join
            }
            Err(error) => Err(error.clone()),
        };

        let stop = super::easytier::stop().await;
        eprintln!("[repro] stop result: {stop:?}");

        probe.expect("guest should discover the PaperConnect host");
        joined.expect("guest transport should connect to the host tunnel");
    }

    #[test]
    fn paperconnect_room_from_code_accepts_published_format_and_rejects_malformed_code() {
        let room = paperconnect::room_from_code("P/YNZE-U61D-2206-HXRG")
            .expect("documented PaperConnect room code should parse");
        assert_eq!(room.network_name, "paper-connect-YNZE-U61D");
        assert_eq!(room.network_secret, "2206-HXRG");

        let invalid = paperconnect::room_from_code("P/YNZE-U61D-2206-SSSS+");
        assert!(invalid.is_err());
        assert!(paperconnect::room_from_code("P/ĀA-ĀAAA-AAAA-AAAA").is_err());
    }

    #[test]
    fn bootstrap_sources_are_combined_without_duplicates() {
        let merged = merge_bootstrap_peers(
            vec!["tcp://fallback.example:54321".to_string()],
            vec![
                "tcp://fallback.example:54321".to_string(),
                "udp://public.example:54321".to_string(),
            ],
        );

        assert_eq!(
            merged,
            vec![
                "tcp://fallback.example:54321".to_string(),
                "udp://public.example:54321".to_string(),
            ]
        );
    }

    #[test]
    fn bootstrap_peer_sanitization_keeps_supported_transports_only() {
        let peers = sanitize_bootstrap_peers(vec![
            " tcp://node.example:54321 ".to_string(),
            "udp://node.example:54321".to_string(),
            "ws://node.example/easytier".to_string(),
            "wss://center.node.1tmc.top".to_string(),
            "https://node.example/peers".to_string(),
            "".to_string(),
        ]);

        assert_eq!(
            peers,
            vec![
                "tcp://node.example:54321".to_string(),
                "udp://node.example:54321".to_string(),
                "ws://node.example/easytier".to_string(),
                "wss://center.node.1tmc.top".to_string(),
            ]
        );
    }

    #[test]
    fn gravitycone_center_is_a_default_bootstrap_peer() {
        assert!(
            super::fallback_bootstrap_peers()
                .iter()
                .any(|peer| peer == "wss://center.node.1tmc.top")
        );
    }

    #[test]
    fn paperconnect_config_is_always_no_tun() {
        let options = EasyTierStartOptions {
            disable_p2p: None,
            compression: None,
            ipv4: None,
        };
        let (config, _, _) = build_embedded_easytier_config(
            "paper-connect-TEST-ROOM".to_string(),
            "TEST-KEY".to_string(),
            vec!["tcp://public.example:54321".to_string()],
            Some("paper-connect-server-54321".to_string()),
            Some(options),
        )
        .expect("PaperConnect no-TUN config should be valid");

        assert!(config.get_flags().no_tun);
        assert_eq!(
            config.get_ipv4().map(|value| value.to_string()),
            Some("10.144.144.1/24".to_string())
        );
        assert!(!config.get_dhcp());
    }
}
