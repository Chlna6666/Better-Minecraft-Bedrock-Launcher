#[path = "bedrock_auth.rs"]
mod managed;

pub(crate) use managed::{AuthPhase, AuthSnapshot, XboxProfile};
#[cfg(target_os = "linux")]
pub(crate) use managed::PreparedLaunchAuth;
#[cfg(target_os = "windows")]
pub(crate) use managed::PreparedLaunchAuth;

use once_cell::sync::Lazy;
#[cfg(target_os = "windows")]
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;
use tokio::sync::watch;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::WatchStream;

pub(crate) const SYSTEM_LOCAL_ACCOUNT_ID: &str = "local-system-xbox-user";
const ACCOUNT_MODE_FILE: &str = "xbox-account-mode";
const ACCOUNT_MODE_SYSTEM: &str = "system";
const SELECTION_AUTO: u8 = 0;
const SELECTION_MANAGED: u8 = 1;
const SELECTION_SYSTEM: u8 = 2;

static EVENT_BRIDGE_STARTED: AtomicBool = AtomicBool::new(false);
static LOCAL_PROBE_STARTED: AtomicBool = AtomicBool::new(false);
static PRELOAD_STARTED: AtomicBool = AtomicBool::new(false);
static SELECTION: AtomicU8 = AtomicU8::new(SELECTION_AUTO);
static AUTH_STATE: Lazy<(watch::Sender<AuthSnapshot>, watch::Receiver<AuthSnapshot>)> =
    Lazy::new(|| watch::channel(managed::AuthSnapshot::signed_out()));
static LATEST_MANAGED: Lazy<Mutex<AuthSnapshot>> =
    Lazy::new(|| Mutex::new(managed::AuthSnapshot::signed_out()));

#[derive(Clone, Debug)]
struct LocalAccountState {
    profile: XboxProfile,
    signed_in: bool,
    detail: String,
}

impl LocalAccountState {
    fn checking() -> Self {
        Self {
            profile: XboxProfile {
                xuid: SYSTEM_LOCAL_ACCOUNT_ID.to_string(),
                gamertag: "正在检测".to_string(),
                display_name: "系统本地账号".to_string(),
                gamerscore: None,
                avatar_url: None,
            },
            signed_in: false,
            detail: "正在读取 Windows 系统 Xbox 用户".to_string(),
        }
    }

    fn signed_out(detail: impl Into<String>) -> Self {
        Self {
            profile: XboxProfile {
                xuid: SYSTEM_LOCAL_ACCOUNT_ID.to_string(),
                gamertag: "未登录".to_string(),
                display_name: "系统本地账号".to_string(),
                gamerscore: None,
                avatar_url: None,
            },
            signed_in: false,
            detail: detail.into(),
        }
    }

    #[cfg(target_os = "windows")]
    fn signed_in(gamertag: String, avatar_path: Option<PathBuf>) -> Self {
        Self {
            profile: XboxProfile {
                xuid: SYSTEM_LOCAL_ACCOUNT_ID.to_string(),
                display_name: format!("{gamertag} · 系统本地"),
                gamertag,
                gamerscore: None,
                avatar_url: avatar_path.map(|path| path.to_string_lossy().into_owned()),
            },
            signed_in: true,
            detail: "Windows 系统 Xbox 用户已登录".to_string(),
        }
    }
}

static LOCAL_ACCOUNT: Lazy<Mutex<LocalAccountState>> =
    Lazy::new(|| Mutex::new(LocalAccountState::checking()));

pub(crate) fn event_stream() -> WatchStream<AuthSnapshot> {
    WatchStream::new(AUTH_STATE.1.clone())
}

/// Schedules all Xbox account startup work and returns immediately.
///
/// Managed account restoration and the Windows system-local account probe are
/// independent tasks. Both are submitted during startup, before GPUI begins,
/// while every blocking Gaming Runtime, keyring, image and filesystem operation
/// remains on the IO/blocking runtime.
pub(crate) fn preload_at_app_startup() {
    if PRELOAD_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }

    load_selection_mode();
    start_managed_event_bridge();

    // Submit the independent Windows probe first so Runtime initialization can
    // overlap keyring access and managed-token restoration.
    #[cfg(target_os = "windows")]
    start_local_user_probe();

    // This function only schedules managed restoration; it does not block the
    // startup thread while refreshing Microsoft/Xbox credentials.
    managed::initialize();
}

pub(crate) fn start_login() -> Result<(), String> {
    select_managed_account();
    managed::start_login()
}

pub(crate) fn cancel_login() {
    managed::cancel_login();
}

pub(crate) fn switch_account(account_id: String) -> Result<(), String> {
    if is_system_local_account(&account_id) {
        SELECTION.store(SELECTION_SYSTEM, Ordering::Release);
        persist_selection_mode(true);
        publish_latest_managed();
        tracing::info!(
            "已选择 Windows 系统本地 Xbox 账号；启动游戏时不会创建 BLoader XUser 安全管道"
        );
        return Ok(());
    }

    select_managed_account();
    managed::switch_account(account_id)
}

pub(crate) fn remove_account(account_id: String) -> Result<(), String> {
    if is_system_local_account(&account_id) {
        return Err("系统本地 Xbox 账号由 Windows 管理，不能从 BMCBL 删除".to_string());
    }
    managed::remove_account(account_id)
}

#[cfg(target_os = "linux")]
pub(crate) async fn prepare_launch(
    prefix_path: &Path,
) -> Result<Option<PreparedLaunchAuth>, String> {
    managed::prepare_launch(prefix_path).await
}

#[cfg(target_os = "windows")]
pub(crate) async fn prepare_launch_windows() -> Result<Option<PreparedLaunchAuth>, String> {
    let latest = latest_managed_snapshot();
    if system_account_is_selected(&latest) {
        let local = local_account_snapshot();
        tracing::info!(
            xbox_gamertag = %local.profile.gamertag,
            system_signed_in = local.signed_in,
            "当前选择系统本地 Xbox 账号；跳过 BMCBL 预认证与安全管道，游戏使用微软官方 XUser 登录"
        );
        return Ok(None);
    }
    managed::prepare_launch_windows().await
}

pub(crate) fn is_system_local_account(account_id: &str) -> bool {
    account_id == SYSTEM_LOCAL_ACCOUNT_ID
}

fn start_managed_event_bridge() {
    if EVENT_BRIDGE_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut events = managed::event_stream();
    let result = crate::tasks::runtime::spawn_io(async move {
        while let Some(snapshot) = events.next().await {
            if let Ok(mut latest) = LATEST_MANAGED.lock() {
                *latest = snapshot.clone();
            }
            publish_with_local_account(snapshot);
        }
    });
    if let Err(error) = result {
        EVENT_BRIDGE_STARTED.store(false, Ordering::Release);
        tracing::warn!(%error, "无法启动 Xbox 托管账号状态桥接");
    }
}

#[cfg(target_os = "windows")]
fn start_local_user_probe() {
    if LOCAL_PROBE_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let result = crate::tasks::runtime::spawn_io(async move {
        let probe = crate::tasks::runtime::run_io_blocking(|| {
            let probe = crate::core::system_xbox_user::probe_default_user();
            match probe {
                crate::core::system_xbox_user::SystemXboxUserProbe::SignedIn(user) => {
                    let avatar_path = user
                        .gamer_picture_png
                        .as_deref()
                        .and_then(|bytes| cache_system_gamer_picture(user.xuid, bytes).ok())
                        .or_else(|| cached_system_gamer_picture(user.xuid));
                    LocalAccountState::signed_in(user.gamertag, avatar_path)
                }
                crate::core::system_xbox_user::SystemXboxUserProbe::SignedOut { hresult } => {
                    let detail = hresult.map_or_else(
                        || "Windows 当前没有已登录的默认 Xbox 用户".to_string(),
                        |status| {
                            format!(
                                "Windows 当前没有可静默取得的默认 Xbox 用户（HRESULT=0x{:08X}）",
                                status as u32
                            )
                        },
                    );
                    LocalAccountState::signed_out(detail)
                }
                crate::core::system_xbox_user::SystemXboxUserProbe::Unavailable { reason } => {
                    LocalAccountState::signed_out(format!("系统 Xbox 用户读取不可用：{reason}"))
                }
            }
        })
        .await;

        let local = match probe {
            Ok(local) => local,
            Err(error) => LocalAccountState::signed_out(format!("本地用户后台任务失败：{error}")),
        };
        tracing::info!(
            xbox_gamertag = %local.profile.gamertag,
            system_signed_in = local.signed_in,
            detail = %local.detail,
            "Windows 系统本地 Xbox 用户探测完成"
        );
        if let Ok(mut current) = LOCAL_ACCOUNT.lock() {
            *current = local;
        }
        publish_latest_managed();
    });
    if let Err(error) = result {
        LOCAL_PROBE_STARTED.store(false, Ordering::Release);
        tracing::warn!(%error, "无法启动 Windows 系统 Xbox 用户探测");
    }
}

fn publish_latest_managed() {
    publish_with_local_account(latest_managed_snapshot());
}

fn latest_managed_snapshot() -> AuthSnapshot {
    LATEST_MANAGED
        .lock()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

fn local_account_snapshot() -> LocalAccountState {
    LOCAL_ACCOUNT
        .lock()
        .map(|account| account.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

fn publish_with_local_account(mut snapshot: AuthSnapshot) {
    #[cfg(target_os = "windows")]
    {
        let select_system = system_account_is_selected(&snapshot);
        let local = local_account_snapshot();
        snapshot
            .accounts
            .retain(|profile| !is_system_local_account(&profile.xuid));
        snapshot.accounts.insert(0, local.profile.clone());

        if select_system {
            snapshot.profile = Some(local.profile.clone());
            snapshot.active_account_id = Some(SYSTEM_LOCAL_ACCOUNT_ID.to_string());
            snapshot.phase = AuthPhase::SignedIn;
            snapshot.user_code = None;
            snapshot.verification_url = None;
            snapshot.error = None;
        }
    }
    AUTH_STATE.0.send_replace(snapshot);
}

fn system_account_is_selected(snapshot: &AuthSnapshot) -> bool {
    match SELECTION.load(Ordering::Acquire) {
        SELECTION_SYSTEM => true,
        SELECTION_MANAGED => false,
        _ => {
            snapshot.profile.is_none()
                && snapshot
                    .accounts
                    .iter()
                    .all(|profile| is_system_local_account(&profile.xuid))
        }
    }
}

fn select_managed_account() {
    SELECTION.store(SELECTION_MANAGED, Ordering::Release);
    persist_selection_mode(false);
}

fn account_mode_path() -> PathBuf {
    crate::utils::file_ops::config_dir().join(ACCOUNT_MODE_FILE)
}

fn load_selection_mode() {
    #[cfg(target_os = "windows")]
    {
        let mode = std::fs::read_to_string(account_mode_path()).unwrap_or_default();
        if mode.trim().eq_ignore_ascii_case(ACCOUNT_MODE_SYSTEM) {
            SELECTION.store(SELECTION_SYSTEM, Ordering::Release);
        }
    }
}

fn persist_selection_mode(system: bool) {
    #[cfg(target_os = "windows")]
    {
        let path = account_mode_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if system {
            if let Err(error) = std::fs::write(&path, format!("{ACCOUNT_MODE_SYSTEM}\n")) {
                tracing::debug!(%error, "无法保存系统 Xbox 账号选择状态");
            }
        } else if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(%error, "无法清除系统 Xbox 账号选择状态");
        }
    }
}

#[cfg(target_os = "windows")]
fn cache_system_gamer_picture(xuid: u64, source: &[u8]) -> Result<PathBuf, String> {
    if source.is_empty() || source.len() > 8 * 1024 * 1024 {
        return Err("系统 Xbox 头像为空或超过大小限制".to_string());
    }
    let decoded = image::load_from_memory(source)
        .map_err(|error| format!("解码系统 Xbox 头像失败：{error}"))?;
    let normalized = decoded.thumbnail(256, 256);
    let mut png = Vec::new();
    normalized
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| format!("编码系统 Xbox 头像失败：{error}"))?;

    use sha2::{Digest as _, Sha256};
    let digest = hex::encode(Sha256::digest(&png));
    let file_name = format!("{xuid}-{}.png", &digest[..16]);
    let cache_dir = crate::utils::file_ops::cache_subdir("xbox/avatars");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("创建系统 Xbox 头像缓存目录失败：{error}"))?;
    let final_path = cache_dir.join(&file_name);
    if !final_path.is_file() {
        let temporary = cache_dir.join(format!(
            ".{file_name}.{}.tmp",
            uuid::Uuid::new_v4().as_simple()
        ));
        std::fs::write(&temporary, &png)
            .map_err(|error| format!("写入系统 Xbox 头像临时文件失败：{error}"))?;
        if let Err(error) = std::fs::rename(&temporary, &final_path) {
            let _ = std::fs::remove_file(&temporary);
            if !final_path.is_file() {
                return Err(format!("提交系统 Xbox 头像缓存失败：{error}"));
            }
        }
    }

    let pointer = cache_dir.join(format!("{xuid}.current"));
    let pointer_tmp = cache_dir.join(format!(
        ".{xuid}.{}.current.tmp",
        uuid::Uuid::new_v4().as_simple()
    ));
    std::fs::write(&pointer_tmp, format!("{file_name}\n"))
        .map_err(|error| format!("写入系统 Xbox 头像索引失败：{error}"))?;
    if pointer.exists() {
        let _ = std::fs::remove_file(&pointer);
    }
    std::fs::rename(&pointer_tmp, &pointer).map_err(|error| {
        let _ = std::fs::remove_file(&pointer_tmp);
        format!("提交系统 Xbox 头像索引失败：{error}")
    })?;
    Ok(final_path)
}

#[cfg(target_os = "windows")]
fn cached_system_gamer_picture(xuid: u64) -> Option<PathBuf> {
    let cache_dir = crate::utils::file_ops::cache_subdir("xbox/avatars");
    let name = std::fs::read_to_string(cache_dir.join(format!("{xuid}.current"))).ok()?;
    let name = name.trim();
    let expected_prefix = format!("{xuid}-");
    if name.is_empty()
        || !name.starts_with(&expected_prefix)
        || !name.ends_with(".png")
        || Path::new(name).components().count() != 1
    {
        return None;
    }
    let path = cache_dir.join(name);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_account_id_is_reserved() {
        assert!(is_system_local_account(SYSTEM_LOCAL_ACCOUNT_ID));
        assert!(!is_system_local_account("123456789"));
    }

    #[test]
    fn automatic_selection_ignores_the_synthetic_local_row() {
        SELECTION.store(SELECTION_AUTO, Ordering::Release);
        let mut snapshot = managed::AuthSnapshot::signed_out();
        snapshot.accounts.push(LocalAccountState::signed_out("test").profile);
        assert!(system_account_is_selected(&snapshot));
    }
}