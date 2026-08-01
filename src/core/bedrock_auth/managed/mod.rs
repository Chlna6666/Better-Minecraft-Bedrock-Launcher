#[cfg(target_os = "windows")]
mod gdk_bridge;
mod msa;
mod secret_store;
#[cfg(target_os = "linux")]
mod wine_bridge;
mod xbox;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::future::Future;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

#[cfg(target_os = "linux")]
pub(crate) use wine_bridge::PreparedLaunchAuth;

#[cfg(target_os = "windows")]
pub(crate) use gdk_bridge::PreparedLaunchAuth;

static AUTH_STATE: Lazy<(watch::Sender<AuthSnapshot>, watch::Receiver<AuthSnapshot>)> =
    Lazy::new(|| watch::channel(AuthSnapshot::signed_out()));
static ACCOUNT_LOCK: Lazy<std::sync::Mutex<()>> = Lazy::new(|| std::sync::Mutex::new(()));
static PREAUTH_CACHE: Lazy<std::sync::Mutex<Option<(String, Vec<u8>, std::time::Instant)>>> =
    Lazy::new(|| std::sync::Mutex::new(None));
static FLOW_GENERATION: AtomicU64 = AtomicU64::new(0);
static FLOW_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct XboxProfile {
    pub(crate) xuid: String,
    pub(crate) gamertag: String,
    pub(crate) display_name: String,
    pub(crate) gamerscore: Option<String>,
    pub(crate) avatar_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AuthPhase {
    SignedOut,
    Restoring,
    RequestingCode,
    WaitingForUser,
    AuthenticatingXbox,
    SwitchingAccount,
    SigningOut,
    SignedIn,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthSnapshot {
    pub(crate) phase: AuthPhase,
    pub(crate) profile: Option<XboxProfile>,
    pub(crate) accounts: Vec<XboxProfile>,
    pub(crate) active_account_id: Option<String>,
    pub(crate) user_code: Option<String>,
    pub(crate) verification_url: Option<String>,
    pub(crate) error: Option<String>,
}

impl AuthSnapshot {
    pub(crate) fn signed_out() -> Self {
        Self {
            phase: AuthPhase::SignedOut,
            profile: None,
            accounts: Vec::new(),
            active_account_id: None,
            user_code: None,
            verification_url: None,
            error: None,
        }
    }

    fn phase_from_current(phase: AuthPhase) -> Self {
        let mut snapshot = current_snapshot();
        snapshot.phase = phase;
        snapshot.user_code = None;
        snapshot.verification_url = None;
        snapshot.error = None;
        snapshot
    }

    fn waiting_from_current(code: &msa::DeviceCode) -> Self {
        let mut snapshot = Self::phase_from_current(AuthPhase::WaitingForUser);
        snapshot.user_code = Some(code.user_code.clone());
        snapshot.verification_url = Some(code.verification_url.clone());
        snapshot
    }

    fn from_catalog(
        phase: AuthPhase,
        catalog: &secret_store::AccountCatalog,
        profile: Option<XboxProfile>,
    ) -> Self {
        Self {
            phase,
            profile,
            accounts: catalog.profiles.clone(),
            active_account_id: catalog.active_account_id.clone(),
            user_code: None,
            verification_url: None,
            error: None,
        }
    }

    fn stable_from_current() -> Self {
        let mut snapshot = current_snapshot();
        snapshot.phase = if snapshot.profile.is_some() {
            AuthPhase::SignedIn
        } else {
            AuthPhase::SignedOut
        };
        snapshot.user_code = None;
        snapshot.verification_url = None;
        snapshot.error = None;
        snapshot
    }

    fn error_from_current(error: &AuthError) -> Self {
        let mut snapshot = Self::phase_from_current(AuthPhase::Error);
        snapshot.error = Some(error.user_message());
        snapshot
    }
}

struct StoredLoginCandidate {
    catalog: secret_store::AccountCatalog,
    profile: Option<XboxProfile>,
    refresh_token: Option<secrecy::SecretString>,
    legacy: bool,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum AuthError {
    #[error("Microsoft/Xbox 网络请求失败")]
    Http(#[source] reqwest::Error),
    #[error("Microsoft 登录失败：{0}")]
    OAuth(String),
    #[error("{0}")]
    InvalidResponse(&'static str),
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Runtime(String),
    #[error("{0}")]
    Protocol(String),
    #[error("{stage} 返回 HTTP {status}，XErr={error_code:?}")]
    XboxService {
        stage: &'static str,
        status: u16,
        error_code: Option<u64>,
    },
    #[error("登录已取消")]
    Cancelled,
    #[error("Microsoft 设备代码已过期")]
    TimedOut,
}

impl AuthError {
    fn user_message(&self) -> String {
        match self {
            Self::Http(error) if error.is_timeout() => {
                "连接 Microsoft/Xbox 服务超时，请检查网络后重试".to_string()
            }
            Self::Http(_) => "无法连接 Microsoft/Xbox 服务，请检查网络后重试".to_string(),
            Self::XboxService {
                error_code: Some(2_148_916_233 | 2_148_916_234 | 2_148_916_235),
                ..
            } => "此 Microsoft 账户的 Xbox 档案不可用，请先在 xbox.com 完成档案设置".to_string(),
            Self::XboxService {
                error_code: Some(2_148_916_236 | 2_148_916_237 | 2_148_916_238),
                ..
            } => "此 Xbox 账户受年龄或家庭设置限制，无法完成登录".to_string(),
            Self::XboxService { status, .. } => {
                format!("Xbox 服务拒绝了登录请求（HTTP {status}）")
            }
            Self::OAuth(message)
            | Self::Storage(message)
            | Self::Runtime(message)
            | Self::Protocol(message) => message.clone(),
            Self::InvalidResponse(message) => (*message).to_string(),
            Self::Cancelled => "登录已取消".to_string(),
            Self::TimedOut => "Microsoft 登录代码已过期，请重新登录".to_string(),
        }
    }
}

pub(crate) fn event_stream() -> WatchStream<AuthSnapshot> {
    WatchStream::new(AUTH_STATE.1.clone())
}

pub(crate) fn initialize() {
    if !begin_flow(AuthPhase::Restoring, restore_session) {
        tracing::debug!("Xbox authentication restore is already running");
    }
}

pub(crate) fn start_login() -> Result<(), String> {
    if begin_flow(AuthPhase::RequestingCode, interactive_login) {
        Ok(())
    } else {
        Err("已有 Microsoft 登录流程正在进行".to_string())
    }
}

pub(crate) fn cancel_login() {
    FLOW_GENERATION.fetch_add(1, Ordering::AcqRel);
    publish(AuthSnapshot::stable_from_current());
}

pub(crate) fn switch_account(account_id: String) -> Result<(), String> {
    if !current_snapshot()
        .accounts
        .iter()
        .any(|profile| profile.xuid == account_id)
    {
        return Err("要切换的 Xbox 账号不存在".to_string());
    }
    if begin_flow(AuthPhase::SwitchingAccount, move |generation| {
        switch_account_flow(account_id, generation)
    }) {
        Ok(())
    } else {
        Err("已有 Microsoft 登录流程正在进行".to_string())
    }
}

pub(crate) fn remove_account(account_id: String) -> Result<(), String> {
    if !current_snapshot()
        .accounts
        .iter()
        .any(|profile| profile.xuid == account_id)
    {
        return Err("要删除的 Xbox 账号不存在".to_string());
    }
    if begin_flow(AuthPhase::SigningOut, move |generation| {
        remove_account_flow(account_id, generation)
    }) {
        Ok(())
    } else {
        Err("已有 Microsoft 登录流程正在进行".to_string())
    }
}

#[cfg(target_os = "linux")]
pub(crate) async fn prepare_launch(
    prefix_path: &Path,
) -> Result<Option<PreparedLaunchAuth>, String> {
    let generation = FLOW_GENERATION.load(Ordering::Acquire);
    let active_account = crate::tasks::runtime::run_io_blocking(|| {
        let _guard = ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
        secret_store::load_active_account()
    })
    .await
    .map_err(|error| AuthError::Runtime(error).user_message())?
    .map_err(|error| AuthError::Storage(error).user_message())?;
    let Some((_catalog, expected_profile, refresh_token)) = active_account else {
        return Ok(None);
    };

    let cached_device_json = {
        let cache = PREAUTH_CACHE.lock().unwrap();
        cache
            .as_ref()
            .and_then(|(cached_xuid, cached_json, timestamp)| {
                if cached_xuid == &expected_profile.xuid && timestamp.elapsed().as_secs() < 7200 {
                    Some(cached_json.clone())
                } else {
                    None
                }
            })
    };

    if let Some(device_json) = cached_device_json {
        let profile_for_storage = expected_profile.clone();
        let prefix_path = prefix_path.to_path_buf();

        let prepared = crate::tasks::runtime::run_io_blocking(move || {
            let _guard = ACCOUNT_LOCK
                .lock()
                .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
            if !current_generation(generation) {
                return Err("Microsoft 登录状态已在启动期间发生变化".to_string());
            }
            let prepared = wine_bridge::prepare(&prefix_path, &refresh_token, &device_json)?;
            Ok(prepared)
        })
        .await
        .map_err(|error| AuthError::Runtime(error).user_message())?
        .map_err(|error| AuthError::Storage(error).user_message())?;

        return Ok(Some(prepared));
    }

    let client = msa::client().map_err(|error| error.user_message())?;
    let token = msa::refresh(&client, &refresh_token)
        .await
        .map_err(|error| error.user_message())?;
    let preauth = xbox::authenticate(&client, &token.access_token)
        .await
        .map_err(|error| error.user_message())?;
    let profile = preauth.profile.clone();
    if profile.xuid != expected_profile.xuid {
        return Err("Microsoft 刷新凭证返回了不匹配的 Xbox 账号".to_string());
    }
    let device_json = preauth
        .winegdk_json()
        .map_err(|error| error.user_message())?;
    let prefix_path = prefix_path.to_path_buf();
    let profile_for_storage = profile.clone();
    let prepared = crate::tasks::runtime::run_io_blocking(move || {
        let _guard = ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
        if !current_generation(generation) {
            return Err("Microsoft 登录状态已在启动期间发生变化".to_string());
        }
        let catalog = secret_store::store_account(&profile_for_storage, &token.refresh_token)?;
        let prepared = wine_bridge::prepare(&prefix_path, &token.refresh_token, &device_json)?;
        Ok((catalog, prepared))
    })
    .await
    .map_err(|error| AuthError::Runtime(error).user_message())?
    .map_err(|error| AuthError::Storage(error).user_message())?;
    publish(AuthSnapshot::from_catalog(
        AuthPhase::SignedIn,
        &prepared.0,
        Some(profile),
    ));
    Ok(Some(prepared.1))
}

#[cfg(target_os = "windows")]
pub(crate) async fn prepare_launch_windows() -> Result<Option<PreparedLaunchAuth>, String> {
    let generation = FLOW_GENERATION.load(Ordering::Acquire);
    let active_account = crate::tasks::runtime::run_io_blocking(|| {
        let _guard = ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
        secret_store::load_active_account()
    })
    .await
    .map_err(|error| AuthError::Runtime(error).user_message())?
    .map_err(|error| AuthError::Storage(error).user_message())?;
    let Some((_catalog, expected_profile, refresh_token)) = active_account else {
        return Ok(None);
    };

    let cached_device_json = {
        let cache = PREAUTH_CACHE.lock().unwrap();
        cache
            .as_ref()
            .and_then(|(cached_xuid, cached_json, timestamp)| {
                if cached_xuid == &expected_profile.xuid && timestamp.elapsed().as_secs() < 7200 {
                    Some(cached_json.clone())
                } else {
                    None
                }
            })
    };

    if let Some(device_json) = cached_device_json {
        let profile_for_storage = expected_profile.clone();

        let prepared = crate::tasks::runtime::run_io_blocking(move || {
            let _guard = ACCOUNT_LOCK
                .lock()
                .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
            if !current_generation(generation) {
                return Err("Microsoft 登录状态已在启动期间发生变化".to_string());
            }
            let prepared = gdk_bridge::prepare(
                &profile_for_storage.xuid,
                &profile_for_storage.gamertag,
                &device_json,
            )?;
            Ok(prepared)
        })
        .await
        .map_err(|error| AuthError::Runtime(error).user_message())?
        .map_err(|error| AuthError::Storage(error).user_message())?;

        return Ok(Some(prepared));
    }

    let client = msa::client().map_err(|error| error.user_message())?;
    let token = msa::refresh(&client, &refresh_token)
        .await
        .map_err(|error| error.user_message())?;
    let preauth = xbox::authenticate(&client, &token.access_token)
        .await
        .map_err(|error| error.user_message())?;
    let profile = preauth.profile.clone();
    if profile.xuid != expected_profile.xuid {
        return Err("Microsoft 刷新凭证返回了不匹配的 Xbox 账号".to_string());
    }
    let device_json = preauth
        .winegdk_json()
        .map_err(|error| error.user_message())?;
    let profile_for_storage = profile.clone();
    let prepared = crate::tasks::runtime::run_io_blocking(move || {
        let _guard = ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
        if !current_generation(generation) {
            return Err("Microsoft 登录状态已在启动期间发生变化".to_string());
        }
        let catalog = secret_store::store_account(&profile_for_storage, &token.refresh_token)?;
        let prepared = gdk_bridge::prepare(
            &profile_for_storage.xuid,
            &profile_for_storage.gamertag,
            &device_json,
        )?;
        Ok((catalog, prepared))
    })
    .await
    .map_err(|error| AuthError::Runtime(error).user_message())?
    .map_err(|error| AuthError::Storage(error).user_message())?;
    publish(AuthSnapshot::from_catalog(
        AuthPhase::SignedIn,
        &prepared.0,
        Some(profile),
    ));
    Ok(Some(prepared.1))
}

fn begin_flow<F, Fut>(initial_phase: AuthPhase, operation: F) -> bool
where
    F: FnOnce(u64) -> Fut + Send + 'static,
    Fut: Future<Output = Result<AuthSnapshot, AuthError>> + Send + 'static,
{
    if FLOW_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let generation = FLOW_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    publish(AuthSnapshot::phase_from_current(initial_phase));
    let spawn_result = crate::tasks::runtime::spawn_io(async move {
        let result = operation(generation).await;
        if current_generation(generation) {
            match result {
                Ok(snapshot) => publish(snapshot),
                Err(AuthError::Cancelled) => publish(AuthSnapshot::stable_from_current()),
                Err(error) => {
                    tracing::warn!(%error, "Xbox authentication flow failed");
                    publish(AuthSnapshot::error_from_current(&error));
                }
            }
        }
        FLOW_RUNNING.store(false, Ordering::Release);
    });
    if let Err(error) = spawn_result {
        FLOW_RUNNING.store(false, Ordering::Release);
        publish(AuthSnapshot::error_from_current(&AuthError::Runtime(error)));
    }
    true
}

async fn interactive_login(generation: u64) -> Result<AuthSnapshot, AuthError> {
    let client = msa::client()?;
    let code = msa::request_device_code(&client).await?;
    if !publish_for(generation, AuthSnapshot::waiting_from_current(&code)) {
        return Err(AuthError::Cancelled);
    }
    let token = msa::poll_device_code(&client, &code, || !current_generation(generation)).await?;
    if !publish_for(
        generation,
        AuthSnapshot::phase_from_current(AuthPhase::AuthenticatingXbox),
    ) {
        return Err(AuthError::Cancelled);
    }
    finish_authentication(client, token, generation, None, false, true).await
}

async fn restore_session(generation: u64) -> Result<AuthSnapshot, AuthError> {
    let candidate = crate::tasks::runtime::run_io_blocking(|| {
        let _guard = ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
        #[cfg(target_os = "linux")]
        {
            wine_bridge::clear_stale_temporary_credentials()?;
            wine_bridge::clear_all_prefix_credentials()?;
        }
        if let Some((catalog, profile, refresh_token)) = secret_store::load_active_account()? {
            return Ok(StoredLoginCandidate {
                catalog,
                profile: Some(profile),
                refresh_token: Some(refresh_token),
                legacy: false,
            });
        }
        let catalog = secret_store::load_account_catalog()?;
        let refresh_token = secret_store::load_legacy_refresh_token()?;
        Ok(StoredLoginCandidate {
            catalog,
            profile: None,
            legacy: refresh_token.is_some(),
            refresh_token,
        })
    })
    .await
    .map_err(AuthError::Runtime)?
    .map_err(AuthError::Storage)?;
    if !publish_for(
        generation,
        AuthSnapshot::from_catalog(
            AuthPhase::Restoring,
            &candidate.catalog,
            candidate.profile.clone(),
        ),
    ) {
        return Err(AuthError::Cancelled);
    }
    let Some(refresh_token) = candidate.refresh_token else {
        return Ok(AuthSnapshot::from_catalog(
            AuthPhase::SignedOut,
            &candidate.catalog,
            None,
        ));
    };
    let client = msa::client()?;
    let token = msa::refresh(&client, &refresh_token).await?;
    if !publish_for(
        generation,
        AuthSnapshot::phase_from_current(AuthPhase::AuthenticatingXbox),
    ) {
        return Err(AuthError::Cancelled);
    }
    let expected_account_id = candidate.profile.map(|profile| profile.xuid);
    finish_authentication(
        client,
        token,
        generation,
        expected_account_id,
        candidate.legacy,
        false,
    )
    .await
}

async fn finish_authentication(
    client: reqwest::Client,
    token: msa::MsaToken,
    generation: u64,
    expected_account_id: Option<String>,
    delete_legacy_token: bool,
    clear_prefix_credentials: bool,
) -> Result<AuthSnapshot, AuthError> {
    let preauth = xbox::authenticate(&client, &token.access_token).await?;
    if !current_generation(generation) {
        return Err(AuthError::Cancelled);
    }
    if expected_account_id
        .as_deref()
        .is_some_and(|account_id| account_id != preauth.profile.xuid)
    {
        return Err(AuthError::Protocol(
            "Microsoft 刷新凭证返回了不匹配的 Xbox 账号".to_string(),
        ));
    }
    if let Ok(device_json) = preauth.winegdk_json() {
        if let Ok(mut cache) = PREAUTH_CACHE.lock() {
            *cache = Some((
                preauth.profile.xuid.clone(),
                device_json,
                std::time::Instant::now(),
            ));
        }
    }

    let profile = preauth.profile;

    let profile_for_storage = profile.clone();
    let catalog = crate::tasks::runtime::run_io_blocking(move || {
        let _guard = ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
        if !current_generation(generation) {
            return Ok(None);
        }
        if clear_prefix_credentials {
            #[cfg(target_os = "linux")]
            wine_bridge::clear_all_prefix_credentials()?;
        }
        let catalog = secret_store::store_account(&profile_for_storage, &token.refresh_token)?;
        if delete_legacy_token {
            secret_store::delete_legacy_refresh_token()?;
        }
        Ok(Some(catalog))
    })
    .await
    .map_err(AuthError::Runtime)?
    .map_err(AuthError::Storage)?
    .ok_or(AuthError::Cancelled)?;
    Ok(AuthSnapshot::from_catalog(
        AuthPhase::SignedIn,
        &catalog,
        Some(profile),
    ))
}

async fn switch_account_flow(
    account_id: String,
    generation: u64,
) -> Result<AuthSnapshot, AuthError> {
    let (mut catalog, profile, refresh_token) = crate::tasks::runtime::run_io_blocking(move || {
        let _guard = ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
        let catalog = secret_store::load_account_catalog()?;
        let profile = catalog
            .profile(&account_id)
            .cloned()
            .ok_or_else(|| "要切换的 Xbox 账号不存在".to_string())?;
        let refresh_token = secret_store::load_account_refresh_token(&account_id)?
            .ok_or_else(|| format!("账号 {} 的加密登录凭证不存在", profile.gamertag))?;
        Ok((catalog, profile, refresh_token))
    })
    .await
    .map_err(AuthError::Runtime)?
    .map_err(AuthError::Storage)?;
    catalog.active_account_id = Some(profile.xuid.clone());
    if !publish_for(
        generation,
        AuthSnapshot::from_catalog(AuthPhase::SwitchingAccount, &catalog, Some(profile.clone())),
    ) {
        return Err(AuthError::Cancelled);
    }
    let client = msa::client()?;
    let token = msa::refresh(&client, &refresh_token).await?;
    finish_authentication(client, token, generation, Some(profile.xuid), false, true).await
}

async fn remove_account_flow(
    account_id: String,
    generation: u64,
) -> Result<AuthSnapshot, AuthError> {
    let previous_profile = current_snapshot().profile;
    let (catalog, was_active, next_account) = crate::tasks::runtime::run_io_blocking(move || {
        let _guard = ACCOUNT_LOCK
            .lock()
            .map_err(|_| "Microsoft 登录凭证锁已损坏".to_string())?;
        let (catalog, was_active) = secret_store::remove_account(&account_id)?;
        if was_active {
            #[cfg(target_os = "linux")]
            wine_bridge::clear_all_prefix_credentials()?;
        }
        let next_account = if was_active {
            let Some(next_account_id) = catalog.active_account_id.as_deref() else {
                return Ok((catalog, true, None));
            };
            let profile = catalog
                .profile(next_account_id)
                .cloned()
                .ok_or_else(|| "Microsoft 账号索引缺少候选账号资料".to_string())?;
            let refresh_token = secret_store::load_account_refresh_token(next_account_id)?
                .ok_or_else(|| format!("账号 {} 的加密登录凭证不存在", profile.gamertag))?;
            Some((profile, refresh_token))
        } else {
            None
        };
        Ok((catalog, was_active, next_account))
    })
    .await
    .map_err(AuthError::Runtime)?
    .map_err(AuthError::Storage)?;
    if !current_generation(generation) {
        return Err(AuthError::Cancelled);
    }
    if !was_active {
        return Ok(AuthSnapshot::from_catalog(
            if previous_profile.is_some() {
                AuthPhase::SignedIn
            } else {
                AuthPhase::SignedOut
            },
            &catalog,
            previous_profile,
        ));
    }
    let Some((profile, refresh_token)) = next_account else {
        return Ok(AuthSnapshot::from_catalog(
            AuthPhase::SignedOut,
            &catalog,
            None,
        ));
    };
    if !publish_for(
        generation,
        AuthSnapshot::from_catalog(AuthPhase::SwitchingAccount, &catalog, Some(profile.clone())),
    ) {
        return Err(AuthError::Cancelled);
    }
    let client = msa::client()?;
    let token = msa::refresh(&client, &refresh_token).await?;
    finish_authentication(client, token, generation, Some(profile.xuid), false, false).await
}

fn current_snapshot() -> AuthSnapshot {
    AUTH_STATE.1.borrow().clone()
}

fn publish_for(generation: u64, snapshot: AuthSnapshot) -> bool {
    if !current_generation(generation) {
        return false;
    }
    publish(snapshot);
    true
}

fn current_generation(generation: u64) -> bool {
    FLOW_GENERATION.load(Ordering::Acquire) == generation
}

fn publish(snapshot: AuthSnapshot) {
    AUTH_STATE.0.send_replace(snapshot);
}
