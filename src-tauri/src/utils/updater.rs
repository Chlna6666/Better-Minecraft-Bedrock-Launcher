use crate::config::config::read_config;
use crate::downloads::manager::DownloaderManager;
use crate::http::proxy::get_client_for_proxy;
use crate::result::{CoreResult};
use crate::tasks::task_manager::{create_task, finish_task};
use crate::utils::cloudflare::get_optimized_ip;
use anyhow::Result;
use regex::Regex;
use semver::Version;
use serde::Deserialize;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tracing::{debug, error, info, warn};

#[derive(Deserialize, Debug)]
pub struct ApplyUpdateArgs {
    #[serde(alias = "downloaded_path", alias = "downloadedPath")]
    pub downloaded_path: String,
    #[serde(alias = "target_exe_path", alias = "targetExePath")]
    pub target_exe_path: Option<String>,
    #[serde(alias = "timeout_secs", alias = "timeoutSecs")]
    pub timeout_secs: Option<u64>,
    #[serde(alias = "auto_quit", alias = "autoQuit")]
    pub auto_quit: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct DownloadAndApplyArgs {
    pub url: String,
    pub filename_hint: Option<String>,
    #[serde(alias = "target_exe_path", alias = "targetExePath")]
    pub target_exe_path: Option<String>,
    #[serde(alias = "timeout_secs", alias = "timeoutSecs")]
    pub timeout_secs: Option<u64>,
    #[serde(alias = "auto_quit", alias = "autoQuit")]
    pub auto_quit: Option<bool>,
    pub task_id: Option<String>,
}

#[derive(Deserialize, Debug)]
struct GitHubAsset {
    browser_download_url: String,
    name: String,
    size: u64,
}

#[derive(Deserialize, Debug)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    prerelease: bool,
    published_at: Option<String>,
    assets: Vec<GitHubAsset>,
    body: Option<String>,
}

#[derive(serde::Serialize, Debug, Clone)]
pub struct ReleaseSummary {
    pub tag: String,
    pub name: Option<String>,
    pub prerelease: bool,
    pub published_at: Option<String>,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub asset_size: Option<u64>,
    pub body: Option<String>,
}

#[tauri::command]
pub fn quit_app(app_handle: AppHandle) {
    info!("quit_app 被调用，准备退出应用 (graceful).");
    app_handle.exit(0);
}

#[tauri::command]
pub fn restart_app(app_handle: AppHandle) {
    info!("restart_app 被调用，准备重启应用。");
    match std::env::current_exe() {
        Ok(exe) => {
            let _ = Command::new(exe).spawn();
        }
        Err(e) => {
            warn!("无法获取当前 exe 路径，改为直接退出: {:?}", e);
        }
    }
    app_handle.exit(0);
}

/// 从 tag 中提取第一个可解析的 semver 子串
fn extract_semver_substring(tag: &str) -> Option<String> {
    let t = tag.trim();
    if t.is_empty() {
        return None;
    }
    let mut s = t.trim_start_matches("refs/tags/").trim().to_string();
    s = s.trim_start_matches(|c| c == 'v' || c == 'V').to_string();

    let re = Regex::new(r"(?i)(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.\-]+)?)").unwrap();
    re.captures(&s)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

async fn check_github_is_fast(max_latency_ms: u64) -> bool {
    let url = "https://api.github.com";
    let client_res = get_client_for_proxy();

    if let Ok(client) = client_res {
        let result = tokio::time::timeout(
            Duration::from_millis(max_latency_ms),
            client.head(url).header("User-Agent", "BMCBL-Latency-Check").send()
        ).await;

        match result {
            Ok(Ok(resp)) => {
                if resp.status().is_success() {
                    debug!("GitHub 延迟检测通过: 响应极快 (<= {}ms)", max_latency_ms);
                    true
                } else {
                    warn!("GitHub 延迟检测失败: 状态码 {}", resp.status());
                    false
                }
            }
            Ok(Err(e)) => {
                warn!("GitHub 延迟检测网络错误: {}", e);
                false
            }
            Err(_) => {
                debug!("GitHub 延迟检测超时 (> {}ms)，已强制取消连接", max_latency_ms);
                false
            }
        }
    } else {
        warn!("无法构建 HTTP 客户端用于网络检测");
        false
    }
}

async fn should_use_acceleration() -> bool {
    info!("正在检测 GitHub 连接质量...");
    let is_fast = check_github_is_fast(180).await;
    if is_fast {
        info!("GitHub 连接良好 (<180ms)，使用直连。");
        false
    } else {
        warn!("GitHub 连接缓慢 (>180ms) 或不可达，自动切换至加速通道。");
        true
    }
}

fn accelerate_download_url(url: &str, use_acceleration: bool) -> String {
    if !use_acceleration {
        return url.to_string();
    }
    let proxy_prefix = "https://dl-proxy.bmcbl.com/";

    if url.starts_with("https://github.com") || url.starts_with("https://objects.githubusercontent.com") {
        format!("{}{}", proxy_prefix, url)
    } else {
        url.to_string()
    }
}

#[tauri::command]
pub async fn check_updates(
    owner: String,
    repo: String,
    api_base: Option<String>,
) -> Result<serde_json::Value, String> {

    let use_acceleration = should_use_acceleration().await;

    let final_api_base = if let Some(base) = api_base {
        base
    } else if use_acceleration {
        "https://updater.bmcbl.com".to_string()
    } else {
        "https://api.github.com".to_string()
    };

    let config = read_config().map_err(|e| format!("读取配置失败: {}", e))?;
    let update_channel = config.launcher.update_channel;
    let channel = match update_channel {
        crate::config::config::UpdateChannel::Nightly => "nightly".to_string(),
        _ => "stable".to_string(),
    };
    let channel = channel.to_lowercase();

    info!(
        "检查更新：{}/{} (api_base={}, channel={:?}, accelerated={})",
        owner, repo, final_api_base, update_channel, use_acceleration
    );

    let url = format!(
        "{}/repos/{}/{}/releases",
        final_api_base.trim_end_matches('/'),
        owner,
        repo
    );
    let start_time = std::time::Instant::now();

    let client = if use_acceleration && final_api_base.contains("updater.bmcbl.com") {
        let optimized_ip = get_optimized_ip().await;

        if let Some(ip) = optimized_ip {
            info!("使用优选 IP {} 连接更新 API", ip);
            reqwest::Client::builder()
                .resolve("updater.bmcbl.com", ip)
                .user_agent("BMCBL-Updater")
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| format!("构建优选 HTTP 客户端失败: {}", e))?
        } else {
            get_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?
        }
    } else {
        get_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?
    };

    let resp = client
        .get(&url)
        .header("User-Agent", "BMCBL-Updater")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| format!("HTTP 请求失败 (url={}): {}", url, e))?;
    let duration = start_time.elapsed();

    let status = resp.status();
    debug!("GitHub API 请求 URL: {}", url);
    debug!("GitHub API 响应状态码: {}", status);
    debug!("GitHub API 请求耗时: {:.2?}", duration);

    if !status.is_success() {
        let err_body = resp.text().await.unwrap_or_else(|_| "无法读取响应体".to_string());
        error!("GitHub API 请求异常详情: Status={}, BodyPreview={:.500}", status, err_body);
        return Err(format!("GitHub API 返回错误状态: {}", status));
    }

    let raw_body = resp.text().await.map_err(|e| format!("读取响应内容失败: {}", e))?;
    debug!("GitHub API 响应内容预览: {:.500}...", raw_body);

    let releases: Vec<GitHubRelease> = serde_json::from_str(&raw_body)
        .map_err(|e| {
            error!("JSON 解析失败，收到的完整内容: {}", raw_body);
            format!("解析 JSON 失败: {}", e)
        })?;

    let current = env!("CARGO_PKG_VERSION");
    let current_ver = Version::parse(current).unwrap_or_else(|_| Version::new(0, 0, 0));

    let mut latest_stable: Option<ReleaseSummary> = None;
    let mut latest_prerelease: Option<ReleaseSummary> = None;
    let mut latest_stable_ver: Option<Version> = None;
    let mut latest_prerelease_ver: Option<Version> = None;

    for r in releases {
        let semver_str = match extract_semver_substring(&r.tag_name) {
            Some(s) => s,
            None => {
                info!("无法从 tag 提取 semver，跳过: {}", r.tag_name);
                continue;
            }
        };
        let parsed_ver = match Version::parse(&semver_str) {
            Ok(v) => v,
            Err(e) => {
                info!("解析 semver 失败，跳过 tag={} err={}", r.tag_name, e);
                continue;
            }
        };

        let mut chosen_asset: Option<(String, String, u64)> = None;
        for a in &r.assets {
            let name_l = a.name.to_lowercase();
            if name_l.ends_with(".exe")
                || name_l.ends_with(".msi")
                || name_l.ends_with(".zip")
                || name_l.ends_with(".7z")
            {
                let size = a.size;
                let final_url = accelerate_download_url(&a.browser_download_url, use_acceleration);
                chosen_asset = Some((a.name.clone(), final_url, size));
                break;
            }
        }

        let summary = ReleaseSummary {
            tag: r.tag_name.clone(),
            name: r.name.clone(),
            prerelease: r.prerelease,
            published_at: r.published_at.clone(),
            asset_name: chosen_asset.as_ref().map(|c| c.0.clone()),
            asset_url: chosen_asset.as_ref().map(|c| c.1.clone()),
            asset_size: chosen_asset.as_ref().map(|c| c.2),
            body: r.body.clone(),
        };

        if r.prerelease {
            let take = match &latest_prerelease_ver {
                Some(prev_v) => parsed_ver > *prev_v,
                None => true,
            };
            if take {
                latest_prerelease = Some(summary);
                latest_prerelease_ver = Some(parsed_ver);
            }
        } else {
            let take = match &latest_stable_ver {
                Some(prev_v) => parsed_ver > *prev_v,
                None => true,
            };
            if take {
                latest_stable = Some(summary);
                latest_stable_ver = Some(parsed_ver);
            }
        }
    }

    let mut update_available = false;
    let mut selected_release: Option<ReleaseSummary> = None;
    if channel == "nightly" {
        if let Some(ref npv) = latest_prerelease_ver {
            let newer = if npv > &current_ver {
                true
            } else {
                let same_core = npv.major == current_ver.major
                    && npv.minor == current_ver.minor
                    && npv.patch == current_ver.patch;
                let np_has_pre = !npv.pre.is_empty();
                let cur_has_pre = !current_ver.pre.is_empty();
                same_core && np_has_pre && !cur_has_pre
            };
            if newer {
                update_available = true;
            }
            selected_release = latest_prerelease.clone();
        } else {
            if let Some(ref ls) = latest_stable_ver {
                if ls > &current_ver {
                    update_available = true;
                }
                selected_release = latest_stable.clone();
            }
        }
    } else {
        if let Some(ref ls) = latest_stable_ver {
            if ls > &current_ver {
                update_available = true;
            }
            selected_release = latest_stable.clone();
        } else {
            if let Some(ref npv) = latest_prerelease_ver {
                if npv > &current_ver {
                    update_available = true;
                }
                selected_release = latest_prerelease.clone();
            }
        }
    }

    let latest_stable_changelog = latest_stable.as_ref().and_then(|s| s.body.clone());
    let latest_prerelease_changelog = latest_prerelease.as_ref().and_then(|s| s.body.clone());

    debug!("当前版本：{}", current);
    debug!("是否有更新: {} (channel={})", update_available, channel);

    Ok(serde_json::json!({
        "current_version": current,
        "current_semver_parsed": current_ver.to_string(),
        "selected_channel": channel,
        "selected_release": selected_release,
        "latest_stable": latest_stable,
        "latest_prerelease": latest_prerelease,
        "latest_stable_changelog": latest_stable_changelog,
        "latest_prerelease_changelog": latest_prerelease_changelog,
        "update_available": update_available,
        "is_accelerated": use_acceleration
    }))
}

#[tauri::command]
pub async fn download_and_apply_update(
    args: DownloadAndApplyArgs,
) -> Result<serde_json::Value, String> {
    let url = args.url;
    let filename_hint = args.filename_hint;

    let target_exe_path = args.target_exe_path.unwrap_or_else(|| "".to_string());
    let timeout_secs = args.timeout_secs.unwrap_or(60u64);
    let auto_quit = args.auto_quit.unwrap_or(true);

    let task_id = if let Some(input_id) = args.task_id {
        create_task(Some(input_id), "ready", None)
    } else {
        create_task(None, "ready", None)
    };

    info!("开始下载并应用：url={} task_id={}", url, task_id);

    let downloads_dir = Path::new("./BMCBL/downloads");
    if let Err(e) = fs::create_dir_all(downloads_dir) {
        error!("创建下载目录失败: {}", e);
        return Err(format!("创建下载目录失败: {}", e));
    }

    let fname = filename_hint.unwrap_or_else(|| {
        url.split('/')
            .last()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "download.bin".to_string())
    });
    let target = downloads_dir.join(&fname);
    info!("保存为: {}", target.display());

    // ================== [Client 构建逻辑] ==================
    let use_acceleration = should_use_acceleration().await;

    let client = if use_acceleration && url.contains("dl-proxy.bmcbl.com") {
        let optimized_ip = get_optimized_ip().await;
        if let Some(ip) = optimized_ip {
            info!("使用优选 IP {} 进行下载", ip);
            reqwest::Client::builder()
                .resolve("dl-proxy.bmcbl.com", ip)
                .resolve("updater.bmcbl.com", ip)
                .user_agent("BMCBL-Updater")
                .build()
                .map_err(|e| format!("构建优选下载客户端失败: {}", e))?
        } else {
            get_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?
        }
    } else {
        get_client_for_proxy().map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?
    };
    // ========================================================

    let manager = DownloaderManager::with_client(client);

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("User-Agent", "BMCBL-Updater".parse().unwrap());

    let res = manager
        .download_with_options(
            &task_id,
            url.clone(),
            target.clone(),
            Some(headers),
            None,
        )
        .await;

    let bytes_len = match res {
        Ok(CoreResult::Success(_)) => {
            finish_task(&task_id, "completed", None);
            fs::metadata(&target)
                .map_err(|e| format!("获取文件大小失败: {}", e))?
                .len()
        }
        Ok(CoreResult::Cancelled) => {
            info!("下载任务已取消: {}", task_id);
            finish_task(&task_id, "cancelled", Some("download cancelled".into()));
            let _ = fs::remove_file(&target);
            return Err("Download cancelled".to_string());
        }
        Ok(CoreResult::Error(err)) => {
            finish_task(&task_id, "error", Some(format!("{:?}", err)));
            let _ = fs::remove_file(&target);
            return Err(format!("下载失败: {:?}", err));
        }
        Err(e) => {
            finish_task(&task_id, "error", Some(format!("{:?}", e)));
            let _ = fs::remove_file(&target);
            return Err(format!("下载错误: {:?}", e));
        }
    };

    info!("下载完成: {} bytes", bytes_len);

    let src = normalize_file_arg(&target.to_string_lossy())
        .map_err(|e| format!("处理下载路径失败: {}", e))?;

    let dst = if target_exe_path.trim().is_empty() {
        std::env::current_exe().map_err(|e| format!("获取当前 exe 失败: {}", e))?
    } else {
        normalize_file_arg(&target_exe_path).map_err(|e| format!("处理目标路径失败: {}", e))?
    };

    let exe = std::env::current_exe().map_err(|e| format!("获取 current_exe 失败: {}", e))?;
    let exe_str = exe.to_string_lossy().to_string();

    info!(
        "[rust] download_and_apply called: exe='{}' src='{}' dst='{}' timeout={} auto_quit={}",
        exe_str,
        src.display(),
        dst.display(),
        timeout_secs,
        auto_quit
    );

    let updater_filename = format!("updater_runner_{}.exe", std::process::id());
    let updater_path = downloads_dir.join(&updater_filename);

    let _ = std::fs::remove_file(&updater_path);

    std::fs::copy(&exe, &updater_path).map_err(|e| {
        format!(
            "复制 updater 可执行失败: {} -> {} : {}",
            exe.display(),
            updater_path.display(),
            e
        )
    })?;

    let child = Command::new(updater_path.clone())
        .arg("--run-updater")
        .arg(&src.to_string_lossy().to_string())
        .arg(&dst.to_string_lossy().to_string())
        .arg(timeout_secs.to_string())
        .spawn()
        .map_err(|e| format!("启动更新子进程失败: {}", e))?;

    info!(
        "已启动更新子进程 pid={} (updater bin: {})",
        child.id(),
        updater_path.display()
    );

    if auto_quit {
        let delay_ms = 300u64;
        info!(
            "scheduling process exit in {} ms (pid {})",
            delay_ms,
            std::process::id()
        );
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(delay_ms));
            std::process::exit(0);
        });
    }

    Ok(serde_json::json!({
        "launched": true,
        "pid": child.id(),
        "saved_to": target.to_string_lossy(),
        "bytes": bytes_len,
        "src": src.to_string_lossy(),
        "dst": dst.to_string_lossy(),
        "task_id": task_id
    }))
}

/// 规范化路径 / file:// 前缀处理
fn normalize_file_arg(s: &str) -> Result<PathBuf> {
    let mut t = s.trim().to_string();
    if t.starts_with("file://") {
        t = t.trim_start_matches("file://").to_string();
        if cfg!(windows) && t.starts_with('/') {
            if t.chars().nth(2) == Some(':') {
                t = t.trim_start_matches('/').to_string();
            }
        }
    }
    let p = match Path::new(&t).canonicalize() {
        Ok(pathbuf) => pathbuf,
        Err(_) => PathBuf::from(t),
    };
    Ok(p)
}

pub fn run_updater_child(src: &Path, dst: &Path, timeout: Duration) -> Result<()> {
    info!(
        "run_updater_child start src='{}' dst='{}' timeout={}s",
        src.display(),
        dst.display(),
        timeout.as_secs()
    );

    if !src.exists() {
        error!("源文件不存在: {}", src.display());
        return Err(anyhow::anyhow!("源文件不存在: {}", src.display()));
    }

    let start = Instant::now();
    loop {
        match std::fs::remove_file(dst) {
            Ok(_) => info!("已删除旧目标文件: {}", dst.display()),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    info!("目标文件不存在，准备替换");
                } else {
                    error!(
                        "无法删除目标（可能正在运行）: {} ; err={}",
                        dst.display(),
                        e
                    );
                    if start.elapsed() > timeout {
                        error!("超时退出（删除目标失败）");
                        return Err(anyhow::anyhow!("等待目标释放超时: {}", e));
                    }
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        }

        if let Err(e) = std::fs::rename(src, dst) {
            error!("rename 失败: {}; 尝试 copy", e);
            match std::fs::copy(src, dst) {
                Ok(bytes) => info!("复制成功 ({} bytes)", bytes),
                Err(e2) => {
                    error!("copy 失败: {}", e2);
                    if start.elapsed() > timeout {
                        error!("超时退出（copy 失败）");
                        return Err(anyhow::anyhow!("尝试复制/替换超时: {}", e2));
                    }
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        } else {
            info!("重命名替换成功: {} -> {}", src.display(), dst.display());
        }

        info!("尝试启动新 exe: {}", dst.display());
        match Command::new(dst).spawn() {
            Ok(_) => info!("已成功启动新程序"),
            Err(e) => {
                error!("启动新程序失败: {}", e);
                return Err(anyhow::anyhow!("启动新可执行失败: {}", e));
            }
        }

        info!("替换完成，退出 updater 子进程");
        return Ok(());
    }
}

pub fn clean_old_versions() {
    let downloads_dir = Path::new("./BMCBL/downloads");
    if !downloads_dir.exists() {
        return; // 目录不存在就不清理
    }
    let pid = std::process::id();

    let entries = match fs::read_dir(downloads_dir) {
        Ok(e) => e,
        Err(e) => {
            info!("清理旧版本时读取目录失败: {}", e);
            return;
        }
    };

    for entry_res in entries {
        if let Ok(entry) = entry_res {
            let path = entry.path();
            if path.is_file() {
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();

                if file_name.starts_with("updater_runner_") && file_name.ends_with(".exe") {
                    if let Some(pid_str) = file_name
                        .strip_prefix("updater_runner_")
                        .and_then(|s| s.strip_suffix(".exe"))
                    {
                        if pid_str == pid.to_string() {
                            continue;
                        }
                    }
                }

                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !["exe", "msi", "zip", "7z", "bin"].contains(&ext.as_str()) {
                    continue;
                }

                match fs::remove_file(&path) {
                    Ok(_) => info!("清理旧版本文件: {}", path.display()),
                    Err(e) => info!("删除旧版本文件失败: {} ; err={}", path.display(), e),
                }
            }
        }
    }
}
