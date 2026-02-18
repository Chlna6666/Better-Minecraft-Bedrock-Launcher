use crate::http::proxy::{build_no_proxy_client_with_resolve, get_no_proxy_client};
use crate::utils::app_info;
use crate::utils::cloudflare;
use once_cell::sync::Lazy;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

const STATS_INGEST_URL: &str = "https://stats.bmcbl.com/v1/ingest?key=X9Q4M3T8V2K7";
const CLIENT_ID_SALT: &str = "bmcbl-stats-clientid-v1";
const STATS_HOST: &str = "stats.bmcbl.com";

static REPORTED_ONCE: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsIngestPayload {
    app_version: String,
    os: String,
    client_id: String,
}

pub fn spawn_startup_ingest() {
    // Best-effort fire-and-forget; never block startup.
    tauri::async_runtime::spawn(async move {
        match report_startup_ingest_once().await {
            Ok(_) => debug!("stats ingest task finished"),
            Err(e) => warn!("stats ingest task failed: {e:#}"),
        }
    });
}

async fn report_startup_ingest_once() -> anyhow::Result<()> {
    if REPORTED_ONCE.swap(true, Ordering::SeqCst) {
        debug!("stats ingest skipped: already reported");
        return Ok(());
    }

    let payload = StatsIngestPayload {
        app_version: app_info::get_version().to_string(),
        os: detect_os_string(),
        client_id: compute_client_id(),
    };

    let start = Instant::now();
    debug!(
        "stats ingest start: appVersion={}, os={}, clientIdPrefix={}",
        payload.app_version,
        payload.os,
        payload.client_id.chars().take(12).collect::<String>()
    );

    let client = match cloudflare::get_optimized_ip().await {
        Some(ip) => {
            debug!("stats ingest: using optimized IP {} for {}", ip, STATS_HOST);
            build_no_proxy_client_with_resolve(STATS_HOST, ip)
        }
        None => {
            debug!("stats ingest: no optimized IP, using default no-proxy client");
            get_no_proxy_client()
        }
    };

    debug!("stats ingest: sending request (timeout=5s)");
    let resp = client
        .post(STATS_INGEST_URL)
        .timeout(Duration::from_secs(5))
        .json(&payload)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!(e).context("stats ingest request failed"))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_else(|_| "".to_string());
    let body_preview = if body.len() > 800 {
        format!("{}...", &body[..800])
    } else {
        body
    };

    // Don't log the full URL (contains key).
    if !status.is_success() {
        warn!(
            "stats ingest failed: status={}, elapsedMs={}, respBody={}",
            status,
            start.elapsed().as_millis(),
            body_preview
        );
    } else {
        debug!(
            "stats ingest ok: status={}, elapsedMs={}, respBody={}",
            status,
            start.elapsed().as_millis(),
            body_preview
        );
    }

    Ok(())
}

fn compute_client_id() -> String {
    let device = device_code();
    let mut hasher = Sha256::new();
    hasher.update(CLIENT_ID_SALT.as_bytes());
    hasher.update(b":");
    hasher.update(device.as_bytes());
    let digest = hasher.finalize();
    hex::encode(digest)
}

fn device_code() -> String {
    if let Some(v) = platform_device_code() {
        return v;
    }

    // Stable per-install fallback: persist a random UUID locally.
    let path = Path::new("./BMCBL/client_id");
    if let Ok(s) = fs::read_to_string(path) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return format!("install:{}", s);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = fs::create_dir_all("./BMCBL");
    let _ = fs::write(path, &id);
    format!("install:{}", id)
}

#[cfg(target_os = "windows")]
fn platform_device_code() -> Option<String> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm
        .open_subkey_with_flags("SOFTWARE\\Microsoft\\Cryptography", KEY_READ | KEY_WOW64_64KEY)
        .ok()?;
    let guid: String = key.get_value("MachineGuid").ok()?;
    let guid = guid.trim().to_string();
    if guid.is_empty() {
        None
    } else {
        Some(format!("win:{}", guid))
    }
}

#[cfg(not(target_os = "windows"))]
fn platform_device_code() -> Option<String> {
    if let Ok(s) = fs::read_to_string("/etc/machine-id") {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Some(format!("linux:{}", s));
        }
    }
    None
}

fn detect_os_string() -> String {
    #[cfg(target_os = "windows")]
    {
        if let Some(s) = windows_os_string() {
            return s;
        }
    }

    let long = sysinfo::System::long_os_version()
        .or_else(sysinfo::System::os_version)
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    long
}

#[cfg(target_os = "windows")]
fn windows_os_string() -> Option<String> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm
        .open_subkey_with_flags(
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
            KEY_READ | KEY_WOW64_64KEY,
        )
        .ok()?;

    let product: String = key.get_value("ProductName").unwrap_or_default();
    let build: String = key
        .get_value("CurrentBuildNumber")
        .or_else(|_| key.get_value("CurrentBuild"))
        .unwrap_or_default();

    let product_trim = product.trim();
    let build_trim = build.trim();
    if build_trim.is_empty() {
        return None;
    }

    let name = if product_trim.contains("Windows 11") {
        "Windows 11"
    } else if product_trim.contains("Windows 10") {
        "Windows 10"
    } else if product_trim.is_empty() {
        "Windows"
    } else {
        product_trim
    };

    Some(format!("{name} Build {build_trim}"))
}
