use anyhow::{Context as _, anyhow};
use easytier::common::config::{
    ConfigFileControl, ConfigLoader as _, NetworkIdentity, PeerConfig, TomlConfigLoader,
    gen_default_flags,
};
use easytier::instance_manager::NetworkInstanceManager;
use easytier::proto::api::config::{
    ConfigPatchAction, ConfigRpc as _, InstanceConfigPatch, PatchConfigRequest, PortForwardPatch,
};
use easytier::proto::api::instance::{
    ListPeerRequest, ListRouteRequest, PeerConnInfo, PeerInfo, list_peer_route_pair,
};
use easytier::proto::common::{CompressionAlgoPb, PortForwardConfigPb, SocketType};
use easytier::proto::rpc_types::controller::BaseController;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::Instant;
use uuid::Uuid;

mod paperconnect;
mod paperconnect_discovery;
mod paperconnect_guest;
mod paperconnect_transport;
mod paperconnect_tunnel;

pub use paperconnect::PaperConnectPlayer;

use crate::http::proxy::{build_no_proxy_client_with_resolve, get_no_proxy_client};
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperConnectRoom {
    pub room_code: String,
    pub network_name: String,
    pub network_secret: String,
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
    network_name: String,
    network_secret: String,
    peers: Vec<String>,
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

fn now_ms() -> i64 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    d.as_millis() as i64
}

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
        Some(ip) => build_no_proxy_client_with_resolve(PUBLIC_BOOTSTRAP_PEERS_HOST, ip),
        None => get_no_proxy_client(),
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

pub fn paperconnect_pick_listen_port() -> Result<u16, String> {
    for _ in 0..12 {
        let listener = StdTcpListener::bind(("0.0.0.0", 0)).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener);
        if (1025..=65535).contains(&port) {
            return Ok(port);
        }
    }
    Err("failed to pick an available port".to_string())
}

pub fn paperconnect_pick_udp_port() -> Result<u16, String> {
    for _ in 0..12 {
        let socket = StdUdpSocket::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
        let port = socket.local_addr().map_err(|e| e.to_string())?.port();
        drop(socket);
        if (1025..=65535).contains(&port) {
            return Ok(port);
        }
    }
    Err("failed to pick an available UDP port".to_string())
}

fn alphabet34() -> &'static [u8; 34] {
    b"0123456789ABCDEFGHJKLMNPQRSTUVWXYZ"
}

fn char_to_digit34(c: char) -> Option<u32> {
    let uc = c.to_ascii_uppercase();
    match uc {
        '0'..='9' => Some((uc as u8 - b'0') as u32),
        'A'..='H' => Some(10 + (uc as u8 - b'A') as u32),
        'J'..='N' => Some(18 + (uc as u8 - b'J') as u32),
        'P'..='Z' => Some(23 + (uc as u8 - b'P') as u32),
        _ => None,
    }
}

fn group_to_value_le_base34(group8: &str) -> anyhow::Result<u128> {
    let s = group8.trim().to_ascii_uppercase().replace('-', "");
    if s.len() != 8 {
        return Err(anyhow!("group must be 8 chars (without '-')"));
    }
    let mut value: u128 = 0;
    let mut place: u128 = 1;
    for ch in s.chars() {
        let digit =
            char_to_digit34(ch).ok_or_else(|| anyhow!("invalid char in group: {ch}"))? as u128;
        value = value
            .checked_add(digit * place)
            .ok_or_else(|| anyhow!("group value overflow"))?;
        place = place
            .checked_mul(34)
            .ok_or_else(|| anyhow!("group value overflow"))?;
    }
    Ok(value)
}

fn format_group8(s: &str) -> anyhow::Result<String> {
    let raw = s.trim().to_ascii_uppercase().replace('-', "");
    if raw.len() != 8 {
        return Err(anyhow!("group must be 8 chars"));
    }
    Ok(format!("{}-{}", &raw[0..4], &raw[4..8]))
}

fn validate_group(group: &str) -> anyhow::Result<String> {
    let formatted = format_group8(group)?;
    let val = group_to_value_le_base34(&formatted)?;
    if val % 7 != 0 {
        return Err(anyhow!(
            "group check failed: {formatted} (little-endian base34 value mod 7 = {})",
            (val % 7)
        ));
    }
    Ok(formatted)
}

fn validate_group_chars_only(group: &str) -> anyhow::Result<String> {
    let formatted = format_group8(group)?;
    let _ = group_to_value_le_base34(&formatted)?;
    Ok(formatted)
}

fn random_group8_div7() -> String {
    let alpha = alphabet34();
    loop {
        let mut raw = String::with_capacity(8);
        let mut bytes = Vec::from(Uuid::new_v4().as_bytes());
        bytes.extend_from_slice(Uuid::new_v4().as_bytes());

        for i in 0..8 {
            let idx = (bytes[i] as usize) % 34;
            raw.push(alpha[idx] as char);
        }
        if let Ok(formatted) = validate_group(&raw) {
            return formatted;
        }
    }
}

pub async fn paperconnect_generate_room() -> Result<PaperConnectRoom, String> {
    let n = random_group8_div7();
    let secret = random_group8_div7();
    let room_code = format!("P/{n}-{secret}");
    Ok(PaperConnectRoom {
        room_code: room_code.clone(),
        network_name: format!("paper-connect-{n}"),
        network_secret: secret,
    })
}

pub async fn paperconnect_parse_room_code(room_code: String) -> Result<PaperConnectRoom, String> {
    let raw = room_code.trim();
    let raw = raw
        .strip_prefix("P/")
        .ok_or_else(|| "roomCode must start with P/".to_string())?;
    let parts: Vec<&str> = raw.split('-').collect();
    if parts.len() != 4 {
        return Err("roomCode must be like P/NNNN-NNNN-SSSS-SSSS".to_string());
    }
    // The published PaperConnect example does not satisfy its own checksum
    // rule, so parsers must accept valid-format codes for compatibility.
    let n = validate_group_chars_only(&format!("{}{}", parts[0], parts[1]))
        .map_err(|e| format!("invalid roomCode N group: {e}"))?;
    let secret = validate_group_chars_only(&format!("{}{}", parts[2], parts[3]))
        .map_err(|e| format!("invalid roomCode S group: {e}"))?;

    let normalized = format!("P/{n}-{secret}");
    Ok(PaperConnectRoom {
        room_code: normalized,
        network_name: format!("paper-connect-{n}"),
        network_secret: secret,
    })
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
    let mut host_port_from_hostname: Option<u16> = None;

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
    host_port_from_hostname = paperconnect::parse_server_hostname(hostname_value.trim())
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

pub async fn easytier_start(request: EasyTierStartRequest) -> Result<(), String> {
    let EasyTierStartRequest {
        network_name,
        network_secret,
        peers,
        mut hostname,
        player_name,
        mut game_port,
        options,
    } = request;
    if !(1025..=65535).contains(&game_port) {
        return Err(format!("invalid PaperConnect game port: {game_port}"));
    }
    if player_name.trim().is_empty() {
        return Err("PaperConnect player name is empty".to_string());
    }
    {
        let instance_id = ONLINE_STATE.easytier_instance_id.lock().unwrap();
        if instance_id.is_some() {
            return Err("EasyTier already running".to_string());
        }
        if ONLINE_STATE
            .easytier_cleanup_in_progress
            .load(Ordering::Acquire)
        {
            return Err("上一条联机连接仍在清理，请稍候再试".to_string());
        }
    }
    ONLINE_STATE
        .easytier_abandoned_connectors
        .lock()
        .map_err(|_| "EasyTier 放弃节点状态锁已损坏".to_string())?
        .clear();

    let peers = if peers.iter().any(|p| !p.trim().is_empty()) {
        let sanitized = sanitize_bootstrap_peers(peers);
        if sanitized.is_empty() {
            tracing::warn!("configured bootstrap peers are invalid; using fallback peers");
            default_bootstrap_peers().await
        } else {
            sanitized
        }
    } else {
        default_bootstrap_peers().await
    };

    let host_server_port = hostname
        .as_deref()
        .and_then(paperconnect::server_port_from_hostname);
    let mut host_protocol = None;
    if let Some(server_port) = host_server_port {
        let transport = paperconnect_transport::start_host().await?;
        game_port = transport.game_port;
        hostname = Some(paperconnect::build_server_hostname(
            server_port,
            transport.protocol,
            game_port,
        )?);
        host_protocol = Some(transport.protocol);
    }

    {
        let mut id = ONLINE_STATE.easytier_instance_id.lock().unwrap();
        if id.is_some() {
            paperconnect_transport::stop_all();
            return Err("EasyTier already running".to_string());
        }
        if ONLINE_STATE
            .easytier_cleanup_in_progress
            .load(Ordering::Acquire)
        {
            paperconnect_transport::stop_all();
            return Err("上一条联机连接仍在清理，请稍候再试".to_string());
        }
        let (cfg, resolved_hostname, resolved_ipv4) = match build_embedded_easytier_config(
            network_name.clone(),
            network_secret.clone(),
            peers.clone(),
            hostname.clone(),
            options.clone(),
        ) {
            Ok(config) => config,
            Err(error) => {
                paperconnect_transport::stop_all();
                return Err(error.to_string());
            }
        };

        *ONLINE_STATE.easytier_last_start.lock().unwrap() = Some(EasyTierLastStart {
            network_name: network_name.clone(),
            network_secret: network_secret.clone(),
            peers: peers.clone(),
            hostname: hostname.clone(),
            resolved_hostname,
            resolved_ipv4,
            game_port,
            options: options.clone(),
        });

        let is_host = hostname
            .as_deref()
            .and_then(paperconnect::server_port_from_hostname)
            .is_some();
        *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() =
            is_host.then(|| EasyTierGameEndpoint {
                host: "127.0.0.1".to_string(),
                port: game_port,
            });

        let instance_id = match ONLINE_STATE.easytier_manager.run_network_instance(
            cfg,
            true,
            ConfigFileControl::STATIC_CONFIG,
        ) {
            Ok(instance_id) => instance_id,
            Err(error) => {
                paperconnect_transport::stop_all();
                return Err(format!("start embedded EasyTier failed: {error}"));
            }
        };
        *id = Some(instance_id);
    }

    let instance_id = *ONLINE_STATE
        .easytier_instance_id
        .lock()
        .unwrap()
        .as_ref()
        .unwrap();
    start_easytier_event_monitor(instance_id);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let has_api = ONLINE_STATE
            .easytier_manager
            .get_instance_service(&instance_id)
            .is_some();
        if has_api {
            break;
        }

        let mut is_running = false;
        let mut last_err: Option<String> = None;
        for i in ONLINE_STATE.easytier_manager.iter() {
            if *i.key() != instance_id {
                continue;
            }
            is_running = i.value().is_easytier_running();
            last_err = i.value().get_latest_error_msg();
            break;
        }

        if !is_running {
            paperconnect_transport::stop_all();
            *ONLINE_STATE.easytier_instance_id.lock().unwrap() = None;
            *ONLINE_STATE.easytier_last_start.lock().unwrap() = None;
            *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() = None;
            let _ = ONLINE_STATE
                .easytier_manager
                .delete_network_instance(vec![instance_id]);
            return Err(format!(
                "embedded EasyTier stopped during startup: {}",
                last_err.unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if let Some(server_port) = hostname
        .as_deref()
        .and_then(paperconnect::server_port_from_hostname)
    {
        if let Err(error) = paperconnect::start_server(
            server_port,
            game_port,
            host_protocol.unwrap_or_default(),
            player_name.clone(),
        )
        .await
        {
            easytier_stop().await?;
            return Err(error);
        }
        if let Err(error) = paperconnect::ping("127.0.0.1", server_port).await {
            if let Err(stop_error) = easytier_stop().await {
                tracing::warn!("PaperConnect 联机中心自检失败后停止 EasyTier 失败：{stop_error}");
            }
            return Err(format!("PaperConnect 联机中心本机自检失败：{error}"));
        }
        if let Err(error) =
            paperconnect::start_client("127.0.0.1".to_string(), server_port, player_name).await
        {
            if let Err(stop_error) = easytier_stop().await {
                tracing::warn!("房主 c:player 首包失败后停止 EasyTier 失败：{stop_error}");
            }
            return Err(format!("PaperConnect 房主玩家心跳失败：{error}"));
        }
        tracing::info!(
            server_port,
            game_port,
            "PaperConnect 联机中心已启动并通过本机自检"
        );
    }

    Ok(())
}

pub async fn easytier_stop() -> Result<(), String> {
    paperconnect_transport::stop_all();
    paperconnect::stop_server();
    paperconnect::stop_client();
    *ONLINE_STATE
        .paperconnect_guest_transport
        .lock()
        .map_err(|_| "PaperConnect 成员传输状态锁已损坏".to_string())? = None;
    paperconnect::clear_players();
    let instance_id = {
        let mut instance_id = ONLINE_STATE.easytier_instance_id.lock().unwrap();
        let instance_id = instance_id.take();
        if instance_id.is_some() {
            ONLINE_STATE
                .easytier_cleanup_in_progress
                .store(true, Ordering::Release);
        }
        instance_id
    };
    *ONLINE_STATE.easytier_last_start.lock().unwrap() = None;
    *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() = None;
    if let Some(id) = instance_id {
        let manager = ONLINE_STATE.easytier_manager.clone();
        let cleanup_in_progress = ONLINE_STATE.easytier_cleanup_in_progress.clone();
        let mut cleanup = Box::pin(crate::tasks::runtime::run_io_blocking(move || {
            manager.delete_network_instance(vec![id])
        }));
        match tokio::time::timeout(EASYTIER_STOP_TIMEOUT, cleanup.as_mut()).await {
            Ok(joined) => {
                cleanup_in_progress.store(false, Ordering::Release);
                match joined {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => tracing::warn!(
                        instance_id = %id,
                        "EasyTier 实例清理失败，但联机会话已关闭：{error}"
                    ),
                    Err(error) => tracing::warn!(
                        instance_id = %id,
                        "EasyTier 实例清理任务异常，但联机会话已关闭：{error}"
                    ),
                }
            }
            Err(_) => {
                tracing::warn!(
                    instance_id = %id,
                    timeout = ?EASYTIER_STOP_TIMEOUT,
                    "停止 EasyTier 超时，连接状态已关闭，后台继续清理实例"
                );
                let cleanup_flag = cleanup_in_progress.clone();
                if let Err(error) = crate::tasks::runtime::spawn_io(async move {
                    match cleanup.await {
                        Ok(Ok(_)) => tracing::info!(instance_id = %id, "EasyTier 后台清理完成"),
                        Ok(Err(error)) => tracing::warn!(
                            instance_id = %id,
                            "EasyTier 后台清理失败：{error}"
                        ),
                        Err(error) => tracing::warn!(
                            instance_id = %id,
                            "EasyTier 后台清理任务异常：{error}"
                        ),
                    }
                    cleanup_flag.store(false, Ordering::Release);
                }) {
                    cleanup_in_progress.store(false, Ordering::Release);
                    tracing::warn!(
                        instance_id = %id,
                        "无法调度 EasyTier 后台清理：{error}"
                    );
                }
            }
        }
    }
    Ok(())
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
    let local_port = paperconnect_pick_listen_port()?;
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
    let local_game_port = paperconnect_pick_udp_port()?;
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

pub async fn easytier_embedded_status() -> Result<Option<EasyTierEmbeddedStatus>, String> {
    let id = match ONLINE_STATE.easytier_instance_id.lock().unwrap().as_ref() {
        Some(v) => *v,
        None => return Ok(None),
    };

    let svc = match ONLINE_STATE.easytier_manager.get_instance_service(&id) {
        Some(v) => v,
        None => return Ok(None),
    };

    let resp = tokio::time::timeout(
        EASYTIER_API_TIMEOUT,
        svc.get_peer_manage_service()
            .list_route(BaseController::default(), ListRouteRequest::default()),
    )
    .await
    .map_err(|_| "EasyTier 路由查询超时".to_string())?
    .map_err(|e| format!("list_route failed: {e}"))?;

    let inst_id = id.to_string();
    let mut hostname = String::new();
    let mut ipv4: Option<String> = None;
    for r in resp.routes {
        if r.inst_id != inst_id {
            continue;
        }
        hostname = r.hostname;
        ipv4 = r.ipv4_addr.map(|inet| {
            let s = inet.to_string();
            s.split_once('/').map(|v| v.0.to_string()).unwrap_or(s)
        });
        break;
    }

    let last_start = ONLINE_STATE.easytier_last_start.lock().unwrap().clone();
    if hostname.trim().is_empty() || ipv4.as_deref().unwrap_or_default().trim().is_empty() {
        if let Some(last) = last_start.as_ref() {
            if hostname.trim().is_empty() {
                if let Some(hn) = last
                    .resolved_hostname
                    .clone()
                    .or_else(|| last.hostname.clone())
                {
                    hostname = hn;
                }
            }
            if ipv4.as_deref().unwrap_or_default().trim().is_empty() {
                if let Some(v) = last.resolved_ipv4.clone() {
                    if !v.trim().is_empty() {
                        ipv4 = Some(v);
                    }
                } else if let Some(opts) = last.options.clone() {
                    if let Some(v) = opts.ipv4 {
                        let raw = v.trim();
                        if !raw.is_empty() {
                            let ip = raw.split_once('/').map(|v| v.0).unwrap_or(raw);
                            ipv4 = Some(ip.to_string());
                        }
                    }
                }
                if ipv4.is_none() {
                    let hn = hostname.trim();
                    if paperconnect::server_port_from_hostname(hn).is_some() {
                        ipv4 = Some(DEFAULT_PAPERCONNECT_VIP.to_string());
                    }
                }
            }
        }
    }

    let game_endpoint = ONLINE_STATE.easytier_game_endpoint.lock().unwrap().clone();
    let game_port = game_endpoint
        .as_ref()
        .map(|endpoint| endpoint.port)
        .or_else(|| last_start.as_ref().map(|value| value.game_port));
    let game_host = game_endpoint.map(|endpoint| endpoint.host);

    Ok(Some(EasyTierEmbeddedStatus {
        instance_id: inst_id,
        hostname,
        ipv4,
        game_host,
        game_port,
    }))
}

pub async fn easytier_embedded_peers() -> Result<Vec<EasyTierPeer>, String> {
    let id = ONLINE_STATE
        .easytier_instance_id
        .lock()
        .unwrap()
        .ok_or_else(|| "EasyTier not running".to_string())?;

    let svc = ONLINE_STATE
        .easytier_manager
        .get_instance_service(&id)
        .ok_or_else(|| "EasyTier API service not available".to_string())?;

    let (route_result, peer_result) = tokio::join!(
        tokio::time::timeout(
            EASYTIER_API_TIMEOUT,
            svc.get_peer_manage_service()
                .list_route(BaseController::default(), ListRouteRequest::default()),
        ),
        tokio::time::timeout(
            EASYTIER_API_TIMEOUT,
            svc.get_peer_manage_service()
                .list_peer(BaseController::default(), ListPeerRequest::default()),
        ),
    );
    let route_response = route_result
        .map_err(|_| "EasyTier 路由查询超时".to_string())?
        .map_err(|error| format!("EasyTier 路由查询失败：{error}"))?;
    let peer_infos = match peer_result {
        Ok(Ok(response)) => response.peer_infos,
        Ok(Err(error)) => {
            tracing::debug!("EasyTier 连接详情查询失败，保留路由节点：{error}");
            Vec::new()
        }
        Err(_) => {
            tracing::debug!("EasyTier 连接详情查询超时，保留路由节点");
            Vec::new()
        }
    };

    let routes = route_response.routes;
    let pairs = list_peer_route_pair(peer_infos.clone(), routes.clone());
    let instance_id = id.to_string();
    let mut peers = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let Some(route) = pair.route else {
            continue;
        };
        let ipv4 = route.ipv4_addr.map(|inet| {
            let s = inet.to_string();
            s.split_once('/').map(|v| v.0.to_string()).unwrap_or(s)
        });

        let mut hostname = route.hostname.clone();
        if hostname.trim().is_empty() {
            let route_instance_id = route.inst_id.trim();
            hostname = if route_instance_id.is_empty() {
                "node-unknown".to_string()
            } else {
                format!("node-{route_instance_id}")
            };
        }
        let connection_kind = if route.inst_id == instance_id {
            EasyTierConnectionKind::Local
        } else if route.cost == 1 {
            EasyTierConnectionKind::Direct
        } else if route.cost > 1 {
            EasyTierConnectionKind::Relayed
        } else {
            EasyTierConnectionKind::Unknown
        };
        let next_hop_peer_id = route
            .next_hop_peer_id_latency_first
            .unwrap_or(route.next_hop_peer_id);
        let connection_peer = match connection_kind {
            EasyTierConnectionKind::Direct => pair.peer.as_ref(),
            EasyTierConnectionKind::Relayed => peer_infos
                .iter()
                .find(|peer| peer.peer_id == next_hop_peer_id),
            EasyTierConnectionKind::Local | EasyTierConnectionKind::Unknown => None,
        };
        let connection = connection_peer.and_then(preferred_peer_connection);
        let protocol = connection
            .and_then(|connection| connection.tunnel.as_ref())
            .map(|tunnel| tunnel.tunnel_type.to_ascii_uppercase())
            .filter(|protocol| !protocol.is_empty());
        let remote_endpoint = connection
            .and_then(|connection| connection.tunnel.as_ref())
            .and_then(|tunnel| tunnel.remote_addr.as_ref())
            .map(ToString::to_string)
            .filter(|endpoint| !endpoint.is_empty());
        let latency_ms = match connection_kind {
            EasyTierConnectionKind::Direct => connection
                .and_then(|connection| connection.stats.as_ref())
                .map(|stats| stats.latency_us.div_ceil(1_000)),
            EasyTierConnectionKind::Relayed => route
                .path_latency_latency_first
                .filter(|latency| *latency > 0)
                .or_else(|| (route.path_latency > 0).then_some(route.path_latency))
                .map(|latency| latency as u64),
            EasyTierConnectionKind::Local | EasyTierConnectionKind::Unknown => None,
        };
        let via_hostname = (connection_kind == EasyTierConnectionKind::Relayed)
            .then(|| {
                routes
                    .iter()
                    .find(|candidate| candidate.peer_id == next_hop_peer_id)
                    .map(|candidate| candidate.hostname.trim().to_string())
            })
            .flatten()
            .filter(|hostname| !hostname.is_empty());
        peers.push(EasyTierPeer {
            ipv4,
            hostname,
            connection_kind,
            protocol,
            remote_endpoint,
            latency_ms,
            via_hostname,
        });
    }
    Ok(peers)
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

pub async fn paperconnect_probe_server() -> Result<paperconnect::ServerInfo, String> {
    let deadline = Instant::now() + PAPERCONNECT_DISCOVERY_TIMEOUT;
    let mut last_probe_error = None;
    loop {
        let peers = match easytier_embedded_peers().await {
            Ok(peers) => peers,
            Err(error) => {
                last_probe_error = Some(format!("读取 EasyTier 节点失败：{error}"));
                Vec::new()
            }
        };
        for peer in peers {
            let Some(announcement) = paperconnect::parse_server_hostname(&peer.hostname) else {
                continue;
            };
            let server_port = announcement.server_port;
            let Some(host) = peer.ipv4 else {
                last_probe_error = Some("已发现房主节点，但节点没有虚拟 IP".to_string());
                continue;
            };
            let Ok(host_addr) = host.parse::<IpAddr>() else {
                last_probe_error = Some(format!("房主节点返回了无效虚拟 IP：{host}"));
                continue;
            };
            let control = match create_paperconnect_control_endpoint(host_addr, server_port).await {
                Ok(control) => control,
                Err(error) => {
                    last_probe_error = Some(format!("创建 PaperConnect 控制端口转发失败：{error}"));
                    continue;
                }
            };
            match probe_paperconnect_control_endpoint(&control, deadline).await {
                Ok(mut server) => {
                    apply_paperconnect_announcement(&mut server, announcement);
                    let remote_game_port = server.game_port;
                    server.host = control.host.clone();
                    server.server_port = control.port;
                    match configure_paperconnect_game_endpoint(server, host_addr).await {
                        Ok(server) => {
                            tracing::info!(
                                hostname = %peer.hostname,
                                host = %host_addr,
                                control_port = server_port,
                                remote_game_port,
                                local_proxy_port = server.game_port,
                                "PaperConnect 成员步骤 2/6：房主 g 端口已解析并通过 EasyTier 连通"
                            );
                            return Ok(server);
                        }
                        Err(error) => {
                            last_probe_error =
                                Some(format!("创建 PaperConnect 游戏端口转发失败：{error}"));
                        }
                    }
                }
                Err(error) => last_probe_error = Some(error),
            }
            control.forward.remove("控制端口转发").await;
        }
        if Instant::now() >= deadline {
            return Err(last_probe_error.unwrap_or_else(|| {
                "已连接 EasyTier，但未发现房主的 PaperConnect 联机中心节点".to_string()
            }));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub async fn paperconnect_start_client(
    server: paperconnect::ServerInfo,
    player_name: String,
) -> Result<PaperConnectClientState, String> {
    paperconnect::start_client(server.host.clone(), server.server_port, player_name.clone())
        .await?;
    *ONLINE_STATE
        .paperconnect_guest_transport
        .lock()
        .map_err(|_| "PaperConnect 成员传输状态锁已损坏".to_string())? =
        Some((server.clone(), player_name.clone()));
    match paperconnect_transport::start_guest(&server, &player_name).await {
        Ok(state) => Ok(state),
        Err(error) => {
            if let Ok(mut transport) = ONLINE_STATE.paperconnect_guest_transport.lock() {
                *transport = None;
            } else {
                tracing::warn!("PaperConnect 成员传输状态锁已损坏");
            }
            paperconnect::stop_client();
            Err(error)
        }
    }
}

pub async fn paperconnect_retry_guest_transport() -> Result<PaperConnectClientState, String> {
    let (server, player_name) = ONLINE_STATE
        .paperconnect_guest_transport
        .lock()
        .map_err(|_| "PaperConnect 成员传输状态锁已损坏".to_string())?
        .clone()
        .ok_or_else(|| "当前房间没有可重新启动的成员游戏代理".to_string())?;

    tracing::info!(
        proxy_port = server.game_port,
        "PaperConnect 成员重新检测 UDP 7551 并启动本机模拟代理"
    );
    paperconnect_transport::start_guest(&server, &player_name).await
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
                Ok(easytier::common::global_ctx::GlobalCtxEvent::ConnectorAbandoned(
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

pub fn easytier_take_abandoned_connectors() -> Vec<EasyTierAbandonedConnector> {
    match ONLINE_STATE.easytier_abandoned_connectors.lock() {
        Ok(mut abandoned) => std::mem::take(&mut *abandoned),
        Err(_) => {
            tracing::warn!("EasyTier 放弃节点状态锁已损坏");
            Vec::new()
        }
    }
}

pub fn paperconnect_players() -> Vec<PaperConnectPlayer> {
    paperconnect::players()
}

pub async fn online_debug_snapshot() -> serde_json::Value {
    serde_json::json!({
        "ts": now_ms(),
        "running": ONLINE_STATE.easytier_instance_id.lock().unwrap().is_some(),
    })
}

#[cfg(test)]
mod tests {
    use easytier::common::config::ConfigLoader as _;

    use super::{
        EasyTierStartOptions, apply_paperconnect_announcement, build_embedded_easytier_config,
        merge_bootstrap_peers, paperconnect, paperconnect_parse_room_code,
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
            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
            .with_test_writer()
            .try_init();
        crate::tasks::runtime::initialize_app_runtime().expect("initialize application runtime");

        let room = paperconnect_parse_room_code("P/NPR4-E6J4-DYAG-VZH2".to_string())
            .await
            .expect("room code should parse");
        super::easytier_start(super::EasyTierStartRequest {
            network_name: room.network_name,
            network_secret: room.network_secret,
            peers: Vec::new(),
            hostname: Some("bmcbl-client-diag".to_string()),
            player_name: "DiagGuest".to_string(),
            game_port: 7551,
            options: None,
        })
        .await
        .expect("easytier_start should succeed");

        let server = super::paperconnect_probe_server()
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

        let stop = super::easytier_stop().await;
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
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,easytier=debug")),
            )
            .with_test_writer()
            .try_init();
        crate::tasks::runtime::initialize_app_runtime().expect("initialize application runtime");

        let expect_discovery_port_occupied =
            std::env::var_os("BMCBL_TEST_EXPECT_7551_OCCUPIED").is_some();
        let room_code = std::env::var("BMCBL_TEST_ROOM_CODE")
            .unwrap_or_else(|_| "P/NPR4-E6J4-DYAG-VZH2".to_string());
        let room = paperconnect_parse_room_code(room_code)
            .await
            .expect("room code should parse");
        eprintln!(
            "[repro] network_name={} network_secret={}",
            room.network_name, room.network_secret
        );

        super::easytier_start(super::EasyTierStartRequest {
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
        .expect("easytier_start should succeed");

        // Watch route table while discovery runs so we can see what the guest observes.
        let watcher = tokio::spawn(async {
            for i in 0..12 {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                match super::easytier_embedded_peers().await {
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

        let probe = super::paperconnect_probe_server().await;
        eprintln!("[repro] probe result: {probe:?}");
        watcher.abort();

        let joined = match &probe {
            Ok(server) => {
                // Full production guest path: c:player heartbeat + RakNet tunnel
                // connection to the host through the EasyTier port forward.
                let join =
                    super::paperconnect_start_client(server.clone(), "ReproGuest".to_string())
                        .await;
                eprintln!("[repro] start_client result: {join:?}");
                eprintln!("[repro] players: {:?}", super::paperconnect_players());
                join
            }
            Err(error) => Err(error.clone()),
        };

        let local_discovery = match &joined {
            Ok(super::PaperConnectClientState::Ready) => {
                let signaling = bedrock_nethernet::LanSignaling::client(
                    std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
                    std::net::SocketAddr::from(([255, 255, 255, 255], 7551)),
                )
                .await
                .map_err(|error| format!("启动 7551 测试监听失败：{error}"));
                match signaling {
                    Ok(signaling) => signaling
                        .discover(std::time::Duration::from_secs(5))
                        .await
                        .map_err(|error| format!("监听 7551 响应失败：{error}")),
                    Err(error) => Err(error),
                }
            }
            Ok(super::PaperConnectClientState::DiscoveryPortOccupied) => {
                Err("本机 UDP 7551 已被占用，模拟代理未创建".to_string())
            }
            Err(error) => Err(error.clone()),
        };
        eprintln!("[repro] local 7551 discovery result: {local_discovery:?}");

        let stop = super::easytier_stop().await;
        eprintln!("[repro] stop result: {stop:?}");

        probe.expect("guest should discover the PaperConnect host");
        if expect_discovery_port_occupied {
            assert_eq!(
                joined,
                Ok(super::PaperConnectClientState::DiscoveryPortOccupied),
                "Minecraft 占用 7551 时必须保留 EasyTier 房间并拒绝创建模拟代理"
            );
            return;
        }
        joined.expect("guest transport should connect to the host tunnel");
        let discovered = local_discovery.expect("local UDP 7551 proxy should answer discovery");
        eprintln!(
            "[repro] local 7551 response: network_id={} address={} server_name={} level_name={} transport_layer={}",
            discovered.network_id,
            discovered.address,
            discovered.server_data.server_name,
            discovered.server_data.level_name,
            discovered.server_data.transport_layer
        );
    }

    #[tokio::test]
    async fn paperconnect_parser_accepts_published_format_and_rejects_malformed_code() {
        let room = paperconnect_parse_room_code("P/YNZE-U61D-2206-HXRG".to_string())
            .await
            .expect("documented PaperConnect room code should parse");
        assert_eq!(room.network_name, "paper-connect-YNZE-U61D");
        assert_eq!(room.network_secret, "2206-HXRG");

        let invalid = paperconnect_parse_room_code("P/YNZE-U61D-2206-SSSS+".to_string()).await;
        assert!(invalid.is_err());
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
