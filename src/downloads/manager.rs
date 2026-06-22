// src/downloads/manager.rs
use crate::config::config::read_config;
use crate::downloads::integrity::{is_appx_download_path, verify_download_integrity};
use crate::downloads::multi::download_multi;
use crate::downloads::runtime::spawn_download_task;
use crate::downloads::single::download_file;
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{
    create_task_with_options, finish_task, register_task_abort_handle, reset_progress,
    task_control, update_progress,
};
use num_cpus;
use reqwest::Client;
use reqwest::Url;
use reqwest::header::CONTENT_DISPOSITION;
use reqwest::header::HeaderMap;
use std::path::Path;
use std::path::PathBuf;
use tracing::debug;

pub struct DownloaderManager {
    client: Client,
}

const MAX_DOWNLOAD_THREADS: usize = 8;
const MAX_MANUAL_DOWNLOAD_THREADS: usize = 16;

pub(crate) fn temp_download_path(final_dest: &Path) -> PathBuf {
    let mut temp_dest = final_dest.to_path_buf();
    match final_dest.extension().and_then(|value| value.to_str()) {
        Some(extension) if !extension.is_empty() => {
            temp_dest.set_extension(format!("{extension}.tmp"));
        }
        _ => {
            temp_dest.set_extension("tmp");
        }
    }
    temp_dest
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
            let mut v = part
                .splitn(2, '=')
                .nth(1)?
                .trim()
                .trim_matches('"')
                .to_string();
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
            let v = part
                .splitn(2, '=')
                .nth(1)?
                .trim()
                .trim_matches('"')
                .to_string();
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
        Ok(resp) => filename_from_content_disposition(resp.headers())
            .or_else(|| filename_from_url(resp.url())),
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
    match tokio::fs::rename(src, dst).await {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            debug!(
                "rename_overwrite fallback copy src={} dst={} err={}",
                src.to_string_lossy(),
                dst.to_string_lossy(),
                rename_error
            );
            tokio::fs::copy(src, dst).await?;
            tokio::fs::remove_file(src).await?;
            Ok(())
        }
    }
}

async fn remove_file_if_exists(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            debug!(
                "remove_file_if_exists failed path={} err={}",
                path.to_string_lossy(),
                error
            );
        }
    }
}

async fn verify_temp_download(path: &Path) -> Result<(), CoreError> {
    if is_appx_download_path(path) {
        debug!(
            "skip appx download-time archive verification path={}",
            path.to_string_lossy()
        );
        return Ok(());
    }

    verify_download_integrity(path, None).await?;

    Ok(())
}

fn is_appx_temp_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .is_some_and(|value| value.ends_with(".appx.tmp"))
}

fn is_trivial_candidate_failure(path: &Path, error: &CoreError) -> bool {
    if !is_appx_temp_path(path) {
        return false;
    }

    match error {
        CoreError::ChecksumMismatch(message) => {
            message.contains("invalid Zip archive: Could not find EOCD")
                || message.contains("invalid zip header")
                || message.contains("empty zip response")
        }
        _ => false,
    }
}

fn is_non_retryable_candidate_error(error: &CoreError) -> bool {
    matches!(error, CoreError::ChecksumMismatch(_))
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

        let configured_threads = if config.launcher.download.auto_thread_count {
            num_cpus::get()
                .saturating_sub(1)
                .clamp(2, MAX_DOWNLOAD_THREADS)
        } else if config.launcher.download.multi_thread {
            (config.launcher.download.max_threads as usize).clamp(1, MAX_MANUAL_DOWNLOAD_THREADS)
        } else {
            1
        };
        let task_control = task_control(task_id)
            .ok_or_else(|| CoreError::Other(format!("Task control missing for {task_id}")))?;

        update_progress(task_id, 0, None, Some("downloading"));

        let final_dest = resolve_final_dest(&self.client, &url, &dest, headers.as_ref())
            .await
            .unwrap_or(dest.clone());
        let temp_dest = temp_download_path(&final_dest);
        let threads = configured_threads.clamp(1, MAX_MANUAL_DOWNLOAD_THREADS);

        let mut retry = 0usize;
        loop {
            debug!(
                "DownloaderManager start download loop retry={} threads={}",
                retry, threads
            );
            reset_progress(task_id, None, Some("downloading"));
            remove_file_if_exists(&temp_dest).await;
            let res = if threads > 1 {
                download_multi(
                    self.client.clone(),
                    task_control.clone(),
                    task_id,
                    &url,
                    &temp_dest,
                    threads,
                    headers.clone(),
                    md5_expected,
                )
                .await
            } else {
                download_file(
                    self.client.clone(),
                    task_control.clone(),
                    task_id,
                    &url,
                    &temp_dest,
                    headers.clone(),
                    md5_expected,
                )
                .await
            };

            match res {
                Ok(CoreResult::Success(_)) => {
                    update_progress(task_id, 0, None, Some("verifying"));
                    if let Err(error) = verify_temp_download(&temp_dest).await {
                        remove_file_if_exists(&temp_dest).await;
                        if is_trivial_candidate_failure(&temp_dest, &error) {
                            return Err(error);
                        }
                        if is_non_retryable_candidate_error(&error) {
                            return Err(error);
                        }
                        if retry < 3 {
                            retry += 1;
                            debug!(
                                "DownloaderManager integrity failed; retrying {} for url={} error={}",
                                retry, url, error
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            continue;
                        }
                        return Err(error);
                    }

                    if temp_dest != final_dest {
                        update_progress(task_id, 0, None, Some("renaming"));
                        match rename_overwrite(&temp_dest, &final_dest).await {
                            Ok(()) => return Ok(CoreResult::Success(final_dest.clone())),
                            Err(e) => {
                                debug!(
                                    "rename_overwrite failed src={} dst={} err={}",
                                    temp_dest.to_string_lossy(),
                                    final_dest.to_string_lossy(),
                                    e
                                );
                                return Ok(CoreResult::Success(temp_dest));
                            }
                        }
                    }
                    return Ok(CoreResult::Success(final_dest));
                }
                Ok(CoreResult::Cancelled) => return Ok(CoreResult::Cancelled),
                Ok(CoreResult::Error(e)) => {
                    remove_file_if_exists(&temp_dest).await;
                    if is_trivial_candidate_failure(&temp_dest, &e) {
                        return Ok(CoreResult::Error(e));
                    }
                    if is_non_retryable_candidate_error(&e) {
                        return Ok(CoreResult::Error(e));
                    }
                    if retry < 3 {
                        retry += 1;
                        debug!(
                            "DownloaderManager retrying {} for url={} error={}",
                            retry, url, e
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    return Ok(CoreResult::Error(e));
                }
                Err(e) => {
                    remove_file_if_exists(&temp_dest).await;
                    if is_trivial_candidate_failure(&temp_dest, &e) {
                        return Err(e);
                    }
                    if is_non_retryable_candidate_error(&e) {
                        return Err(e);
                    }
                    if retry < 3 {
                        retry += 1;
                        debug!(
                            "DownloaderManager retrying {} for url={} error={}",
                            retry, url, e
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    pub async fn download_with_url_candidates(
        &self,
        task_id: &str,
        urls: Vec<String>,
        dest: PathBuf,
        headers: Option<HeaderMap>,
        md5_expected: Option<&str>,
    ) -> Result<CoreResult<PathBuf>, CoreError> {
        if urls.is_empty() {
            return Err(CoreError::BadUpdateIdentity);
        }

        let total = urls.len();
        let mut last_error = None;
        for (index, url) in urls.into_iter().enumerate() {
            debug!(
                "DownloaderManager trying download url candidate {}/{}: {}",
                index + 1,
                total,
                url
            );
            match self
                .download_with_options(
                    task_id,
                    url.clone(),
                    dest.clone(),
                    headers.clone(),
                    md5_expected,
                )
                .await
            {
                Ok(CoreResult::Success(path)) => return Ok(CoreResult::Success(path)),
                Ok(CoreResult::Cancelled) => return Ok(CoreResult::Cancelled),
                Ok(CoreResult::Error(error)) => {
                    debug!(
                        "DownloaderManager url candidate failed {}/{} url={} error={}",
                        index + 1,
                        total,
                        url,
                        error
                    );
                    let is_checksum_mismatch = matches!(error, CoreError::ChecksumMismatch(_));
                    last_error = Some(error);
                    if is_checksum_mismatch {
                        break;
                    }
                }
                Err(error) => {
                    debug!(
                        "DownloaderManager url candidate failed {}/{} url={} error={}",
                        index + 1,
                        total,
                        url,
                        error
                    );
                    let is_checksum_mismatch = matches!(error, CoreError::ChecksumMismatch(_));
                    last_error = Some(error);
                    if is_checksum_mismatch {
                        break;
                    }
                }
            }
        }

        Err(last_error.unwrap_or(CoreError::BadUpdateIdentity))
    }

    pub async fn download_single_with_url_candidates(
        &self,
        task_id: &str,
        urls: Vec<String>,
        dest: PathBuf,
        headers: Option<HeaderMap>,
        md5_expected: Option<&str>,
    ) -> Result<CoreResult<PathBuf>, CoreError> {
        if urls.is_empty() {
            return Err(CoreError::BadUpdateIdentity);
        }

        let task_control = task_control(task_id)
            .ok_or_else(|| CoreError::Other(format!("Task control missing for {task_id}")))?;

        update_progress(task_id, 0, None, Some("downloading"));

        let final_dest = resolve_final_dest(&self.client, &urls[0], &dest, headers.as_ref())
            .await
            .unwrap_or(dest);
        let temp_dest = temp_download_path(&final_dest);

        let total = urls.len();
        let mut last_error = None;
        for (index, url) in urls.into_iter().enumerate() {
            debug!(
                "DownloaderManager trying single-thread download url candidate {}/{}: {}",
                index + 1,
                total,
                url
            );
            reset_progress(task_id, None, Some("downloading"));
            remove_file_if_exists(&temp_dest).await;

            match download_file(
                self.client.clone(),
                task_control.clone(),
                task_id,
                &url,
                &temp_dest,
                headers.clone(),
                md5_expected,
            )
            .await
            {
                Ok(CoreResult::Success(_)) => {
                    update_progress(task_id, 0, None, Some("verifying"));
                    if let Err(error) = verify_temp_download(&temp_dest).await {
                        remove_file_if_exists(&temp_dest).await;
                        last_error = Some(error);
                        continue;
                    }

                    if temp_dest != final_dest {
                        update_progress(task_id, 0, None, Some("renaming"));
                        match rename_overwrite(&temp_dest, &final_dest).await {
                            Ok(()) => return Ok(CoreResult::Success(final_dest.clone())),
                            Err(error) => {
                                debug!(
                                    "rename_overwrite failed src={} dst={} err={}",
                                    temp_dest.to_string_lossy(),
                                    final_dest.to_string_lossy(),
                                    error
                                );
                                return Ok(CoreResult::Success(temp_dest));
                            }
                        }
                    }

                    return Ok(CoreResult::Success(final_dest.clone()));
                }
                Ok(CoreResult::Cancelled) => return Ok(CoreResult::Cancelled),
                Ok(CoreResult::Error(error)) | Err(error) => {
                    debug!(
                        "DownloaderManager single-thread url candidate failed {}/{} url={} error={}",
                        index + 1,
                        total,
                        url,
                        error
                    );
                    remove_file_if_exists(&temp_dest).await;
                    last_error = Some(error);
                }
            }
        }

        Err(last_error.unwrap_or(CoreError::BadUpdateIdentity))
    }

    /// manager 创建新的 task 并 spawn 后台执行，立即返回 task_id
    pub fn start_download(
        &self,
        url: String,
        dest: PathBuf,
        md5_expected: Option<String>,
    ) -> String {
        let task_id = create_task_with_options(None, "ready", None, true);
        let client = self.client.clone();

        // clones for task
        let url_clone = url.clone();
        let dest_clone = dest.clone();
        let md5_clone = md5_expected.clone();
        let task_id_clone = task_id.clone();

        let abort_handle = match spawn_download_task(task_id.clone(), async move {
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
        }) {
            Ok(abort_handle) => abort_handle,
            Err(error) => {
                finish_task(&task_id, "error", Some(error));
                return task_id;
            }
        };
        register_task_abort_handle(task_id.clone(), abort_handle);

        task_id
    }
}
