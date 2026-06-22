use anyhow::{Context as _, anyhow};
use easytier::common::config::{
    ConfigFileControl, ConfigLoader as _, NetworkIdentity, PeerConfig, TomlConfigLoader,
    gen_default_flags,
};
use easytier::instance_manager::NetworkInstanceManager;
use easytier::proto::api::instance::ListRouteRequest;
use easytier::proto::common::CompressionAlgoPb;
use easytier::proto::rpc_types::controller::BaseController;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::net::TcpListener as StdTcpListener;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::Instant;
use uuid::Uuid;

mod acl;

use crate::core::easytier::runtime::ensure_easytier_runtime_ready;
use crate::http::proxy::{build_no_proxy_client_with_resolve, get_no_proxy_client};
use crate::utils::cloudflare;
use acl::build_paperconnect_acl;

const DEFAULT_PAPERCONNECT_VIP: &str = "10.144.144.1";
const DEFAULT_BOOTSTRAP_PEERS: [&str; 1] = ["tcp://public.easytier.bmcbl.com:54321"];
const PUBLIC_BOOTSTRAP_PEERS_URL: &str = "https://et-public-node.roundstudio.top/";
const PUBLIC_BOOTSTRAP_PEERS_HOST: &str = "et-public-node.roundstudio.top";
const BOOTSTRAP_PEERS_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EasyTierEmbeddedStatus {
    pub instance_id: String,
    pub hostname: String,
    pub ipv4: Option<String>,
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

#[derive(Debug, Clone)]
struct EasyTierLastStart {
    network_name: String,
    network_secret: String,
    peers: Vec<String>,
    hostname: Option<String>,
    resolved_hostname: Option<String>,
    resolved_ipv4: Option<String>,
    options: Option<EasyTierStartOptions>,
}

#[derive(Default)]
struct OnlineState {
    easytier_manager: Arc<NetworkInstanceManager>,
    easytier_instance_id: Mutex<Option<Uuid>>,
    easytier_last_start: Mutex<Option<EasyTierLastStart>>,
}

static ONLINE_STATE: Lazy<OnlineState> = Lazy::new(|| OnlineState {
    easytier_manager: Arc::new(NetworkInstanceManager::new()),
    easytier_instance_id: Mutex::new(None),
    easytier_last_start: Mutex::new(None),
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
    let peers = fallback_bootstrap_peers();
    if let Ok(mut cache_guard) = BOOTSTRAP_PEERS_CACHE.lock() {
        *cache_guard = Some(BootstrapPeersCache {
            fetched_at: Instant::now(),
            peers: peers.clone(),
        });
    }
    peers
}

pub async fn paperconnect_pick_listen_port() -> Result<u16, String> {
    for _ in 0..12 {
        let listener = StdTcpListener::bind(("0.0.0.0", 0)).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener);
        if port > 0 {
            return Ok(port);
        }
    }
    Err("failed to pick an available port".to_string())
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
    let n = validate_group_chars_only(&format!("{}{}", parts[0], parts[1]))
        .map_err(|e| format!("invalid roomCode N group: {e}"))?;
    let secret = validate_group_chars_only(&format!("{}{}", parts[2], parts[3]))
        .map_err(|e| format!("invalid roomCode S group: {e}"))?;

    if let Ok(v) = group_to_value_le_base34(&n) {
        if v % 7 != 0 {
            tracing::warn!(
                room_code = %room_code,
                n_group = %n,
                mod7 = (v % 7),
                "roomCode N group checksum mismatch; accepting for compatibility"
            );
        }
    }
    if let Ok(v) = group_to_value_le_base34(&secret) {
        if v % 7 != 0 {
            tracing::warn!(
                room_code = %room_code,
                s_group = %secret,
                mod7 = (v % 7),
                "roomCode S group checksum mismatch; accepting for compatibility"
            );
        }
    }

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
    flags.use_smoltcp = true;
    flags.disable_p2p = true;
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
    if let Some(port_text) = hostname_value
        .trim()
        .strip_prefix("paper-connect-server-")
        .or_else(|| hostname_value.trim().strip_prefix("scaffolding-mc-server-"))
    {
        if let Ok(protocol_port) = port_text.parse::<u16>() {
            if (1025..=65535).contains(&protocol_port) {
                host_port_from_hostname = Some(protocol_port);
            }
        }
    }

    let is_paperconnect_network = network_name_for_policy.starts_with("paper-connect-")
        || network_name_for_policy.starts_with("scaffolding-mc-");
    let is_paperconnect_host = is_paperconnect_network && host_port_from_hostname.is_some();

    if ipv4.is_none() && is_paperconnect_network {
        if is_paperconnect_host {
            ipv4 = Some(cidr::Ipv4Inet::from_str(&format!(
                "{DEFAULT_PAPERCONNECT_VIP}/24"
            ))?);
        } else {
            let b = Uuid::new_v4().as_bytes()[0];
            let host = 2u8 + (b % 253u8);
            ipv4 = Some(cidr::Ipv4Inet::from_str(&format!("10.144.144.{host}/24"))?);
        }
        dhcp = false;
    }

    if !flags.no_tun {
        flags.use_smoltcp = false;
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

pub async fn easytier_start(
    network_name: String,
    network_secret: String,
    peers: Vec<String>,
    hostname: Option<String>,
    options: Option<EasyTierStartOptions>,
) -> Result<(), String> {
    ensure_easytier_runtime_ready()?;

    let peers = if peers.iter().any(|p| !p.trim().is_empty()) {
        sanitize_bootstrap_peers(peers)
    } else {
        default_bootstrap_peers().await
    };

    {
        let mut id = ONLINE_STATE.easytier_instance_id.lock().unwrap();
        if id.is_some() {
            return Err("EasyTier already running".to_string());
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
            options: options.clone(),
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

    Ok(())
}

pub async fn easytier_stop() -> Result<(), String> {
    let instance_id = ONLINE_STATE.easytier_instance_id.lock().unwrap().take();
    *ONLINE_STATE.easytier_last_start.lock().unwrap() = None;
    if let Some(id) = instance_id {
        let manager = ONLINE_STATE.easytier_manager.clone();
        tokio::task::spawn_blocking(move || manager.delete_network_instance(vec![id]))
            .await
            .map_err(|e| format!("stop embedded EasyTier join failed: {e}"))?
            .map_err(|e| format!("stop embedded EasyTier failed: {e}"))?;
    }
    Ok(())
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

    let resp = svc
        .get_peer_manage_service()
        .list_route(BaseController::default(), ListRouteRequest::default())
        .await
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

    if hostname.trim().is_empty() || ipv4.as_deref().unwrap_or_default().trim().is_empty() {
        if let Some(last) = ONLINE_STATE.easytier_last_start.lock().unwrap().clone() {
            if hostname.trim().is_empty() {
                if let Some(hn) = last.resolved_hostname.or(last.hostname) {
                    hostname = hn;
                }
            }
            if ipv4.as_deref().unwrap_or_default().trim().is_empty() {
                if let Some(v) = last.resolved_ipv4 {
                    if !v.trim().is_empty() {
                        ipv4 = Some(v);
                    }
                } else if let Some(opts) = last.options {
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
                    if hn.starts_with("paper-connect-server-") {
                        ipv4 = Some(DEFAULT_PAPERCONNECT_VIP.to_string());
                    }
                }
            }
        }
    }

    Ok(Some(EasyTierEmbeddedStatus {
        instance_id: inst_id,
        hostname,
        ipv4,
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

    let resp = svc
        .get_peer_manage_service()
        .list_route(BaseController::default(), ListRouteRequest::default())
        .await
        .map_err(|e| format!("list_route failed: {e}"))?;

    let mut peers = Vec::new();
    for r in resp.routes {
        let ipv4 = r.ipv4_addr.map(|inet| {
            let s = inet.to_string();
            s.split_once('/').map(|v| v.0.to_string()).unwrap_or(s)
        });

        let mut hostname = r.hostname;
        if hostname.trim().is_empty() {
            let id = r.inst_id.trim();
            hostname = if id.is_empty() {
                "node-unknown".to_string()
            } else {
                format!("node-{id}")
            };
        }
        peers.push(EasyTierPeer { ipv4, hostname });
    }
    Ok(peers)
}

pub async fn online_debug_snapshot() -> serde_json::Value {
    serde_json::json!({
        "ts": now_ms(),
        "running": ONLINE_STATE.easytier_instance_id.lock().unwrap().is_some(),
    })
}
