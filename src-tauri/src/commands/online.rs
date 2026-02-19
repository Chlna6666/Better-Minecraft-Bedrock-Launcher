use anyhow::{anyhow, Context};
use easytier::common::config::{
    gen_default_flags, ConfigFileControl, ConfigLoader, NetworkIdentity, PeerConfig,
    PortForwardConfig, TomlConfigLoader,
};
use easytier::common::stun::{StunInfoCollector, StunInfoCollectorTrait};
use easytier::instance_manager::NetworkInstanceManager;
use easytier::proto::api::instance::ListRouteRequest;
use easytier::proto::common::CompressionAlgoPb;
use easytier::proto::rpc_types::controller::BaseController;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::net::{IpAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::State;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

use super::online_acl::build_paperconnect_acl;

const MAX_PACKET_SIZE: usize = 64 * 1024;
const ZSTD_LEVEL: i32 = 3;
const DEFAULT_PAPERCONNECT_VIP: &str = "10.144.144.1";
const DEFAULT_BOOTSTRAP_PEERS: [&str; 2] =
    ["tcp://39.108.52.138:11010", "tcp://8.148.29.206:11010"];

pub struct OnlineState {
    easytier_manager: Arc<NetworkInstanceManager>,
    easytier_instance_id: Mutex<Option<Uuid>>,
    easytier_last_start: Mutex<Option<EasyTierLastStart>>,
    paperconnect_server: Mutex<Option<PaperConnectServerHandle>>,
}

impl Default for OnlineState {
    fn default() -> Self {
        Self {
            easytier_manager: Arc::new(NetworkInstanceManager::new()),
            easytier_instance_id: Mutex::new(None),
            easytier_last_start: Mutex::new(None),
            paperconnect_server: Mutex::new(None),
        }
    }
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

struct PaperConnectServerHandle {
    shutdown: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    listen_port: u16,
    state: std::sync::Arc<tokio::sync::Mutex<PaperConnectServerState>>,
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
pub struct PaperConnectCenter {
    pub ipv4: Option<String>,
    pub hostname: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EasyTierEmbeddedStatus {
    pub instance_id: String,
    pub hostname: String,
    pub ipv4: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EasyTierNatTypeSnapshot {
    pub udp_nat_type: i32,
    pub tcp_nat_type: i32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperConnectPlayerEntry {
    pub player: String,
    pub client_id: String,
    pub is_room_host: bool,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperConnectServerSnapshot {
    pub return_time: i64,
    pub listen_port: u16,
    pub game_port: u16,
    pub game_type: String,
    pub game_protocol_type: String,
    pub players: Vec<PaperConnectPlayerEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperConnectServerStartArgs {
    pub listen_port: u16,
    pub game_port: u16,
    pub game_type: String,
    pub game_protocol_type: String,
    #[serde(default)]
    pub room_host_player_name: Option<String>,
    #[serde(default)]
    pub room_host_client_id: Option<String>,
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
    #[serde(alias = "tcpWhitelist", alias = "tcp_whitelist")]
    pub tcp_whitelist: Option<Vec<u16>>,
    #[serde(alias = "udpWhitelist", alias = "udp_whitelist")]
    pub udp_whitelist: Option<Vec<u16>>,
    #[serde(alias = "ipv4")]
    pub ipv4: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EasyTierPortForwardArgs {
    pub proto: String,
    pub bind_port: u16,
    pub dst_ip: String,
    pub dst_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TcpFraming {
    Raw,
    LengthPrefixedLe,
    LengthPrefixedBe,
    LengthPrefixedU16Le,
    LengthPrefixedU16Be,
    LineDelimited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TcpEncoding {
    Plain,
    Zstd,
}

#[derive(Debug, Clone)]
struct TcpPacket {
    text: String,
    framing: TcpFraming,
    encoding: TcpEncoding,
}

fn now_ms() -> i64 {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    d.as_millis() as i64
}

fn default_client_id() -> String {
    format!("BMCBL v{}", env!("CARGO_PKG_VERSION"))
}

fn default_bootstrap_peers() -> Vec<String> {
    DEFAULT_BOOTSTRAP_PEERS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

static DEFAULT_PLAYER_NAME: Lazy<String> = Lazy::new(|| {
    let raw = Uuid::new_v4().simple().to_string();
    let suffix = raw.chars().take(4).collect::<String>().to_ascii_uppercase();
    format!("BMCBL_USER_{suffix}")
});

fn default_player_name() -> String {
    DEFAULT_PLAYER_NAME.clone()
}

#[tauri::command]
pub async fn paperconnect_default_client_id() -> Result<String, String> {
    Ok(default_client_id())
}

#[tauri::command]
pub async fn paperconnect_pick_listen_port() -> Result<u16, String> {
    // Pick an ephemeral TCP port that is likely to be available for PaperConnect.
    // This isn't perfectly race-free, but we start the server immediately after in the UI flow.
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

#[tauri::command]
pub async fn paperconnect_generate_room() -> Result<PaperConnectRoom, String> {
    let n = random_group8_div7();
    let s = random_group8_div7();
    let room_code = format!("P/{n}-{s}");
    Ok(PaperConnectRoom {
        room_code: room_code.clone(),
        network_name: format!("paper-connect-{n}"),
        network_secret: s,
    })
}

#[tauri::command]
pub async fn paperconnect_parse_room_code(room_code: String) -> Result<PaperConnectRoom, String> {
    let s = room_code.trim();
    let s = s
        .strip_prefix("P/")
        .ok_or_else(|| "roomCode must start with P/".to_string())?;
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 4 {
        return Err("roomCode must be like P/NNNN-NNNN-SSSS-SSSS".to_string());
    }
    let n = validate_group_chars_only(&format!("{}{}", parts[0], parts[1]))
        .map_err(|e| format!("invalid roomCode N group: {e}"))?;
    let sec = validate_group_chars_only(&format!("{}{}", parts[2], parts[3]))
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
    if let Ok(v) = group_to_value_le_base34(&sec) {
        if v % 7 != 0 {
            tracing::warn!(
                room_code = %room_code,
                s_group = %sec,
                mod7 = (v % 7),
                "roomCode S group checksum mismatch; accepting for compatibility"
            );
        }
    }

    let room_code = format!("P/{n}-{sec}");
    Ok(PaperConnectRoom {
        room_code,
        network_name: format!("paper-connect-{n}"),
        network_secret: sec,
    })
}

#[tauri::command]
pub async fn easytier_start(
    state: State<'_, OnlineState>,
    network_name: String,
    network_secret: String,
    peers: Vec<String>,
    hostname: Option<String>,
    options: Option<EasyTierStartOptions>,
) -> Result<(), String> {
    let peers = if peers.iter().any(|p| !p.trim().is_empty()) {
        peers
    } else {
        default_bootstrap_peers()
    };

    {
        let mut id = state.easytier_instance_id.lock().unwrap();
        if id.is_some() {
            return Err("EasyTier already running".to_string());
        }
        let (cfg, resolved_hostname, resolved_ipv4) =
            build_embedded_easytier_config_with_port_forwards(
                network_name.clone(),
                network_secret.clone(),
                peers.clone(),
                hostname.clone(),
                options.clone(),
                Vec::new(),
            )
            .map_err(|e| e.to_string())?;
        *state.easytier_last_start.lock().unwrap() = Some(EasyTierLastStart {
            network_name: network_name.clone(),
            network_secret: network_secret.clone(),
            peers: peers.clone(),
            hostname: hostname.clone(),
            resolved_hostname,
            resolved_ipv4,
            options: options.clone(),
        });
        let instance_id = state
            .easytier_manager
            .run_network_instance(cfg, true, ConfigFileControl::STATIC_CONFIG)
            .map_err(|e| format!("start embedded EasyTier failed: {e}"))?;
        *id = Some(instance_id);
    }

    let instance_id = *state.easytier_instance_id.lock().unwrap().as_ref().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let has_api = state
            .easytier_manager
            .get_instance_service(&instance_id)
            .is_some();
        if has_api {
            break;
        }

        let mut is_running = false;
        let mut last_err: Option<String> = None;
        for i in state.easytier_manager.iter() {
            if *i.key() != instance_id {
                continue;
            }
            is_running = i.value().is_easytier_running();
            last_err = i.value().get_latest_error_msg();
            break;
        }

        if !is_running {
            *state.easytier_instance_id.lock().unwrap() = None;
            *state.easytier_last_start.lock().unwrap() = None;
            let _ = state
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

#[tauri::command]
pub async fn easytier_restart_with_port_forwards(
    state: State<'_, OnlineState>,
    forwards: Vec<EasyTierPortForwardArgs>,
    options: Option<EasyTierStartOptions>,
) -> Result<(), String> {
    let Some(last) = state.easytier_last_start.lock().unwrap().clone() else {
        return Err("EasyTier not started yet".to_string());
    };

    // Preflight bind ports before stopping the existing instance. This avoids a bad UX where
    // EasyTier is stopped, then the restart fails due to local bind errors (e.g. Windows 10013).
    let mut preflight: Vec<(String, u16)> = Vec::new();
    for f in forwards.iter() {
        let proto = f.proto.trim().to_ascii_lowercase();
        if proto != "tcp" && proto != "udp" {
            return Err(format!("invalid port forward proto: {}", f.proto));
        }
        if preflight
            .iter()
            .any(|(p, port)| p == &proto && *port == f.bind_port)
        {
            continue;
        }
        preflight.push((proto, f.bind_port));
    }

    for (proto, port) in preflight.iter() {
        let bind = ("127.0.0.1", *port);
        let res = match proto.as_str() {
            "tcp" => StdTcpListener::bind(bind)
                .map(|l| drop(l))
                .map_err(|e| e.to_string()),
            "udp" => StdUdpSocket::bind(bind)
                .map(|s| drop(s))
                .map_err(|e| e.to_string()),
            _ => Ok(()),
        };
        if let Err(msg) = res {
            return Err(format!(
                "cannot bind local {proto} port {port} on 127.0.0.1 ({msg}). Try changing the port, closing the app using it, or running as admin (Windows may block some ports, e.g. os error 10013)."
            ));
        }
    }

    let network_name = last.network_name.clone();
    let network_secret = last.network_secret.clone();
    let peers = last.peers.clone();
    let hostname = last.hostname.clone();
    let options = options.or_else(|| last.options.clone());

    let old_id = state.easytier_instance_id.lock().unwrap().take();
    if let Some(id) = old_id {
        let mgr = state.easytier_manager.clone();
        tokio::task::spawn_blocking(move || mgr.delete_network_instance(vec![id]))
            .await
            .map_err(|e| format!("stop embedded EasyTier join failed: {e}"))?
            .map_err(|e| format!("stop embedded EasyTier failed: {e}"))?;
    }

    let mut port_forwards = Vec::new();
    for f in forwards.into_iter() {
        let proto = f.proto.trim().to_ascii_lowercase();
        if proto != "tcp" && proto != "udp" {
            return Err(format!("invalid port forward proto: {}", f.proto));
        }
        // PaperConnect clients connect to the overlay via local loopback port-forwards.
        // Binding on 127.0.0.1 is more reliable on Windows and avoids exposing ports.
        let bind_addr = format!("127.0.0.1:{}", f.bind_port)
            .parse()
            .map_err(|e| format!("invalid bind port {}: {e}", f.bind_port))?;
        let dst_addr = format!("{}:{}", f.dst_ip.trim(), f.dst_port)
            .parse()
            .map_err(|e| format!("invalid dst addr {}:{}: {e}", f.dst_ip, f.dst_port))?;
        port_forwards.push(PortForwardConfig {
            bind_addr,
            dst_addr,
            proto,
        });
    }

    let (cfg, resolved_hostname, resolved_ipv4) =
        build_embedded_easytier_config_with_port_forwards(
            network_name.clone(),
            network_secret.clone(),
            peers.clone(),
            hostname.clone(),
            options.clone(),
            port_forwards,
        )
        .map_err(|e| e.to_string())?;

    let instance_id = match state.easytier_manager.run_network_instance(
        cfg,
        true,
        ConfigFileControl::STATIC_CONFIG,
    ) {
        Ok(id) => id,
        Err(e) => {
            // Best-effort rollback: bring EasyTier back up without port-forwards so the online session
            // isn't left completely offline if port-forward restart fails.
            let rollback_cfg = build_embedded_easytier_config(
                network_name,
                network_secret,
                peers,
                hostname,
                options,
            )
            .map_err(|e2| {
                format!("restart embedded EasyTier failed: {e}; rollback build failed: {e2}")
            })?;

            let rollback_id = state
                .easytier_manager
                .run_network_instance(rollback_cfg, true, ConfigFileControl::STATIC_CONFIG)
                .map_err(|e2| {
                    format!("restart embedded EasyTier failed: {e}; rollback start failed: {e2}")
                })?;

            *state.easytier_instance_id.lock().unwrap() = Some(rollback_id);
            return Err(format!("restart embedded EasyTier failed: {e}"));
        }
    };

    *state.easytier_instance_id.lock().unwrap() = Some(instance_id);
    *state.easytier_last_start.lock().unwrap() = Some(EasyTierLastStart {
        network_name,
        network_secret,
        peers,
        hostname,
        resolved_hostname,
        resolved_ipv4,
        options,
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let has_api = state
            .easytier_manager
            .get_instance_service(&instance_id)
            .is_some();
        if has_api {
            break;
        }

        let mut is_running = false;
        let mut last_err: Option<String> = None;
        for i in state.easytier_manager.iter() {
            if *i.key() != instance_id {
                continue;
            }
            is_running = i.value().is_easytier_running();
            last_err = i.value().get_latest_error_msg();
            break;
        }

        if !is_running {
            *state.easytier_instance_id.lock().unwrap() = None;
            // Keep last_start so the user can retry without losing settings.
            let _ = state
                .easytier_manager
                .delete_network_instance(vec![instance_id]);
            return Err(format!(
                "embedded EasyTier stopped during restart: {}",
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

#[tauri::command]
pub async fn easytier_stop(state: State<'_, OnlineState>) -> Result<(), String> {
    let instance_id = state.easytier_instance_id.lock().unwrap().take();
    *state.easytier_last_start.lock().unwrap() = None;
    if let Some(id) = instance_id {
        let mgr = state.easytier_manager.clone();
        tokio::task::spawn_blocking(move || mgr.delete_network_instance(vec![id]))
            .await
            .map_err(|e| format!("stop embedded EasyTier join failed: {e}"))?
            .map_err(|e| format!("stop embedded EasyTier failed: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn paperconnect_find_center(
    state: State<'_, OnlineState>,
    cli_path: Option<String>,
) -> Result<Option<PaperConnectCenter>, String> {
    let assume_default_vip = state
        .easytier_last_start
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|l| l.options.as_ref())
        .and_then(|o| o.no_tun)
        .unwrap_or(true);

    let peers = if state.easytier_instance_id.lock().unwrap().is_some() {
        easytier_embedded_peers(state).await?
    } else if let Some(p) = cli_path {
        easytier_cli_peers(p).await?
    } else {
        return Ok(None);
    };
    let re = Regex::new(r"^(?:paper-connect-server|scaffolding-mc-server)-(\d{2,5})$")
        .map_err(|e| e.to_string())?;

    let mut fallback: Option<PaperConnectCenter> = None;
    for p in peers {
        if let Some(caps) = re.captures(p.hostname.trim()) {
            let port: u16 = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<u16>().ok())
                .unwrap_or(0);
            if (1025..=65535).contains(&port) {
                let mut ipv4 = p.ipv4;
                if assume_default_vip && ipv4.as_deref().unwrap_or_default().trim().is_empty() {
                    ipv4 = Some(DEFAULT_PAPERCONNECT_VIP.to_string());
                }
                let c = PaperConnectCenter {
                    ipv4,
                    hostname: p.hostname,
                    port,
                };
                if c.ipv4.as_deref().unwrap_or_default().trim().is_empty() {
                    fallback = Some(c);
                    continue;
                }
                return Ok(Some(c));
            }
        }
    }
    Ok(fallback)
}

fn build_embedded_easytier_config(
    network_name: String,
    network_secret: String,
    peers: Vec<String>,
    hostname: Option<String>,
    options: Option<EasyTierStartOptions>,
) -> anyhow::Result<TomlConfigLoader> {
    let (cfg, _, _) = build_embedded_easytier_config_with_port_forwards(
        network_name,
        network_secret,
        peers,
        hostname,
        options,
        Vec::new(),
    )?;
    Ok(cfg)
}

fn build_embedded_easytier_config_with_port_forwards(
    network_name: String,
    network_secret: String,
    peers: Vec<String>,
    hostname: Option<String>,
    options: Option<EasyTierStartOptions>,
    port_forwards: Vec<PortForwardConfig>,
) -> anyhow::Result<(TomlConfigLoader, Option<String>, Option<String>)> {
    let net_name_for_policy = network_name.clone();

    let cfg = TomlConfigLoader::default();
    cfg.set_network_identity(NetworkIdentity::new(network_name, network_secret));
    cfg.set_hostname(hostname);
    cfg.set_listeners(vec![
        url::Url::parse("udp://0.0.0.0:0")?,
        url::Url::parse("tcp://0.0.0.0:0")?,
    ]);

    let mut flags = gen_default_flags();
    flags.bind_device = false;
    flags.no_tun = true;
    // When running without TUN, EasyTier relies on the smoltcp userspace stack for
    // proxy/port-forward behavior (PaperConnect's typical setup).
    flags.use_smoltcp = true;
    flags.disable_p2p = true;
    // Compress data-plane packets between EasyTier peers to reduce relay/egress bandwidth.
    flags.data_compress_algo = CompressionAlgoPb::Zstd.into();

    let mut tcp_whitelist: Option<Vec<String>> = None;
    let mut udp_whitelist: Option<Vec<String>> = None;
    let mut ipv4: Option<cidr::Ipv4Inet> = None;
    let mut dhcp = true;
    let mut host_port_from_hostname: Option<u16> = None;
    let mut paperconnect_protocol_port: Option<u16> = None;

    if let Some(opts) = options {
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
        if let Some(wl) = opts.tcp_whitelist {
            tcp_whitelist = Some(wl.into_iter().map(|p| p.to_string()).collect());
        }
        if let Some(wl) = opts.udp_whitelist {
            udp_whitelist = Some(wl.into_iter().map(|p| p.to_string()).collect());
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

    // PaperConnect host discovery uses `paper-connect-server-PORT`. When using `no_tun`, follow
    // PaperConnect's convention by default:
    // - fixed virtual IP = 10.144.144.1/24
    // - whitelist TCP = PaperConnect port
    // - whitelist UDP = Bedrock port (default 19132) if not provided
    let hn = cfg.get_hostname();
    if let Some(port_str) = hn
        .trim()
        .strip_prefix("paper-connect-server-")
        .or_else(|| hn.trim().strip_prefix("scaffolding-mc-server-"))
    {
        if let Ok(p) = port_str.parse::<u16>() {
            if (1025..=65535).contains(&p) {
                host_port_from_hostname = Some(p);
            }
        }
    }

    let is_paperconnect_net = net_name_for_policy.starts_with("paper-connect-")
        || net_name_for_policy.starts_with("scaffolding-mc-");
    let is_paperconnect_host = is_paperconnect_net && host_port_from_hostname.is_some();

    // Joiners may learn the PaperConnect protocol port out-of-band (e.g. via `paperconnect_find_center`)
    // and restart EasyTier with `tcpWhitelist: [port]` to apply the stricter ACL.
    paperconnect_protocol_port = host_port_from_hostname.or_else(|| {
        tcp_whitelist.as_ref().and_then(|wl| {
            if wl.len() != 1 {
                return None;
            }
            wl[0]
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|p| (1025..=65535).contains(p))
        })
    });

    // PaperConnect expects a stable virtual subnet (10.144.144.0/24). Some EasyTier setups may
    // otherwise allocate an internal DHCP pool from a different private range, causing peers to
    // end up in a different /24 and breaking host discovery/port-forward assumptions.
    //
    // Enforce the PaperConnect subnet whenever the network name matches, regardless of `no_tun`,
    // unless the caller explicitly provided an ipv4 override.
    if ipv4.is_none() && is_paperconnect_net {
        if host_port_from_hostname.is_some() {
            // PaperConnect host: fixed virtual IP for compatibility.
            ipv4 = Some(cidr::Ipv4Inet::from_str(&format!(
                "{DEFAULT_PAPERCONNECT_VIP}/24"
            ))?);
            dhcp = false;
        } else {
            // PaperConnect clients: keep them in the same /24 so the host can be reached
            // consistently. Use a random-but-valid host octet to reduce collisions.
            let b = Uuid::new_v4().as_bytes()[0];
            let host = 2u8 + (b % 253u8); // 2..254 (avoid .0/.1/.255)
            ipv4 = Some(cidr::Ipv4Inet::from_str(&format!("10.144.144.{host}/24"))?);
            dhcp = false;
        }
    }

    if flags.no_tun {
        // Prefer "open all ports" to avoid accidental connectivity issues when users run Bedrock
        // on non-default ports or when connectivity probes expect additional ports.
        //
        // Port-forwards (when used) still control what is exposed on the local loopback side.
        tcp_whitelist = None;
        udp_whitelist = None;
    }

    if !flags.no_tun {
        flags.use_smoltcp = false;
    }
    let no_tun_enabled = flags.no_tun;
    cfg.set_flags(flags);

    // PaperConnect security policy (TUN only): use a custom inbound ACL instead of destination-port
    // whitelists. This allows UDP-based transports (RakNet / NetherNet / WebRTC) on any port while
    // still blocking member-to-member traffic (joiner <-> joiner).
    if is_paperconnect_net && !no_tun_enabled {
        // Avoid mixing whitelist rules and ACL policy.
        tcp_whitelist = None;
        udp_whitelist = None;

        let acl = build_paperconnect_acl(
            is_paperconnect_host,
            DEFAULT_PAPERCONNECT_VIP,
            paperconnect_protocol_port,
        );
        cfg.set_acl(Some(acl));
    }

    let resolved_ipv4 = ipv4.as_ref().map(|inet| {
        let s = inet.to_string();
        s.split_once('/').map(|v| v.0.to_string()).unwrap_or(s)
    });

    cfg.set_dhcp(dhcp);
    cfg.set_ipv4(ipv4);
    if let Some(wl) = tcp_whitelist {
        cfg.set_tcp_whitelist(wl);
    }
    if let Some(wl) = udp_whitelist {
        cfg.set_udp_whitelist(wl);
    }
    cfg.set_port_forwards(port_forwards);

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

#[tauri::command]
pub async fn easytier_embedded_status(
    state: State<'_, OnlineState>,
) -> Result<Option<EasyTierEmbeddedStatus>, String> {
    let id = match state.easytier_instance_id.lock().unwrap().as_ref() {
        Some(v) => *v,
        None => return Ok(None),
    };

    let svc = match state.easytier_manager.get_instance_service(&id) {
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

    // Some EasyTier configurations (notably `no_tun` + smoltcp) may take time to populate
    // the route table fields for the local instance. Provide a deterministic fallback
    // based on the last start config so the UI can proceed.
    if hostname.trim().is_empty() || ipv4.as_deref().unwrap_or_default().trim().is_empty() {
        if let Some(last) = state.easytier_last_start.lock().unwrap().clone() {
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
                    if hn.starts_with("paper-connect-server-")
                        || hn.starts_with("scaffolding-mc-server-")
                    {
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

#[tauri::command]
pub async fn easytier_embedded_peers(
    state: State<'_, OnlineState>,
) -> Result<Vec<EasyTierPeer>, String> {
    let id = state
        .easytier_instance_id
        .lock()
        .unwrap()
        .ok_or_else(|| "EasyTier not running".to_string())?;

    let svc = state
        .easytier_manager
        .get_instance_service(&id)
        .ok_or_else(|| "EasyTier API service not available".to_string())?;

    // `list_peer` doesn't include hostname/ip directly; use route table snapshot instead.
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
        // Some third-party clients don't set hostname; keep them visible in UI.
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

fn parse_easytier_peer_table(out: &str) -> Vec<EasyTierPeer> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in out.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            continue;
        }
        let cols: Vec<String> = t
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_string())
            .collect();
        if cols.len() >= 2 {
            rows.push(cols);
        }
    }

    let mut header_row_idx: Option<usize> = None;
    let mut ipv4_idx: Option<usize> = None;
    let mut host_idx: Option<usize> = None;
    for (i, row) in rows.iter().enumerate() {
        let lower: Vec<String> = row.iter().map(|c| c.to_ascii_lowercase()).collect();
        if let Some(a) = lower.iter().position(|c| c == "ipv4") {
            if let Some(b) = lower.iter().position(|c| c == "hostname") {
                header_row_idx = Some(i);
                ipv4_idx = Some(a);
                host_idx = Some(b);
                break;
            }
        }
    }

    let (header_row_idx, ipv4_idx, host_idx) = match (header_row_idx, ipv4_idx, host_idx) {
        (Some(h), Some(a), Some(b)) => (h, a, b),
        _ => return Vec::new(),
    };

    let mut peers = Vec::new();
    for row in rows.into_iter().skip(header_row_idx + 1) {
        let ipv4 = row.get(ipv4_idx).cloned().unwrap_or_default();
        let hostname = row.get(host_idx).cloned().unwrap_or_default();
        if hostname.is_empty() {
            continue;
        }
        let ipv4 = if ipv4.trim().is_empty() {
            None
        } else {
            Some(ipv4)
        };
        peers.push(EasyTierPeer { ipv4, hostname });
    }
    peers
}

#[tauri::command]
pub async fn easytier_cli_peers(cli_path: String) -> Result<Vec<EasyTierPeer>, String> {
    let out = tokio::process::Command::new(cli_path)
        .arg("peer")
        .output()
        .await
        .map_err(|e| format!("run easytier-cli peer failed: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "easytier-cli peer failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    let text = String::from_utf8_lossy(&out.stdout);
    Ok(parse_easytier_peer_table(&text))
}

async fn read_one_message(stream: &mut TcpStream) -> anyhow::Result<String> {
    Ok(read_one_packet(stream).await?.text)
}

async fn write_message_and_close(stream: &mut TcpStream, msg: &str) -> anyhow::Result<()> {
    write_packet_and_close(stream, msg, TcpFraming::LineDelimited).await
}

fn looks_like_zstd_frame(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    magic == 0xFD2F_B528 || (0x184D_2A50..=0x184D_2A5F).contains(&magic)
}

fn zstd_compress(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    zstd::stream::encode_all(Cursor::new(bytes), ZSTD_LEVEL).context("zstd compress")
}

fn zstd_decompress_limited(bytes: &[u8], limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut decoder =
        zstd::stream::read::Decoder::new(Cursor::new(bytes)).context("zstd decoder")?;
    let mut out = Vec::<u8>::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = decoder.read(&mut tmp).context("zstd read")?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&tmp[..n]);
        if out.len() > limit {
            return Err(anyhow!("decompressed packet too large"));
        }
    }
    Ok(out)
}

fn decode_packet_text(bytes: Vec<u8>) -> anyhow::Result<(String, TcpEncoding)> {
    match String::from_utf8(bytes) {
        Ok(s) => Ok((s, TcpEncoding::Plain)),
        Err(e) => {
            let bytes = e.into_bytes();
            if !looks_like_zstd_frame(&bytes) {
                return Err(anyhow!("packet must be utf8"));
            }
            let out = zstd_decompress_limited(&bytes, MAX_PACKET_SIZE)
                .context("zstd decompress packet")?;
            let s = String::from_utf8(out).context("decompressed packet must be utf8")?;
            Ok((s, TcpEncoding::Zstd))
        }
    }
}

fn effective_response_encoding(req_encoding: TcpEncoding, framing: TcpFraming) -> TcpEncoding {
    // Line-delimited framing isn't safe for binary payloads.
    if framing == TcpFraming::LineDelimited {
        return TcpEncoding::Plain;
    }
    req_encoding
}

async fn read_one_packet(stream: &mut TcpStream) -> anyhow::Result<TcpPacket> {
    let mut hdr = [0u8; 4];
    let mut prebuf = Vec::<u8>::new();

    // Try to read a 4-byte length prefix first. If it doesn't look valid, fall back to unframed text.
    match stream.read_exact(&mut hdr).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            let (msg, encoding) = decode_packet_text(prebuf)?;
            return Ok(TcpPacket {
                text: msg,
                framing: TcpFraming::Raw,
                encoding,
            });
        }
        Err(e) => return Err(anyhow!(e).context("read tcp")),
    }

    let len_le = u32::from_le_bytes(hdr) as usize;
    let len_be = u32::from_be_bytes(hdr) as usize;
    let len_le_ok = (1..=MAX_PACKET_SIZE).contains(&len_le);
    let len_be_ok = (1..=MAX_PACKET_SIZE).contains(&len_be);

    let mut try_u32 = Vec::new();
    if len_le_ok {
        try_u32.push((len_le, TcpFraming::LengthPrefixedLe));
    }
    if len_be_ok && len_be != len_le {
        try_u32.push((len_be, TcpFraming::LengthPrefixedBe));
    }
    for (len, framing) in try_u32 {
        let mut buf = vec![0u8; len];
        if stream.read_exact(&mut buf).await.is_err() {
            // If we mis-detected, fall through to other strategies.
            break;
        }
        let (msg, encoding) = decode_packet_text(buf)?;
        return Ok(TcpPacket {
            text: msg,
            framing,
            encoding,
        });
    }

    // Try a 2-byte length prefix (some PaperConnect implementations use u16 length).
    // We already consumed 4 bytes: [len16][first 2 bytes of payload]. Read the rest.
    let payload_start_looks_texty = {
        let b0 = hdr[2];
        let b1 = hdr[3];
        (b0.is_ascii_alphabetic() && b1 == b':') || matches!(b0, b'{' | b'[' | b'"')
    };
    let len16_le = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
    let len16_be = u16::from_be_bytes([hdr[0], hdr[1]]) as usize;
    let len16_le_ok = (1..=MAX_PACKET_SIZE).contains(&len16_le);
    let len16_be_ok = (1..=MAX_PACKET_SIZE).contains(&len16_be);

    let mut try_u16 = Vec::new();
    if payload_start_looks_texty && len16_le_ok {
        try_u16.push((len16_le, TcpFraming::LengthPrefixedU16Le));
    }
    if payload_start_looks_texty && len16_be_ok && len16_be != len16_le {
        try_u16.push((len16_be, TcpFraming::LengthPrefixedU16Be));
    }

    for (len, framing) in try_u16 {
        if len < 2 {
            continue;
        }
        let already = &hdr[2..4];
        let mut buf = Vec::with_capacity(len);
        buf.extend_from_slice(already);
        let remaining = len.saturating_sub(already.len());
        if remaining > 0 {
            let mut rest = vec![0u8; remaining];
            if stream.read_exact(&mut rest).await.is_err() {
                break;
            }
            buf.extend_from_slice(&rest);
        }
        let (msg, encoding) = decode_packet_text(buf)?;
        return Ok(TcpPacket {
            text: msg,
            framing,
            encoding,
        });
    }

    fn json_readiness(bytes: &[u8]) -> Result<bool, serde_json::Error> {
        match serde_json::from_slice::<Value>(bytes) {
            Ok(_) => Ok(true),
            Err(e) if e.is_eof() => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn paperconnect_readiness(buf: &[u8]) -> Result<bool, serde_json::Error> {
        if let Some(i) = buf.iter().position(|b| *b == 0) {
            let json = &buf[i + 1..];
            json_readiness(json)
        } else if matches!(buf.first().copied(), Some(b'{' | b'[' | b'"')) {
            json_readiness(buf)
        } else {
            // Likely a `proto\0json` request where we haven't received the separator yet.
            Ok(false)
        }
    }

    // Not a plausible length prefix: treat the 4 bytes as part of an unframed text message.
    prebuf.extend_from_slice(&hdr);
    let mut tmp = [0u8; 4096];
    let mut framing = TcpFraming::Raw;
    loop {
        if looks_like_zstd_frame(&prebuf) {
            if let Ok(out) = zstd_decompress_limited(&prebuf, MAX_PACKET_SIZE) {
                match paperconnect_readiness(&out) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => return Err(anyhow!(e).context("invalid json packet")),
                }
            }
        } else {
            match paperconnect_readiness(&prebuf) {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => return Err(anyhow!(e).context("invalid json packet")),
            }
        }

        let n = stream.read(&mut tmp).await.context("read tcp")?;
        if n == 0 {
            break;
        }
        prebuf.extend_from_slice(&tmp[..n]);
        if prebuf.len() > MAX_PACKET_SIZE {
            return Err(anyhow!("packet too large"));
        }
        if !looks_like_zstd_frame(&prebuf) && prebuf.contains(&b'\n') {
            framing = TcpFraming::LineDelimited;
            break;
        }
    }
    if framing == TcpFraming::LineDelimited {
        if let Some(i) = prebuf.iter().position(|b| *b == b'\n') {
            prebuf.truncate(i);
        }
    }
    let (msg, encoding) = decode_packet_text(prebuf)?;
    Ok(TcpPacket {
        text: msg,
        framing,
        encoding,
    })
}

async fn write_packet_bytes(
    stream: &mut TcpStream,
    bytes: &[u8],
    framing: TcpFraming,
) -> anyhow::Result<()> {
    match framing {
        TcpFraming::Raw => {
            stream.write_all(bytes).await.context("write tcp")?;
        }
        TcpFraming::LengthPrefixedLe => {
            let len = u32::try_from(bytes.len()).context("packet too large")?;
            stream
                .write_all(&len.to_le_bytes())
                .await
                .context("write tcp")?;
            stream.write_all(bytes).await.context("write tcp")?;
        }
        TcpFraming::LengthPrefixedBe => {
            let len = u32::try_from(bytes.len()).context("packet too large")?;
            stream
                .write_all(&len.to_be_bytes())
                .await
                .context("write tcp")?;
            stream.write_all(bytes).await.context("write tcp")?;
        }
        TcpFraming::LengthPrefixedU16Le => {
            let len = u16::try_from(bytes.len()).context("packet too large")?;
            stream
                .write_all(&len.to_le_bytes())
                .await
                .context("write tcp")?;
            stream.write_all(bytes).await.context("write tcp")?;
        }
        TcpFraming::LengthPrefixedU16Be => {
            let len = u16::try_from(bytes.len()).context("packet too large")?;
            stream
                .write_all(&len.to_be_bytes())
                .await
                .context("write tcp")?;
            stream.write_all(bytes).await.context("write tcp")?;
        }
        TcpFraming::LineDelimited => {
            stream.write_all(bytes).await.context("write tcp")?;
            if !bytes.ends_with(b"\n") {
                stream.write_all(b"\n").await.context("write tcp")?;
            }
        }
    }
    Ok(())
}

async fn write_packet(
    stream: &mut TcpStream,
    msg: &str,
    framing: TcpFraming,
) -> anyhow::Result<()> {
    write_packet_bytes(stream, msg.as_bytes(), framing).await
}

async fn write_packet_bytes_and_close(
    stream: &mut TcpStream,
    bytes: &[u8],
    framing: TcpFraming,
) -> anyhow::Result<()> {
    write_packet_bytes(stream, bytes, framing).await?;
    let _ = stream.shutdown().await;
    Ok(())
}

async fn write_packet_and_close(
    stream: &mut TcpStream,
    msg: &str,
    framing: TcpFraming,
) -> anyhow::Result<()> {
    write_packet_bytes_and_close(stream, msg.as_bytes(), framing).await
}

#[tauri::command]
pub async fn paperconnect_tcp_request(
    state: State<'_, OnlineState>,
    host: String,
    port: u16,
    proto: String,
    body: Value,
) -> Result<Value, String> {
    let host = host.trim().to_string();
    let addr = format!("{host}:{port}");
    let mut body = body;
    if proto == "c:player" || proto == "c:ping" {
        if proto == "c:player" && !body.is_object() {
            body = serde_json::json!({});
        }
        if let Some(obj) = body.as_object_mut() {
            let client_ok = obj
                .get("clientId")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !client_ok {
                obj.insert("clientId".to_string(), Value::String(default_client_id()));
            }
            let player_ok = obj
                .get("playerName")
                .and_then(|v| v.as_str())
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);
            if !player_ok {
                obj.insert(
                    "playerName".to_string(),
                    Value::String(default_player_name()),
                );
            }
        }
    }

    async fn attempt(
        addr: &str,
        proto: &str,
        body: &Value,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> anyhow::Result<Value> {
        let mut stream = tokio::time::timeout(connect_timeout, TcpStream::connect(addr))
            .await
            .context("connect timed out")?
            .context("connect tcp")?;
        let _ = stream.set_nodelay(true);
        let payload = format!("{proto}\0{}", body.to_string());
        write_packet(&mut stream, &payload, TcpFraming::Raw).await?;
        let pkt = tokio::time::timeout(read_timeout, read_one_packet(&mut stream))
            .await
            .context("read response timed out")??;
        let resp = pkt.text;
        let json = resp
            .split_once('\0')
            .map(|(_, j)| j)
            .unwrap_or(resp.as_str());
        let v: Value = serde_json::from_str(json).context("invalid json response")?;
        if let Some(err_msg) = v.get("error").and_then(|v| v.as_str()) {
            return Err(anyhow!("server error: {err_msg}"));
        }
        Ok(v)
    }

    async fn easytier_has_route_to_host(
        state: &OnlineState,
        host: &str,
    ) -> Option<bool> {
        let ip: IpAddr = host.parse().ok()?;
        if ip.is_loopback() {
            return None;
        }

        let id = state.easytier_instance_id.lock().unwrap().as_ref().copied()?;
        let svc = state.easytier_manager.get_instance_service(&id)?;
        let resp = svc
            .get_peer_manage_service()
            .list_route(BaseController::default(), ListRouteRequest::default())
            .await
            .ok()?;

        for r in resp.routes {
            let Some(inet) = r.ipv4_addr else { continue };
            let s = inet.to_string();
            let route_ip = s.split_once('/').map(|v| v.0).unwrap_or(s.as_str());
            if route_ip == host {
                return Some(true);
            }
        }
        Some(false)
    }

    // For PaperConnect, the overlay may come up slightly after the UI initiates the first request.
    // Instead of forcing the frontend to wait for the next 5s tick, do a short in-command retry
    // burst for the initial handshake. Once connected, the normal 5s heartbeat cadence is fine.
    //
    // Follow the spec: raw `namespace\0json` payload.
    let connect_timeout = Duration::from_millis(1500);
    let read_timeout = Duration::from_secs(3);

    let mut last_err: Option<anyhow::Error> = None;
    for (i, backoff_ms) in [0u64, 150, 350, 650, 1000].into_iter().enumerate() {
        if backoff_ms > 0 {
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }
        match attempt(&addr, &proto, &body, connect_timeout, read_timeout).await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = Some(e);
                // If the host route is already gone (e.g. room closed), fail fast so the
                // frontend can stop showing a "connecting" state.
                if matches!(
                    easytier_has_route_to_host(&state, &host).await,
                    Some(false)
                ) {
                    return Err("paperconnect host offline".to_string());
                }
                // If it's already been a few attempts, avoid spending too long here.
                if i >= 4 {
                    break;
                }
            }
        }
    }

    Err(format!(
        "paperconnect request failed: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    ))
}

#[tauri::command]
pub async fn paperconnect_server_start(
    state: State<'_, OnlineState>,
    args: PaperConnectServerStartArgs,
) -> Result<(), String> {
    {
        let mut g = state.paperconnect_server.lock().unwrap();
        if g.is_some() {
            return Err("PaperConnect server already running".to_string());
        }
    }

    let listener = TcpListener::bind(("0.0.0.0", args.listen_port))
        .await
        .map_err(|e| format!("bind failed: {e}"))?;

    let game_type = args.game_type.clone();
    let game_protocol_type = args.game_protocol_type.clone();
    let game_port = args.game_port;
    let listen_port = args.listen_port;

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let room_host_player_name = args
        .room_host_player_name
        .clone()
        .unwrap_or_else(|| "host".to_string());
    let room_host_client_id = args
        .room_host_client_id
        .clone()
        .unwrap_or_else(default_client_id);

    let state_inner = std::sync::Arc::new(tokio::sync::Mutex::new(PaperConnectServerState::new(
        game_type,
        game_protocol_type,
        game_port,
        room_host_player_name,
        room_host_client_id,
    )));

    let state_for_handle = state_inner.clone();

    let task = tokio::spawn(async move {
        let mut cleanup_tick = tokio::time::interval(Duration::from_secs(1));
        cleanup_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    break;
                }
                _ = cleanup_tick.tick() => {
                    let now = now_ms();
                    let mut st = state_inner.lock().await;
                    st.cleanup(now);
                }
                res = listener.accept() => {
                    let (mut stream, _) = match res {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let st = state_inner.clone();
                    tokio::spawn(async move {
                        let _ = handle_paperconnect_conn(&mut stream, st).await;
                    });
                }
            }
        }
    });

    *state.paperconnect_server.lock().unwrap() = Some(PaperConnectServerHandle {
        shutdown: shutdown_tx,
        task,
        listen_port,
        state: state_for_handle,
    });
    Ok(())
}

#[tauri::command]
pub async fn paperconnect_server_stop(state: State<'_, OnlineState>) -> Result<(), String> {
    let handle = state.paperconnect_server.lock().unwrap().take();
    if let Some(h) = handle {
        let _ = h.shutdown.send(());
        let _ = h.task.await;
    }
    Ok(())
}

#[tauri::command]
pub async fn paperconnect_server_state(
    state: State<'_, OnlineState>,
) -> Result<Option<PaperConnectServerSnapshot>, String> {
    let (listen_port, st) = {
        let g = state.paperconnect_server.lock().unwrap();
        let Some(handle) = g.as_ref() else {
            return Ok(None);
        };
        (handle.listen_port, handle.state.clone())
    };

    let now = now_ms();
    let mut inner = st.lock().await;
    inner.cleanup(now);

    let mut players: Vec<PaperConnectPlayerEntry> = inner
        .players
        .iter()
        .map(|(k, p)| PaperConnectPlayerEntry {
            player: p.player_name.clone(),
            client_id: p.client_id.clone(),
            is_room_host: k == &inner.room_host_key,
            first_seen_ms: p.first_seen_ms,
            last_seen_ms: p.last_seen_ms,
        })
        .collect();
    players.sort_by(|a, b| {
        b.is_room_host
            .cmp(&a.is_room_host)
            .then_with(|| b.last_seen_ms.cmp(&a.last_seen_ms))
            .then_with(|| a.client_id.cmp(&b.client_id))
    });

    Ok(Some(PaperConnectServerSnapshot {
        return_time: now,
        listen_port,
        game_port: inner.game_port,
        game_type: inner.game_type.clone(),
        game_protocol_type: inner.game_protocol_type.clone(),
        players,
    }))
}

struct PaperConnectPlayer {
    player_name: String,
    client_id: String,
    first_seen_ms: i64,
    last_seen_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PlayerKey {
    player_name: String,
    client_id: String,
}

struct PaperConnectServerState {
    game_type: String,
    game_protocol_type: String,
    game_port: u16,
    created_ms: i64,
    players: HashMap<PlayerKey, PaperConnectPlayer>, // (playerName, clientId) -> player
    room_host_key: PlayerKey,
}

impl PaperConnectServerState {
    fn new(
        game_type: String,
        game_protocol_type: String,
        game_port: u16,
        room_host_player_name: String,
        room_host_client_id: String,
    ) -> Self {
        let now = now_ms();
        let host_key = PlayerKey {
            player_name: room_host_player_name.clone(),
            client_id: room_host_client_id.clone(),
        };
        let mut players = HashMap::new();
        players.insert(
            host_key.clone(),
            PaperConnectPlayer {
                player_name: room_host_player_name,
                client_id: room_host_client_id,
                first_seen_ms: now,
                last_seen_ms: now,
            },
        );

        Self {
            game_type,
            game_protocol_type,
            game_port,
            created_ms: now,
            players,
            room_host_key: host_key,
        }
    }

    fn cleanup(&mut self, now: i64) {
        let timeout_ms = 10_000i64;
        // Never evict the room host entry; otherwise a brief heartbeat gap can reset the host's
        // `first_seen_ms` and make the UI look like the host is "disconnecting" repeatedly.
        self.players
            .retain(|k, p| k == &self.room_host_key || now - p.last_seen_ms <= timeout_ms);

        // Keep a stable room host identity for PaperConnect compatibility.
        if !self.players.contains_key(&self.room_host_key) {
            let host_key = self.room_host_key.clone();
            self.players.insert(
                host_key.clone(),
                PaperConnectPlayer {
                    player_name: host_key.player_name.clone(),
                    client_id: host_key.client_id.clone(),
                    // If the host entry had to be recreated for any reason, treat the host as
                    // online since room creation time to keep session duration stable.
                    first_seen_ms: self.created_ms,
                    last_seen_ms: now,
                },
            );
        }
    }
}

async fn handle_paperconnect_conn(
    stream: &mut TcpStream,
    state: std::sync::Arc<tokio::sync::Mutex<PaperConnectServerState>>,
) -> anyhow::Result<()> {
    let pkt = tokio::time::timeout(Duration::from_secs(5), read_one_packet(stream))
        .await
        .context("read request timed out")??;
    let msg = pkt.text;
    let framing = pkt.framing;
    let resp_encoding = effective_response_encoding(pkt.encoding, framing);
    let (proto, json) = if let Some(v) = msg.split_once('\0') {
        v
    } else if let Some(v) = msg.split_once('\n') {
        // Some third-party implementations accidentally use newline as a separator.
        v
    } else if let Some(i) = msg.find('{') {
        // Last-resort: accept `proto{json}` without an explicit separator.
        (&msg[..i], &msg[i..])
    } else {
        tracing::debug!("paperconnect: invalid request (missing protocol separator)");
        let resp = serde_json::json!({ "error": "missing protocol separator" });
        let bytes = resp.to_string().into_bytes();
        let bytes = match resp_encoding {
            TcpEncoding::Plain => bytes,
            TcpEncoding::Zstd => zstd_compress(&bytes)?,
        };
        write_packet_bytes_and_close(stream, &bytes, framing).await?;
        return Ok(());
    };
    let proto = proto.trim();
    let json = json.trim();

    match proto {
        "c:ping" => {
            #[derive(Deserialize)]
            struct PingReq {
                time: Option<i64>,
                #[serde(rename = "clientId", alias = "client_id")]
                client_id: Option<String>,
                #[serde(rename = "playerName", alias = "player_name", alias = "player")]
                player_name: Option<String>,
            }
            let req: PingReq = serde_json::from_str(json).context("invalid json")?;
            let time = req.time.unwrap_or(0);
            if let (Some(client_id), Some(player_name)) = (req.client_id, req.player_name) {
                let now = now_ms();
                let mut st = state.lock().await;
                let key = PlayerKey {
                    player_name: player_name.clone(),
                    client_id: client_id.clone(),
                };
                match st.players.get_mut(&key) {
                    Some(existing) => {
                        existing.player_name = player_name;
                        existing.client_id = client_id;
                        existing.last_seen_ms = now;
                    }
                    None => {
                        st.players.insert(
                            key,
                            PaperConnectPlayer {
                                player_name,
                                client_id,
                                first_seen_ms: now,
                                last_seen_ms: now,
                            },
                        );
                    }
                }
                st.cleanup(now);
            }
            let (game_type, game_protocol_type, game_port) = {
                let st = state.lock().await;
                (
                    st.game_type.clone(),
                    st.game_protocol_type.clone(),
                    st.game_port,
                )
            };
            let resp = serde_json::json!({
                "time": time,
                "returnTime": now_ms(),
                "gameType": game_type,
                "gameProtocolType": game_protocol_type,
                "gamePort": game_port
            });
            let bytes = resp.to_string().into_bytes();
            let bytes = match resp_encoding {
                TcpEncoding::Plain => bytes,
                TcpEncoding::Zstd => zstd_compress(&bytes)?,
            };
            write_packet_bytes_and_close(stream, &bytes, framing).await?;
        }
        "c:player" => {
            #[derive(Deserialize)]
            struct PlayerReq {
                #[serde(rename = "clientId", alias = "client_id")]
                client_id: String,
                #[serde(rename = "playerName", alias = "player_name", alias = "player")]
                player_name: String,
            }
            let req: PlayerReq = serde_json::from_str(json).context("invalid json")?;
            let now = now_ms();
            let mut st = state.lock().await;
            let key = PlayerKey {
                player_name: req.player_name.clone(),
                client_id: req.client_id.clone(),
            };
            match st.players.get_mut(&key) {
                Some(existing) => {
                    existing.player_name = req.player_name.clone();
                    existing.client_id = req.client_id.clone();
                    existing.last_seen_ms = now;
                }
                None => {
                    st.players.insert(
                        key,
                        PaperConnectPlayer {
                            player_name: req.player_name.clone(),
                            client_id: req.client_id.clone(),
                            first_seen_ms: now,
                            last_seen_ms: now,
                        },
                    );
                }
            }
            st.cleanup(now);
            let host_key = st.room_host_key.clone();
            let players: Vec<Value> = st
                .players
                .iter()
                .map(|(k, p)| {
                    serde_json::json!({
                        "player": p.player_name,
                        "clientId": p.client_id,
                        "isRoomHost": k == &host_key,
                        "firstSeenMs": p.first_seen_ms,
                        "lastSeenMs": p.last_seen_ms
                    })
                })
                .collect();
            let resp = serde_json::json!({
                "returnTime": now,
                "players": players
            });
            let bytes = resp.to_string().into_bytes();
            let bytes = match resp_encoding {
                TcpEncoding::Plain => bytes,
                TcpEncoding::Zstd => zstd_compress(&bytes)?,
            };
            write_packet_bytes_and_close(stream, &bytes, framing).await?;
        }
        _ => {
            let resp = serde_json::json!({ "error": "unknown protocol" });
            let bytes = resp.to_string().into_bytes();
            let bytes = match resp_encoding {
                TcpEncoding::Plain => bytes,
                TcpEncoding::Zstd => zstd_compress(&bytes)?,
            };
            write_packet_bytes_and_close(stream, &bytes, framing).await?;
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn easytier_embedded_nat_types(
    state: State<'_, OnlineState>,
) -> Result<Option<EasyTierNatTypeSnapshot>, String> {
    async fn detect_stun_nat_types() -> EasyTierNatTypeSnapshot {
        tokio::task::spawn_blocking(|| {
            let collector = StunInfoCollector::new_with_default_servers();
            collector.update_stun_info();

            let deadline = std::time::Instant::now() + Duration::from_secs(6);
            let mut last = collector.get_stun_info();
            while std::time::Instant::now() < deadline {
                last = collector.get_stun_info();
                if last.udp_nat_type != 0 || last.tcp_nat_type != 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }

            EasyTierNatTypeSnapshot {
                udp_nat_type: last.udp_nat_type,
                tcp_nat_type: last.tcp_nat_type,
            }
        })
        .await
        .unwrap_or(EasyTierNatTypeSnapshot {
            udp_nat_type: 0,
            tcp_nat_type: 0,
        })
    }

    let id_opt = { *state.easytier_instance_id.lock().unwrap() };
    let id = match id_opt {
        Some(v) => v,
        None => return Ok(Some(detect_stun_nat_types().await)),
    };

    let svc = match state.easytier_manager.get_instance_service(&id) {
        Some(v) => v,
        None => return Ok(Some(detect_stun_nat_types().await)),
    };

    let resp = svc
        .get_peer_manage_service()
        .show_node_info(
            BaseController::default(),
            easytier::proto::api::instance::ShowNodeInfoRequest::default(),
        )
        .await
        .map_err(|e| format!("show_node_info failed: {e}"))?;

    let node = match resp.node_info {
        Some(v) => v,
        None => return Ok(Some(detect_stun_nat_types().await)),
    };
    let stun = node.stun_info.unwrap_or_default();

    Ok(Some(EasyTierNatTypeSnapshot {
        udp_nat_type: stun.udp_nat_type,
        tcp_nat_type: stun.tcp_nat_type,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_roundtrip_decode_packet_text() {
        let msg = "c:ping\0{\"time\":1}";
        let compressed = zstd_compress(msg.as_bytes()).expect("compress");
        assert!(looks_like_zstd_frame(&compressed));
        let (out, enc) = decode_packet_text(compressed).expect("decode");
        assert_eq!(enc, TcpEncoding::Zstd);
        assert_eq!(out, msg);
    }

    #[test]
    fn zstd_decompress_limit_enforced() {
        let big = vec![b'a'; MAX_PACKET_SIZE + 1];
        let compressed = zstd_compress(&big).expect("compress");
        let err = zstd_decompress_limited(&compressed, MAX_PACKET_SIZE).expect_err("should fail");
        assert!(err.to_string().contains("too large"));
    }
}
