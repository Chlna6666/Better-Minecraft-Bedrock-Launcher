// src/downloads/manager.rs
use crate::config::config::read_config;
use crate::downloads::multi::download_multi;
use crate::downloads::single::download_file;
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{create_task, finish_task, update_progress};
use num_cpus;
use reqwest::header::CONTENT_DISPOSITION;
use reqwest::header::HeaderMap;
use reqwest::Client;
use reqwest::Url;
use std::path::Path;
use std::path::PathBuf;
use tracing::debug;

pub struct DownloaderManager {
    client: Client,
}

fn is_placeholder_download_name(dest: &Path) -> bool {
    dest.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("download"))
        .unwrap_or(false)
}

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    let mut s = trimmed.replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
    while s.ends_with('.') {
        s.pop();
    }
    if s.is_empty() {
        "download.bin".to_string()
    } else {
        s
    }
}

fn percent_decode_lossy(input: &str) -> String {
    // Minimal percent-decoder for RFC5987-ish values (UTF-8 assumed).
    // Invalid sequences are kept as-is.
    let mut out: Vec<u8> = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = bytes[i + 1];
            let h2 = bytes[i + 2];
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(a), Some(b)) = (hex(h1), hex(h2)) {
                out.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn filename_from_content_disposition(headers: &HeaderMap) -> Option<String> {
    let cd = headers.get(CONTENT_DISPOSITION)?.to_str().ok()?.trim();
    // Prefer filename* (RFC5987), then filename.
    for part in cd.split(';').map(|s| s.trim()) {
        let lower = part.to_ascii_lowercase();
        if lower.starts_with("filename*=") {
            let mut v = part.splitn(2, '=').nth(1)?.trim().trim_matches('"').to_string();
            // Expected: UTF-8''<percent-encoded>
            if let Some(idx) = v.find("''") {
                v = v[(idx + 2)..].to_string();
            }
            let v = percent_decode_lossy(&v);
            let v = v.trim().trim_matches('"').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    for part in cd.split(';').map(|s| s.trim()) {
        let lower = part.to_ascii_lowercase();
        if lower.starts_with("filename=") {
            let v = part.splitn(2, '=').nth(1)?.trim().trim_matches('"').to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn filename_from_url(u: &Url) -> Option<String> {
    u.path_segments()
        .and_then(|mut s| s.next_back())
        .map(|seg| seg.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn resolve_final_dest(
    client: &Client,
    url: &str,
    dest: &PathBuf,
    headers: Option<&HeaderMap>,
) -> Option<PathBuf> {
    if !is_placeholder_download_name(dest.as_path()) {
        return None;
    }

    let mut req = client.head(url);
    if let Some(h) = headers {
        req = req.headers(h.clone());
    }

    let suggested = match req.send().await {
        Ok(resp) => {
            filename_from_content_disposition(resp.headers())
                .or_else(|| filename_from_url(resp.url()))
        }
        Err(_) => {
            let effective_url = Url::parse(url).ok();
            effective_url.as_ref().and_then(filename_from_url)
        }
    };

    let mut final_name = suggested.map(|s| sanitize_filename(&s))?;
    if is_placeholder_download_name(Path::new(&final_name)) {
        // Still a placeholder - keep original dest.
        return None;
    }

    // If server doesn't provide an extension, keep the original extension (if any).
    if Path::new(&final_name).extension().is_none() {
        if let Some(ext) = dest.extension().and_then(|s| s.to_str()) {
            final_name = format!("{}.{}", final_name, ext);
        }
    }

    let parent = dest.parent()?;
    let final_dest = parent.join(final_name);

    // Avoid renaming to the same path.
    if final_dest == *dest {
        return None;
    }

    Some(final_dest)
}

async fn rename_overwrite(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    if tokio::fs::metadata(dst).await.is_ok() {
        tokio::fs::remove_file(dst).await?;
    }
    tokio::fs::rename(src, dst).await
}

impl DownloaderManager {
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    /// 直接执行下载（不创建 task），使用已有 task_id（命令层传入）
    pub async fn download_with_options(
        &self,
        task_id: &str,
        url: String,
        dest: PathBuf,
        headers: Option<HeaderMap>, // [新增]
        md5_expected: Option<&str>,
    ) -> Result<CoreResult<PathBuf>, CoreError> {
        let config = read_config().map_err(|e| CoreError::Config(e.to_string()))?;

        let threads = if config.launcher.download.auto_thread_count {
            num_cpus::get()
        } else if config.launcher.download.multi_thread {
            config.launcher.download.max_threads as usize
        } else {
            1
        };

        update_progress(task_id, 0, None, Some("downloading"));

        let final_dest = resolve_final_dest(&self.client, &url, &dest, headers.as_ref()).await;

        let mut retry = 0usize;
        loop {
            debug!(
                "DownloaderManager start download loop retry={} threads={}",
                retry, threads
            );
            let res = if threads > 1 {
                download_multi(
                    self.client.clone(),
                    task_id,
                    &url,
                    &dest,
                    threads,
                    headers.clone(),
                    md5_expected,
                )
                    .await
            } else {
                download_file(
                    self.client.clone(),
                    task_id,
                    &url,
                    &dest,
                    headers.clone(),
                    md5_expected,
                )
                    .await
            };

            match res {
                Ok(CoreResult::Success(_)) => {
                    if let Some(final_path) = final_dest.as_ref() {
                        update_progress(task_id, 0, None, Some("renaming"));
                        match rename_overwrite(&dest, final_path).await {
                            Ok(()) => return Ok(CoreResult::Success(final_path.clone())),
                            Err(e) => {
                                // Rename failed; keep the downloaded temp file as-is.
                                debug!(
                                    "rename_overwrite failed src={} dst={} err={}",
                                    dest.to_string_lossy(),
                                    final_path.to_string_lossy(),
                                    e
                                );
                                return Ok(CoreResult::Success(dest));
                            }
                        }
                    }
                    return Ok(CoreResult::Success(dest));
                }
                Ok(CoreResult::Cancelled) => return Ok(CoreResult::Cancelled),
                Ok(CoreResult::Error(e)) => {
                    if retry < 3 {
                        retry += 1;
                        debug!("DownloaderManager retrying {} for url={}", retry, url);
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    return Ok(CoreResult::Error(e));
                }
                Err(e) => {
                    if retry < 3 {
                        retry += 1;
                        debug!("DownloaderManager retrying {} for url={}", retry, url);
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    /// manager 创建新的 task 并 spawn 后台执行，立即返回 task_id
    pub fn start_download(
        &self,
        url: String,
        dest: PathBuf,
        md5_expected: Option<String>,
    ) -> String {
        let task_id = create_task(None, "ready", None);
        let client = self.client.clone();

        // clones for task
        let url_clone = url.clone();
        let dest_clone = dest.clone();
        let md5_clone = md5_expected.clone();
        let task_id_clone = task_id.clone();

        tokio::spawn(async move {
            update_progress(&task_id_clone, 0, None, Some("starting"));

            let manager = DownloaderManager::with_client(client);

            let res = manager
                .download_with_options(
                    &task_id_clone,
                    url_clone,
                    dest_clone.clone(),
                    None, // 默认不传 header
                    md5_clone.as_deref(),
                )
                .await;

            match res {
                Ok(CoreResult::Success(final_path)) => {
                    finish_task(
                        &task_id_clone,
                        "completed",
                        Some(final_path.to_string_lossy().to_string()),
                    );
                }
                Ok(CoreResult::Cancelled) => {
                    finish_task(
                        &task_id_clone,
                        "cancelled",
                        Some("download cancelled".into()),
                    );
                    let _ = tokio::fs::remove_file(&dest_clone).await;
                }
                Ok(CoreResult::Error(err)) => {
                    finish_task(&task_id_clone, "error", Some(format!("{:?}", err)));
                    let _ = tokio::fs::remove_file(&dest_clone).await;
                }
                Err(e) => {
                    finish_task(&task_id_clone, "error", Some(format!("{:?}", e)));
                    let _ = tokio::fs::remove_file(&dest_clone).await;
                }
            }
        });

        task_id
    }
}
