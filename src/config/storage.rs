use super::config::{
    CURRENT_CONFIG_VERSION, Config, FONT_SOURCE_DEFAULT, FONT_SOURCE_LOCAL, FONT_SOURCE_SYSTEM,
    clamp_background_blur, clamp_music_volume, default_error_report_sentry_dsn,
    default_glass_effect_enabled, default_gpu_adapter_name, default_online_player_name,
    get_default_config, normalize_font_source, normalize_gpu_adapter_name, normalize_language_code,
    normalize_renderer_backend, normalize_theme_mode,
};
use crate::{http::proxy, utils::file_ops};
use once_cell::sync::Lazy;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, Once, RwLock};
use std::time::{Duration, Instant};
use std::{fs, io};
use tracing::{debug, error};

static CONFIG_CACHE: Lazy<RwLock<Option<Config>>> = Lazy::new(|| RwLock::new(None));
static CONFIG_SYNC_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// 配置写盘合并：任意设置项改动先更新内存缓存并标记 dirty，
/// 由后台线程在 ~500ms 静默期后统一序列化 + fsync + rename，
/// 避免连续调节设置（如滑块）时每次改动都全量写盘。
const CONFIG_FLUSH_DEBOUNCE: Duration = Duration::from_millis(500);

struct ConfigFlushState {
    dirty_at: Option<Instant>,
}

static CONFIG_FLUSH: Lazy<(Mutex<ConfigFlushState>, Condvar)> = Lazy::new(|| {
    (
        Mutex::new(ConfigFlushState { dirty_at: None }),
        Condvar::new(),
    )
});

fn schedule_config_flush() {
    ensure_config_flush_thread();
    let (lock, condvar) = &*CONFIG_FLUSH;
    let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    state.dirty_at = Some(Instant::now());
    condvar.notify_all();
}

fn ensure_config_flush_thread() {
    static SPAWN_FLUSH_THREAD: Once = Once::new();
    SPAWN_FLUSH_THREAD.call_once(|| {
        if let Err(spawn_error) = std::thread::Builder::new()
            .name("bmcbl-config-flush".to_string())
            .spawn(config_flush_worker)
        {
            error!("Failed to spawn config flush thread: {spawn_error}");
        }
    });
}

fn config_flush_worker() {
    let (lock, condvar) = &*CONFIG_FLUSH;
    loop {
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while state.dirty_at.is_none() {
            state = condvar
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        // 静默期等待：期间新的改动会重置计时；flush_config_now 会清空 dirty_at。
        loop {
            let Some(dirty_at) = state.dirty_at else {
                break;
            };
            let elapsed = dirty_at.elapsed();
            if elapsed >= CONFIG_FLUSH_DEBOUNCE {
                break;
            }
            let (guard, _timeout) = condvar
                .wait_timeout(state, CONFIG_FLUSH_DEBOUNCE - elapsed)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = guard;
        }
        if state.dirty_at.take().is_none() {
            continue;
        }
        drop(state);
        flush_cached_config_to_disk();
    }
}

fn flush_cached_config_to_disk() {
    let _sync_guard = CONFIG_SYNC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(config) = read_cached_config() else {
        return;
    };
    if let Err(persist_error) = persist_config_to_disk(&config) {
        error!("Failed to persist config to disk: {:?}", persist_error);
    }
}

/// 立即把内存中未落盘的配置写入磁盘（供应用退出路径调用）。
pub fn flush_config_now() {
    let (lock, _) = &*CONFIG_FLUSH;
    {
        let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.dirty_at.take().is_none() {
            return;
        }
    }
    flush_cached_config_to_disk();
}

#[cfg(test)]
pub(super) fn clear_config_cache_for_test() -> std::sync::MutexGuard<'static, ()> {
    let guard = CONFIG_SYNC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    {
        let mut cache = CONFIG_CACHE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cache = None;
    }
    guard
}

pub fn get_config_file_path() -> PathBuf {
    file_ops::config_dir().join("settings.toml")
}

pub fn ensure_config_dir() -> io::Result<()> {
    let config_dir = file_ops::config_dir();
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir)?;
    }
    Ok(())
}

pub fn ensure_config_file() -> io::Result<()> {
    let config_file = get_config_file_path();
    if !config_file.exists() {
        let default_config = get_default_config();
        let toml_content = toml::to_string(&default_config).map_err(|error| {
            io::Error::other(format!("Failed to serialize default config: {error}"))
        })?;
        let mut file = fs::File::create(config_file)?;
        file.write_all(toml_content.as_bytes())?;
    }
    Ok(())
}

pub fn initialize_config_cache() -> io::Result<Config> {
    let _sync_guard = CONFIG_SYNC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let config = load_config_from_disk()?;
    store_cached_config(&config);
    Ok(config)
}

pub fn read_config() -> io::Result<Config> {
    read_cached_config().ok_or_else(config_cache_not_initialized_error)
}

pub fn reload_config() -> io::Result<Config> {
    let _sync_guard = CONFIG_SYNC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let config = load_config_from_disk()?;
    store_cached_config(&config);
    Ok(config)
}

fn read_cached_config() -> Option<Config> {
    CONFIG_CACHE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 只读取代理配置：只在缓存读锁内克隆小结构，避免为读一个字段深拷贝整个 Config。
pub fn read_proxy_config() -> io::Result<super::config::ProxyConfig> {
    CONFIG_CACHE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|config| config.launcher.download.proxy.clone())
        .ok_or_else(config_cache_not_initialized_error)
}

fn store_cached_config(config: &Config) {
    let mut cache = CONFIG_CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *cache = Some(config.clone());
}

fn config_cache_not_initialized_error() -> io::Error {
    io::Error::other("configuration cache is not initialized; initialize it during startup")
}

fn load_config_from_disk() -> io::Result<Config> {
    ensure_config_dir()?;
    ensure_config_file()?;

    let config_file = get_config_file_path();
    let content = fs::read_to_string(&config_file)?;
    let has_legacy_keep_appx = content.contains("keep_appx_after_install");
    let has_config_version = content.contains("config_version");
    let has_renderer_backend = content.contains("renderer_backend");
    let has_gpu_adapter_name = content.contains("gpu_adapter_name");
    let has_auto_check_updates = content.contains("auto_check_updates");
    let has_check_on_start = content.contains("check_on_start");
    let has_error_report_sentry_enabled = content.contains("error_report_sentry_enabled");
    let has_error_report_sentry_dsn = content.contains("error_report_sentry_dsn");
    let legacy_gpu_power_preference = extract_legacy_gpu_power_preference(&content);
    let has_background_blur = content.contains("background_blur");
    let has_glass_effect_enabled = content.contains("glass_effect_enabled");
    let has_theme_mode = content.contains("theme_mode");
    let has_font_source = content.contains("font_source");
    let has_music_section = content.contains("[music]");
    let has_online_section = content.contains("[online]");
    let has_online_player_name = content.contains("player_name");

    let config: Config = match toml::from_str(&content) {
        Ok(parsed_config) => parsed_config,
        Err(err) => {
            error!("Failed to parse config on first attempt: {:?}", err);

            if let Ok(toml::Value::Table(existing_table)) = toml::from_str::<toml::Value>(&content)
            {
                if let Ok(toml::Value::Table(default_table)) =
                    toml::Value::try_from(&get_default_config())
                {
                    let merged_config = merge_tables(default_table, existing_table);
                    match toml::ser::to_string(&toml::Value::Table(merged_config)) {
                        Ok(updated_content) => {
                            fs::write(&config_file, updated_content)?;
                        }
                        Err(serialize_err) => {
                            error!(
                                "Failed to serialize merged config, falling back to parsed content: {:?}",
                                serialize_err
                            );
                        }
                    }
                }
            }

            let updated_content = fs::read_to_string(&config_file)?;
            toml::from_str(&updated_content).unwrap_or_else(|second_err| {
                error!("Failed to parse config on second attempt: {:?}", second_err);
                get_default_config()
            })
        }
    };

    let mut config = config;
    let mut migrated = false;

    // Ensure the version field exists on disk even for older configs that deserialize via defaults.
    if !has_config_version {
        config.config_version = CURRENT_CONFIG_VERSION;
        // Old configs predate schema versioning; force re-acceptance when terms change.
        config.agreement_accepted = false;
        migrated = true;
    } else if config.config_version < CURRENT_CONFIG_VERSION {
        // Future migrations can be keyed off config.config_version.
        config.config_version = CURRENT_CONFIG_VERSION;
        migrated = true;
    }

    let normalized_lang = normalize_language_code(&config.launcher.language);
    if normalized_lang != config.launcher.language {
        config.launcher.language = normalized_lang;
        migrated = true;
    }
    let normalized_renderer_backend = normalize_renderer_backend(&config.launcher.renderer_backend);
    if normalized_renderer_backend != config.launcher.renderer_backend {
        config.launcher.renderer_backend = normalized_renderer_backend;
        migrated = true;
    }
    if !has_renderer_backend {
        migrated = true;
    }
    let normalized_gpu_adapter_name = normalize_gpu_adapter_name(&config.launcher.gpu_adapter_name);
    if normalized_gpu_adapter_name != config.launcher.gpu_adapter_name {
        config.launcher.gpu_adapter_name = normalized_gpu_adapter_name;
        migrated = true;
    }
    if !has_gpu_adapter_name {
        if matches!(
            legacy_gpu_power_preference.as_deref(),
            Some("integrated") | Some("igpu") | Some("low") | Some("low_power")
        ) {
            config.launcher.gpu_adapter_name = default_gpu_adapter_name();
        }
        migrated = true;
    }
    let normalized_background_blur = clamp_background_blur(config.custom_style.background_blur);
    if !config.custom_style.background_blur.is_finite()
        || (config.custom_style.background_blur - normalized_background_blur).abs() > f32::EPSILON
    {
        config.custom_style.background_blur = normalized_background_blur;
        migrated = true;
    }
    if !has_background_blur {
        migrated = true;
    }
    if !has_glass_effect_enabled {
        config.custom_style.glass_effect_enabled = default_glass_effect_enabled();
        migrated = true;
    }
    let normalized_theme_mode = normalize_theme_mode(&config.custom_style.theme_mode);
    if normalized_theme_mode != config.custom_style.theme_mode {
        config.custom_style.theme_mode = normalized_theme_mode;
        migrated = true;
    }
    if !has_theme_mode {
        migrated = true;
    }
    let normalized_font_source = normalize_font_source(&config.custom_style.font_source);
    if normalized_font_source != config.custom_style.font_source {
        config.custom_style.font_source = normalized_font_source;
        migrated = true;
    }
    if config.custom_style.font_source == FONT_SOURCE_LOCAL
        && config.custom_style.local_font_path.trim().is_empty()
    {
        config.custom_style.font_source = FONT_SOURCE_DEFAULT.to_string();
        migrated = true;
    }
    if config.custom_style.font_source == FONT_SOURCE_SYSTEM
        && config.custom_style.system_font_family.trim().is_empty()
    {
        config.custom_style.font_source = FONT_SOURCE_DEFAULT.to_string();
        migrated = true;
    }
    if !has_font_source {
        migrated = true;
    }
    migrated |= normalize_music_settings(&mut config, has_music_section);
    if !has_online_section {
        migrated = true;
    }
    if !has_online_player_name
        || config.online.player_name.trim().is_empty()
        || config.online.player_name.trim() == "BMCBL_USER"
        || config.online.player_name.trim().starts_with("BMCBL_USER_")
    {
        config.online.player_name = default_online_player_name();
        migrated = true;
    }
    migrated |=
        normalize_update_check_settings(&mut config, has_auto_check_updates, has_check_on_start);
    if has_legacy_keep_appx {
        migrated = true;
    }
    if !has_error_report_sentry_enabled {
        let legacy_dsn_missing = !has_error_report_sentry_dsn
            || config.launcher.error_report_sentry_dsn.trim().is_empty();
        config.launcher.error_report_sentry_enabled = true;
        if legacy_dsn_missing {
            config.launcher.error_report_sentry_auto = false;
        }
        migrated = true;
    }
    if config.launcher.error_report_sentry_enabled
        && config.launcher.error_report_sentry_dsn.trim().is_empty()
    {
        config.launcher.error_report_sentry_dsn = default_error_report_sentry_dsn();
        migrated = true;
    }
    if !config.launcher.error_report_sentry_enabled && config.launcher.error_report_sentry_auto {
        config.launcher.error_report_sentry_auto = false;
        migrated = true;
    }
    if migrated {
        if let Err(error) = persist_config_to_disk(&config) {
            error!("Failed to persist migrated config: {:?}", error);
        } else {
            debug!("Migrated config");
        }
    }

    debug!("Read and updated config: {:?}", config);
    Ok(config)
}

fn persist_config_to_disk(config: &Config) -> io::Result<()> {
    ensure_config_dir()?;
    let config_file = get_config_file_path();
    let temp_file = config_file.with_extension("toml.tmp");
    let toml_content = toml::to_string(config)
        .map_err(|error| io::Error::other(format!("Failed to serialize config: {error}")))?;
    let mut file = fs::File::create(&temp_file)?;
    file.write_all(toml_content.as_bytes())?;
    file.sync_all()?;
    drop(file);

    if let Err(error) = fs::rename(&temp_file, &config_file) {
        if let Err(remove_error) = fs::remove_file(&temp_file)
            && remove_error.kind() != io::ErrorKind::NotFound
        {
            error!(
                ?remove_error,
                temp_path = %temp_file.display(),
                "failed to remove temporary config file after persist error"
            );
        }
        return Err(error);
    }

    Ok(())
}

pub(super) fn normalize_update_check_settings(
    config: &mut Config,
    has_auto_check_updates: bool,
    has_check_on_start: bool,
) -> bool {
    let mut migrated = false;

    if !has_auto_check_updates {
        if has_check_on_start {
            config.launcher.auto_check_updates = config.launcher.check_on_start;
        }
        migrated = true;
    }

    if config.launcher.check_on_start != config.launcher.auto_check_updates {
        config.launcher.check_on_start = config.launcher.auto_check_updates;
        if has_check_on_start {
            migrated = true;
        }
    }

    migrated
}

pub(super) fn normalize_music_settings(config: &mut Config, has_music_section: bool) -> bool {
    let mut migrated = false;
    let normalized_music_volume = clamp_music_volume(config.music.volume);
    if !config.music.volume.is_finite()
        || (config.music.volume - normalized_music_volume).abs() > f32::EPSILON
    {
        config.music.volume = normalized_music_volume;
        migrated = true;
    }
    if !has_music_section {
        migrated = true;
    }
    migrated
}

fn extract_legacy_gpu_power_preference(content: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(content).ok()?;
    value
        .get("launcher")?
        .get("gpu_power_preference")?
        .as_str()
        .map(|value| value.trim().to_ascii_lowercase().replace('-', "_"))
}

fn merge_tables(
    mut default: toml::map::Map<String, toml::Value>,
    existing: toml::map::Map<String, toml::Value>,
) -> toml::map::Map<String, toml::Value> {
    for (key, existing_value) in existing {
        match default.get_mut(&key) {
            Some(default_value) => {
                if let (toml::Value::Table(default_table), toml::Value::Table(existing_table)) =
                    (default_value.clone(), existing_value.clone())
                {
                    *default_value =
                        toml::Value::Table(merge_tables(default_table, existing_table));
                } else {
                    *default_value = existing_value;
                }
            }
            None => {
                default.insert(key, existing_value);
            }
        }
    }
    default
}

/// 写入完整配置：立即更新内存缓存（`read_config` 马上可见），
/// 磁盘写入由后台线程按静默期合并执行。
pub fn write_config(config: &Config) -> std::io::Result<()> {
    let _sync_guard = CONFIG_SYNC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let proxy_changed = {
        let mut cache = CONFIG_CACHE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(cached) = cache.as_mut() else {
            return Err(config_cache_not_initialized_error());
        };
        let changed = cached.launcher.download.proxy != config.launcher.download.proxy;
        *cached = config.clone();
        changed
    };
    schedule_config_flush();

    if proxy_changed {
        proxy::clear_client_cache();
    }
    Ok(())
}

/// 更新配置：直接在缓存上原地修改（避免多次全量 Config 克隆），
/// 磁盘写入由后台线程按静默期合并执行。
pub fn update_config<T, F>(mutator: F) -> io::Result<T>
where
    F: FnOnce(&mut Config) -> T,
{
    let _sync_guard = CONFIG_SYNC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let (result, proxy_changed) = {
        let mut cache = CONFIG_CACHE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(config) = cache.as_mut() else {
            return Err(config_cache_not_initialized_error());
        };
        let previous_proxy = config.launcher.download.proxy.clone();
        let result = mutator(config);
        let proxy_changed = config.launcher.download.proxy != previous_proxy;
        (result, proxy_changed)
    };
    schedule_config_flush();

    if proxy_changed {
        proxy::clear_client_cache();
    }

    Ok(result)
}
