use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use once_cell::sync::{Lazy, OnceCell};
use sonde_sdk::{ErrorEvent, Event, SondeClient};
use tracing::{debug, warn};

use crate::config::config::Config;
use crate::utils::app_info;

static CLIENT: OnceCell<SondeClient> = OnceCell::new();
static INITIALIZING: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));

pub fn spawn_startup_telemetry(config: &Config) -> Result<(), String> {
    if !config.launcher.stats_upload {
        debug!("telemetry skipped: stats_upload is disabled");
        return Ok(());
    }

    if INITIALIZING.swap(true, Ordering::SeqCst) {
        debug!("telemetry skipped: already initializing");
        return Ok(());
    }

    let config_clone = config.clone();
    crate::tasks::runtime::spawn_io(async move {
        match init_telemetry(&config_clone).await {
            Ok(_) => debug!("telemetry client connected and startup event sent"),
            Err(error) => warn!("telemetry initialization failed: {error}"),
        }
    })?;
    Ok(())
}

pub(crate) fn resolve_telemetry_language(config: &Config) -> String {
    let configured = config.launcher.language.trim();
    if configured.eq_ignore_ascii_case("auto") || configured.is_empty() {
        crate::utils::system_info::get_system_language()
    } else {
        configured.to_string()
    }
}

async fn init_telemetry(config: &Config) -> Result<(), String> {
    let device_id = resolve_device_id()?;
    let endpoint = crate::config::config::resolved_telemetry_endpoint(&config.launcher);
    let api_key = crate::config::config::resolved_telemetry_key(&config.launcher);

    let language = resolve_telemetry_language(config);
    let client = SondeClient::builder(endpoint, api_key, device_id)
        .app_version(app_info::get_version())
        .os(detect_os_string())
        .system_language(&language)
        .architecture(crate::utils::system_info::get_cpu_architecture())
        .connect()
        .await
        .map_err(|e| format!("failed to connect sonde client: {e}"))?;

    let mut startup_event = Event::new("app_startup");
    if config.launcher.language.trim().eq_ignore_ascii_case("auto") {
        startup_event.attributes.insert(
            "language_mode".to_string(),
            serde_json::Value::String("auto".to_string()),
        );
    }
    if let Err(e) = client.event(startup_event).await {
        warn!("failed to send telemetry app_startup event: {e}");
    }

    let _ = CLIENT.set(client);
    Ok(())
}

fn resolve_device_id() -> Result<String, String> {
    sonde_sdk::machine_device_id(Some("bmcbl"))
        .map_err(|e| format!("failed to detect machine device ID: {e}"))
}

#[allow(dead_code)]
pub fn client() -> Option<&'static SondeClient> {
    CLIENT.get()
}

#[allow(dead_code)]
pub fn track_event(event: Event) {
    if let Some(client) = client() {
        let client = client.clone();
        let _ = crate::tasks::runtime::spawn_io(async move {
            if let Err(e) = client.event(event).await {
                debug!("failed to enqueue telemetry event: {e}");
            }
        });
    }
}

#[allow(dead_code)]
pub fn notify_language_changed(new_language: &str) {
    if let Some(client) = client() {
        let client = client.clone();
        let resolved =
            if new_language.trim().eq_ignore_ascii_case("auto") || new_language.trim().is_empty() {
                crate::utils::system_info::get_system_language()
            } else {
                new_language.trim().to_string()
            };
        let _ = crate::tasks::runtime::spawn_io(async move {
            let facts = sonde_sdk::DeviceFacts {
                app_version: Some(app_info::get_version().to_string()),
                os: Some(detect_os_string()),
                system_language: Some(resolved),
                architecture: Some(crate::utils::system_info::get_cpu_architecture()),
            };
            if let Err(e) = client.set_device_facts(facts).await {
                debug!("failed to update telemetry device facts on language change: {e}");
            }
        });
    }
}

#[allow(dead_code)]
pub fn track_error(error: ErrorEvent) {
    if let Some(client) = client() {
        let client = client.clone();
        let _ = crate::tasks::runtime::spawn_io(async move {
            if let Err(e) = client.error(error).await {
                debug!("failed to enqueue telemetry error: {e}");
            }
        });
    }
}

pub fn shutdown() {
    if let Some(client) = CLIENT.get() {
        debug!("shutting down telemetry client");
        let client = client.clone();
        if let Ok(runtime) = crate::tasks::runtime::app_runtime() {
            let _ = runtime.block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(2), client.shutdown()).await;
            });
        }
    }
}

fn detect_os_string() -> String {
    #[cfg(target_os = "windows")]
    {
        if let Some(s) = windows_os_string() {
            return s;
        }
    }

    sysinfo::System::long_os_version()
        .or_else(sysinfo::System::os_version)
        .unwrap_or_else(|| std::env::consts::OS.to_string())
}

#[cfg(target_os = "windows")]
fn windows_os_string() -> Option<String> {
    let os_ver = sysinfo::System::os_version();
    let kernel = sysinfo::System::kernel_version();
    format_windows_os(os_ver.as_deref(), kernel.as_deref())
}

#[cfg(target_os = "windows")]
fn format_windows_os(os_version: Option<&str>, kernel_version: Option<&str>) -> Option<String> {
    fn first_number(s: &str) -> Option<String> {
        let mut started = false;
        let mut out = String::new();
        for ch in s.chars() {
            if ch.is_ascii_digit() {
                started = true;
                out.push(ch);
            } else if started {
                break;
            }
        }
        if out.is_empty() { None } else { Some(out) }
    }

    let os_version = os_version?.trim();
    if os_version.is_empty() {
        return None;
    }

    let major = first_number(os_version)?;
    let build = kernel_version
        .and_then(|s| first_number(s.trim()))
        .or_else(|| {
            let start = os_version.find('(')?;
            let end = os_version[start..].find(')')? + start;
            first_number(os_version.get(start + 1..end)?.trim())
        });

    Some(match build {
        Some(b) => format!("Windows {major} Build {b}"),
        None => format!("Windows {major}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_machine_device_id_is_stable_and_in_memory() {
        let id1 = resolve_device_id().expect("detect machine device id");
        assert_eq!(id1.len(), 64);
        let id2 = resolve_device_id().expect("detect machine device id");
        assert_eq!(id1, id2, "device ID must be stable across multiple calls");
    }

    #[test]
    fn test_detect_os_string_nonempty() {
        let os = detect_os_string();
        assert!(!os.is_empty());
    }

    #[test]
    fn test_disabled_telemetry_does_not_spawn() {
        let mut config = crate::config::config::get_default_config();
        config.launcher.stats_upload = false;

        let result = spawn_startup_telemetry(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_telemetry_language_resolves_auto() {
        let mut config = crate::config::config::get_default_config();
        config.launcher.language = "auto".to_string();
        let resolved = resolve_telemetry_language(&config);
        assert_ne!(resolved, "auto");
        assert!(!resolved.is_empty());

        config.launcher.language = "zh-TW".to_string();
        assert_eq!(resolve_telemetry_language(&config), "zh-TW");
    }
}
