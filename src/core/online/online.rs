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

mod acl;
mod paperconnect;

pub use paperconnect::PaperConnectPlayer;

use crate::core::easytier::runtime::ensure_easytier_runtime_ready;
use crate::http::proxy::{build_no_proxy_client_with_resolve, get_no_proxy_client};
use crate::utils::cloudflare;
use acl::build_paperconnect_acl;

const DEFAULT_PAPERCONNECT_VIP: &str = "10.144.144.1";
const DEFAULT_BOOTSTRAP_PEERS: [&str; 1] = ["tcp://public.easytier.bmcbl.com:54321"];
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
    pub no_tun: bool,
    pub game_host: Option<String>,
    pub game_port: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EasyTierStartOptions {
    #[serde(alias = "disableP2p", alias = "disable_p2p")]
    pub disable_p2p: Option<bool>,
    #[serde(alias = "noTun", alias = "no_tun")]
    pub no_tun: Option<bool>,
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
    easytier_cleanup_in_progress: Arc<AtomicBool>,
}

static ONLINE_STATE: Lazy<OnlineState> = Lazy::new(|| OnlineState {
    easytier_manager: Arc::new(NetworkInstanceManager::new()),
    easytier_instance_id: Mutex::new(None),
    easytier_last_start: Mutex::new(None),
    easytier_game_endpoint: Mutex::new(None),
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
        Some(scheme) if scheme == "tcp" || scheme == "udp"
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
    flags.no_tun = false;
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
        if let Some(v) = opts.no_tun {
            flags.no_tun = v;
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
    if let Some(port_text) = hostname_value.trim().strip_prefix("paper-connect-server-") {
        if let Ok(protocol_port) = port_text.parse::<u16>() {
            if (1025..=65535).contains(&protocol_port) {
                host_port_from_hostname = Some(protocol_port);
            }
        }
    }

    let is_paperconnect_network = network_name_for_policy.starts_with("paper-connect-");
    let is_paperconnect_host = is_paperconnect_network && host_port_from_hostname.is_some();

    if is_paperconnect_network
        && hostname_value.trim().starts_with("paper-connect-server-")
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

    let no_tun_enabled = flags.no_tun;
    cfg.set_flags(flags);

    if is_paperconnect_network && !no_tun_enabled {
        let acl = build_paperconnect_acl(
            is_paperconnect_host,
            DEFAULT_PAPERCONNECT_VIP,
            host_port_from_hostname,
        );
        cfg.set_acl(Some(acl));
    }

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
        hostname,
        player_name,
        game_port,
        options,
    } = request;
    if !(1025..=65535).contains(&game_port) {
        return Err(format!("invalid PaperConnect game port: {game_port}"));
    }
    if player_name.trim().is_empty() {
        return Err("PaperConnect player name is empty".to_string());
    }

    ensure_easytier_runtime_ready()?;

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

    {
        let mut id = ONLINE_STATE.easytier_instance_id.lock().unwrap();
        if id.is_some() {
            return Err("EasyTier already running".to_string());
        }
        if ONLINE_STATE
            .easytier_cleanup_in_progress
            .load(Ordering::Acquire)
        {
            return Err("上一条联机连接仍在清理，请稍候再试".to_string());
        }
        let (cfg, resolved_hostname, resolved_ipv4) = build_embedded_easytier_config(
            network_name.clone(),
            network_secret.clone(),
            peers.clone(),
            hostname.clone(),
            options.clone(),
        )
        .map_err(|e| e.to_string())?;

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

        let no_tun = options
            .as_ref()
            .and_then(|value| value.no_tun)
            .unwrap_or(false);
        let is_host = hostname
            .as_deref()
            .and_then(paperconnect::server_port_from_hostname)
            .is_some();
        *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() =
            is_host.then(|| EasyTierGameEndpoint {
                host: if no_tun {
                    "127.0.0.1".to_string()
                } else {
                    DEFAULT_PAPERCONNECT_VIP.to_string()
                },
                port: game_port,
            });

        let instance_id = ONLINE_STATE
            .easytier_manager
            .run_network_instance(cfg, true, ConfigFileControl::STATIC_CONFIG)
            .map_err(|e| format!("start embedded EasyTier failed: {e}"))?;
        *id = Some(instance_id);
    }

    let instance_id = *ONLINE_STATE
        .easytier_instance_id
        .lock()
        .unwrap()
        .as_ref()
        .unwrap();
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
        if let Err(error) =
            paperconnect::start_server(server_port, game_port, player_name.clone()).await
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
    paperconnect::stop_server();
    paperconnect::stop_client();
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
        let mut cleanup =
            tokio::task::spawn_blocking(move || manager.delete_network_instance(vec![id]));
        match tokio::time::timeout(EASYTIER_STOP_TIMEOUT, &mut cleanup).await {
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
                tokio::spawn(async move {
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
                    cleanup_in_progress.store(false, Ordering::Release);
                });
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
    forward: Option<EasyTierPortForward>,
}

async fn create_paperconnect_control_endpoint(
    host_addr: IpAddr,
    server_port: u16,
    no_tun: bool,
) -> Result<PaperConnectControlEndpoint, String> {
    let remote_addr = SocketAddr::new(host_addr, server_port);
    if !no_tun {
        return Ok(PaperConnectControlEndpoint {
            host: host_addr.to_string(),
            port: server_port,
            remote_addr,
            forward: None,
        });
    }

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
        forward: Some(forward),
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

async fn configure_paperconnect_game_endpoint(
    mut server: paperconnect::ServerInfo,
    host_addr: IpAddr,
    no_tun: bool,
) -> Result<paperconnect::ServerInfo, String> {
    if no_tun {
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
    } else {
        server.game_host = server.host.clone();
    }

    *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() = Some(EasyTierGameEndpoint {
        host: server.game_host.clone(),
        port: server.game_port,
    });
    Ok(server)
}

fn easytier_no_tun() -> bool {
    ONLINE_STATE
        .easytier_last_start
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|value| value.options.as_ref())
        .and_then(|value| value.no_tun)
        .unwrap_or(false)
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
    let no_tun = last_start
        .as_ref()
        .and_then(|value| value.options.as_ref())
        .and_then(|value| value.no_tun)
        .unwrap_or(false);
    let game_port = game_endpoint
        .as_ref()
        .map(|endpoint| endpoint.port)
        .or_else(|| last_start.as_ref().map(|value| value.game_port));
    let game_host = game_endpoint.map(|endpoint| endpoint.host);

    Ok(Some(EasyTierEmbeddedStatus {
        instance_id: inst_id,
        hostname,
        ipv4,
        no_tun,
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
    let no_tun = easytier_no_tun();
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
            let Some(server_port) = paperconnect::server_port_from_hostname(&peer.hostname) else {
                continue;
            };
            let Some(host) = peer.ipv4 else {
                last_probe_error = Some("已发现房主节点，但节点没有虚拟 IP".to_string());
                continue;
            };
            let Ok(host_addr) = host.parse::<IpAddr>() else {
                last_probe_error = Some(format!("房主节点返回了无效虚拟 IP：{host}"));
                continue;
            };
            let control = match create_paperconnect_control_endpoint(host_addr, server_port, no_tun)
                .await
            {
                Ok(control) => control,
                Err(error) => {
                    last_probe_error = Some(format!("创建 PaperConnect 控制端口转发失败：{error}"));
                    continue;
                }
            };
            match probe_paperconnect_control_endpoint(&control, deadline).await {
                Ok(mut server) => {
                    server.host = control.host.clone();
                    server.server_port = control.port;
                    match configure_paperconnect_game_endpoint(server, host_addr, no_tun).await {
                        Ok(server) => return Ok(server),
                        Err(error) => {
                            last_probe_error =
                                Some(format!("创建 PaperConnect 游戏端口转发失败：{error}"));
                        }
                    }
                }
                Err(error) => last_probe_error = Some(error),
            }
            if let Some(forward) = control.forward {
                forward.remove("控制端口转发").await;
            }
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
    host: String,
    port: u16,
    player_name: String,
) -> Result<(), String> {
    paperconnect::start_client(host, port, player_name).await
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
        EasyTierStartOptions, build_embedded_easytier_config, merge_bootstrap_peers,
        paperconnect_parse_room_code, sanitize_bootstrap_peers,
    };

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
            "https://node.example/peers".to_string(),
            "".to_string(),
        ]);

        assert_eq!(
            peers,
            vec![
                "tcp://node.example:54321".to_string(),
                "udp://node.example:54321".to_string(),
            ]
        );
    }

    #[test]
    fn paperconnect_config_preserves_requested_tun_mode() {
        let options = EasyTierStartOptions {
            disable_p2p: None,
            no_tun: Some(true),
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

    #[test]
    fn paperconnect_config_keeps_tun_enabled_when_requested() {
        let options = EasyTierStartOptions {
            disable_p2p: None,
            no_tun: Some(false),
            compression: None,
            ipv4: None,
        };
        let (config, _, _) = build_embedded_easytier_config(
            "paper-connect-TEST-ROOM".to_string(),
            "TEST-KEY".to_string(),
            vec!["tcp://public.example:54321".to_string()],
            Some("bmcbl-client-player".to_string()),
            Some(options),
        )
        .expect("PaperConnect TUN config should be valid");

        assert!(!config.get_flags().no_tun);
        assert!(config.get_dhcp());
        assert!(config.get_ipv4().is_none());
    }
}
