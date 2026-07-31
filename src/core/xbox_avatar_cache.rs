use crate::core::bedrock_auth::XboxProfile;
use once_cell::sync::Lazy;
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

const AVATAR_CACHE_DIR: &str = "xbox/avatars";
const MAX_DOWNLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_SOURCE_DIMENSION: u32 = 4096;
const MAX_SOURCE_PIXELS: u64 = 16 * 1024 * 1024;
const CACHED_AVATAR_EDGE: u32 = 256;

static CACHE_INDEX: Lazy<Mutex<HashMap<String, PathBuf>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static IN_FLIGHT: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static REFRESHED: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static CACHE_REVISION: AtomicU64 = AtomicU64::new(0);
static CACHE_EVENTS: Lazy<(watch::Sender<u64>, watch::Receiver<u64>)> =
    Lazy::new(|| watch::channel(0));

pub(crate) fn event_stream() -> WatchStream<u64> {
    WatchStream::new(CACHE_EVENTS.1.clone())
}

/// Returns only a verified local cache path. Network URLs are deliberately not
/// returned here: UI code should render the Lucide fallback until a local file
/// is ready, then switch to that file after the cache event is emitted.
pub(crate) fn cached_avatar_path(profile: &XboxProfile) -> Option<PathBuf> {
    cached_path_for_xuid(&profile.xuid)
}

/// Schedules one background refresh per `(XUID, avatar URL)` during the current
/// launcher process. Existing files remain available while the network request
/// runs, so startup and account switching always render cache-first.
pub(crate) fn refresh_profiles(profiles: Vec<XboxProfile>) {
    for profile in profiles {
        let Some(url) = profile
            .avatar_url
            .as_deref()
            .map(str::trim)
            .filter(|url| url.starts_with("https://"))
            .map(ToString::to_string)
        else {
            continue;
        };
        if !valid_xuid(&profile.xuid) {
            tracing::warn!(xuid = %profile.xuid, "拒绝为无效 XUID 缓存 Xbox 头像");
            continue;
        }

        let refresh_key = format!("{}\n{}", profile.xuid, url);
        if REFRESHED
            .lock()
            .map(|set| set.contains(&refresh_key))
            .unwrap_or(false)
        {
            continue;
        }
        let should_start = IN_FLIGHT
            .lock()
            .map(|mut set| set.insert(refresh_key.clone()))
            .unwrap_or(false);
        if !should_start {
            continue;
        }

        let xuid = profile.xuid;
        let gamertag = profile.gamertag;
        let task_key = refresh_key.clone();
        let spawn_result = crate::tasks::runtime::spawn_io(async move {
            let result = download_and_cache(&xuid, &url).await;
            if let Ok(mut in_flight) = IN_FLIGHT.lock() {
                in_flight.remove(&task_key);
            }
            match result {
                Ok(path) => {
                    if let Ok(mut refreshed) = REFRESHED.lock() {
                        refreshed.insert(task_key);
                    }
                    tracing::debug!(
                        xbox_gamertag = %gamertag,
                        cache_path = %path.display(),
                        "Xbox 头像缓存已更新"
                    );
                }
                Err(error) => {
                    tracing::debug!(
                        xbox_gamertag = %gamertag,
                        %error,
                        "Xbox 头像网络更新失败，继续使用已有缓存或默认头像"
                    );
                }
            }
        });
        if let Err(error) = spawn_result {
            if let Ok(mut in_flight) = IN_FLIGHT.lock() {
                in_flight.remove(&refresh_key);
            }
            tracing::warn!(%error, "无法启动 Xbox 头像缓存任务");
        }
    }
}

async fn download_and_cache(xuid: &str, url: &str) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .user_agent("BMCBL Xbox Avatar Cache")
        .timeout(Duration::from_secs(20))
        .https_only(true)
        .build()
        .map_err(|error| format!("创建头像请求客户端失败：{error}"))?;
    let response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "image/*")
        .send()
        .await
        .map_err(|error| format!("下载 Xbox 头像失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("Xbox 头像服务返回 HTTP {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
    {
        return Err("Xbox 头像响应超过大小限制".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("读取 Xbox 头像响应失败：{error}"))?;
    if bytes.is_empty() || bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err("Xbox 头像响应为空或超过大小限制".to_string());
    }

    let xuid = xuid.to_string();
    let bytes = bytes.to_vec();
    crate::tasks::runtime::run_io_blocking(move || write_cache_entry(&xuid, &bytes))
        .await
        .map_err(|error| format!("头像缓存后台任务失败：{error}"))?
}

fn write_cache_entry(xuid: &str, source: &[u8]) -> Result<PathBuf, String> {
    if !valid_xuid(xuid) {
        return Err("Xbox XUID 无效".to_string());
    }

    let dimensions = image::ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .map_err(|error| format!("识别 Xbox 头像格式失败：{error}"))?
        .into_dimensions()
        .map_err(|error| format!("读取 Xbox 头像尺寸失败：{error}"))?;
    let source_pixels = u64::from(dimensions.0) * u64::from(dimensions.1);
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > MAX_SOURCE_DIMENSION
        || dimensions.1 > MAX_SOURCE_DIMENSION
        || source_pixels > MAX_SOURCE_PIXELS
    {
        return Err(format!(
            "Xbox 头像尺寸不受支持：{}x{}",
            dimensions.0, dimensions.1
        ));
    }

    let decoded = image::load_from_memory(source)
        .map_err(|error| format!("解码 Xbox 头像失败：{error}"))?;
    let normalized = decoded.thumbnail(CACHED_AVATAR_EDGE, CACHED_AVATAR_EDGE);
    let mut png = Vec::new();
    normalized
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| format!("编码 Xbox 头像缓存失败：{error}"))?;

    let digest = hex::encode(Sha256::digest(&png));
    let file_name = format!("{xuid}-{}.png", &digest[..16]);
    let cache_dir = avatar_cache_dir();
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("创建 Xbox 头像缓存目录失败：{error}"))?;
    let final_path = cache_dir.join(&file_name);
    if !final_path.is_file() {
        let temporary_path = cache_dir.join(format!(
            ".{file_name}.{}.tmp",
            uuid::Uuid::new_v4().as_simple()
        ));
        std::fs::write(&temporary_path, &png)
            .map_err(|error| format!("写入 Xbox 头像临时缓存失败：{error}"))?;
        if let Err(error) = std::fs::rename(&temporary_path, &final_path) {
            let _ = std::fs::remove_file(&temporary_path);
            if !final_path.is_file() {
                return Err(format!("提交 Xbox 头像缓存失败：{error}"));
            }
        }
    }

    write_pointer(xuid, &file_name, &cache_dir)?;
    let changed = CACHE_INDEX
        .lock()
        .map(|mut index| index.insert(xuid.to_string(), final_path.clone()) != Some(final_path.clone()))
        .unwrap_or(true);
    if changed {
        emit_cache_event();
    }
    Ok(final_path)
}

fn cached_path_for_xuid(xuid: &str) -> Option<PathBuf> {
    if !valid_xuid(xuid) {
        return None;
    }
    if let Ok(index) = CACHE_INDEX.lock()
        && let Some(path) = index.get(xuid)
        && path.is_file()
    {
        return Some(path.clone());
    }

    let cache_dir = avatar_cache_dir();
    let pointer = cache_dir.join(format!("{xuid}.current"));
    let file_name = std::fs::read_to_string(pointer).ok()?;
    let file_name = file_name.trim();
    if !valid_cache_file_name(xuid, file_name) {
        return None;
    }
    let path = cache_dir.join(file_name);
    if !path.is_file() {
        return None;
    }
    if let Ok(mut index) = CACHE_INDEX.lock() {
        index.insert(xuid.to_string(), path.clone());
    }
    Some(path)
}

fn write_pointer(xuid: &str, file_name: &str, cache_dir: &Path) -> Result<(), String> {
    if !valid_cache_file_name(xuid, file_name) {
        return Err("Xbox 头像缓存文件名无效".to_string());
    }
    let pointer_path = cache_dir.join(format!("{xuid}.current"));
    let temporary_path = cache_dir.join(format!(
        ".{xuid}.{}.current.tmp",
        uuid::Uuid::new_v4().as_simple()
    ));
    std::fs::write(&temporary_path, format!("{file_name}\n"))
        .map_err(|error| format!("写入 Xbox 头像索引失败：{error}"))?;
    #[cfg(windows)]
    if pointer_path.exists() {
        std::fs::remove_file(&pointer_path)
            .map_err(|error| format!("替换 Xbox 头像索引失败：{error}"))?;
    }
    std::fs::rename(&temporary_path, &pointer_path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        format!("提交 Xbox 头像索引失败：{error}")
    })
}

fn avatar_cache_dir() -> PathBuf {
    crate::utils::file_ops::cache_subdir(AVATAR_CACHE_DIR)
}

fn valid_xuid(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_cache_file_name(xuid: &str, file_name: &str) -> bool {
    !file_name.is_empty()
        && file_name.len() <= 96
        && file_name.starts_with(&format!("{xuid}-"))
        && file_name.ends_with(".png")
        && Path::new(file_name).components().count() == 1
}

fn emit_cache_event() {
    let revision = CACHE_REVISION.fetch_add(1, Ordering::AcqRel) + 1;
    CACHE_EVENTS.0.send_replace(revision);
}

#[cfg(test)]
mod tests {
    use super::{valid_cache_file_name, valid_xuid};

    #[test]
    fn xuid_accepts_only_bounded_decimal_identifiers() {
        assert!(valid_xuid("1234567890"));
        assert!(!valid_xuid(""));
        assert!(!valid_xuid("../123"));
        assert!(!valid_xuid(&"1".repeat(33)));
    }

    #[test]
    fn cache_file_name_cannot_escape_account_directory() {
        assert!(valid_cache_file_name("123", "123-aabbccdd.png"));
        assert!(!valid_cache_file_name("123", "../123-aabbccdd.png"));
        assert!(!valid_cache_file_name("123", "999-aabbccdd.png"));
    }
}
