// src-tauri/src/updater.rs
use anyhow::{Result};
use semver::Version;
use serde::Deserialize;
use std::fs;
use std::fs::{OpenOptions};
use std::io::{Write};
use std::path::{Path, PathBuf};
use std::process::{Command};
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tracing::{error, info};
use crate::http::proxy::{get_client_for_proxy};

use regex::Regex;
use tauri::ipc::IpcResponse;

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

/// 从 tag 中提取第一个可解析的 semver 子串
fn extract_semver_substring(tag: &str) -> Option<String> {
    let t = tag.trim();
    if t.is_empty() {
        return None;
    }
    // 去掉常见前缀
    let mut s = t.trim_start_matches("refs/tags/").trim().to_string();
    // 去掉单个前导 v/V
    s = s.trim_start_matches(|c| c == 'v' || c == 'V').to_string();

    // 匹配 semver-like 子串（例如 1.2.3, 1.2.3-beta.1, 1.2.3+build）
    let re = Regex::new(r"(?i)(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.\-]+)?)").unwrap();
    re.captures(&s).and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

/// 可选：从 GitHub Releases 获取（api_base 可指向镜像）
#[tauri::command]
pub async fn check_updates(owner: String, repo: String, api_base: Option<String>) -> Result<serde_json::Value, String> {
    info!("检查更新：{}/{} (api_base={:?})", owner, repo, api_base);
    let base = api_base.unwrap_or_else(|| "https://api.github.com".to_string());
    let url = format!("{}/repos/{}/{}/releases", base.trim_end_matches('/'), owner, repo);
    let client = get_client_for_proxy()
        .map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;
    let resp = client
        .get(&url)
        .header("User-Agent", "BMCBL-Updater")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| format!("HTTP 请求失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API 返回状态: {}", resp.status()));
    }

    let releases: Vec<GitHubRelease> = resp.json().await.map_err(|e| format!("解析 JSON 失败: {}", e))?;
    let current = env!("CARGO_PKG_VERSION");
    let current_ver = Version::parse(current).unwrap_or_else(|_| Version::new(0, 0, 0));

    let mut latest_stable: Option<ReleaseSummary> = None;
    let mut latest_prerelease: Option<ReleaseSummary> = None;
    let mut latest_stable_ver: Option<Version> = None;
    let mut latest_prerelease_ver: Option<Version> = None;

    for r in releases {
        // 尝试从 tag 提取 semver 子串
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

        // 选 asset（保持原来的策略），同时记录 asset_size（字节）
        let mut chosen_asset: Option<(String, String, u64)> = None; // (name, url, size)
        for a in &r.assets {
            let name_l = a.name.to_lowercase();
            if name_l.ends_with(".exe") || name_l.ends_with(".msi") || name_l.ends_with(".zip") || name_l.ends_with(".7z") {
                let size = a.size; // 访问字段（已在 struct 中声明）
                chosen_asset = Some((a.name.clone(), a.browser_download_url.clone(), size));
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
            body: r.body.clone(), // 访问字段而非方法
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
    if let Some(ref ls_ver) = latest_stable_ver {
        if ls_ver > &current_ver {
            update_available = true;
        }
    }

    let latest_stable_changelog = latest_stable.as_ref().and_then(|s| s.body.clone());
    let latest_prerelease_changelog = latest_prerelease.as_ref().and_then(|s| s.body.clone());

    Ok(serde_json::json!({
        "current_version": current,
        "current_semver_parsed": current_ver.to_string(),
        "latest_stable": latest_stable,
        "latest_prerelease": latest_prerelease,
        "latest_stable_changelog": latest_stable_changelog,
        "latest_prerelease_changelog": latest_prerelease_changelog,
        "update_available": update_available
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
    info!("开始下载并应用：url={}", url);
    let downloads_dir = Path::new("./BMCBL/downloads");
    if let Err(e) = fs::create_dir_all(downloads_dir) {
        error!("创建下载目录失败: {}", e);
        return Err(format!("创建下载目录失败: {}", e));
    }
    // 决定文件名
    let fname = filename_hint.unwrap_or_else(|| {
        url.split('/')
            .last()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "download.bin".to_string())
    });
    let target = downloads_dir.join(&fname);
    info!("保存为: {}", target.display());
    // 支持代理 via UPDATER_HTTP_PROXY 环境变量
    let client_builder = reqwest::Client::builder();
    let client = if let Ok(proxy) = std::env::var("UPDATER_HTTP_PROXY") {
        client_builder
            .proxy(reqwest::Proxy::all(&proxy).map_err(|e| format!("解析代理失败: {}", e))?)
            .build()
            .map_err(|e| format!("构建 client 失败: {}", e))?
    } else {
        client_builder.build().map_err(|e| format!("构建 client 失败: {}", e))?
    };
    // 同步下载（使用 await）
    let resp = client.get(&url)
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("下载响应失败: {}", resp.status()));
    }
    let bytes = resp.bytes()
        .await
        .map_err(|e| format!("读取下载内容失败: {}", e))?;
    fs::write(&target, &bytes)
        .map_err(|e| format!("写入文件失败: {} : {}", target.display(), e))?;
    info!("下载完成: {} bytes", bytes.len());
    // 现在应用更新（复用 apply_update 逻辑）
    let src = normalize_file_arg(&target.to_string_lossy()).map_err(|e| format!("处理下载路径失败: {}", e))?;
    let dst = if target_exe_path.trim().is_empty() {
        std::env::current_exe().map_err(|e| format!("获取当前 exe 失败: {}", e))?
    } else {
        normalize_file_arg(&target_exe_path).map_err(|e| format!("处理目标路径失败: {}", e))?
    };
    let exe = std::env::current_exe().map_err(|e| format!("获取 current_exe 失败: {}", e))?;
    let exe_str = exe.to_string_lossy().to_string();

    // 使用 tracing 记录调用（不再写入 apply_update_call.log 文件）
    info!("[rust] download_and_apply called: exe='{}' src='{}' dst='{}' timeout={} auto_quit={}",
          exe_str, src.display(), dst.display(), timeout_secs, auto_quit);

    // --- 新增：把当前 exe 拷贝为一个独立的 updater 可执行文件 ---
    let updater_filename = format!("updater_runner_{}.exe", std::process::id());
    let updater_path = downloads_dir.join(&updater_filename);
    // 如果已存在同名文件，先尝试删除（忽略错误）
    let _ = std::fs::remove_file(&updater_path);
    std::fs::copy(&exe, &updater_path)
        .map_err(|e| format!("复制 updater 可执行失败: {} -> {} : {}", exe.display(), updater_path.display(), e))?;
    let child = Command::new(updater_path.clone())
        .arg("--run-updater")
        .arg(&src.to_string_lossy().to_string())
        .arg(&dst.to_string_lossy().to_string())
        .arg(timeout_secs.to_string())
        .spawn()
        .map_err(|e| format!("启动更新子进程失败: {}", e))?;
    info!("已启动更新子进程 pid={} (updater bin: {})", child.id(), updater_path.display());
    info!("已启动更新子进程 pid={}", child.id());
    if auto_quit {
        let delay_ms = 300u64;
        info!("scheduling process exit in {} ms (pid {})", delay_ms, std::process::id());
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            std::process::exit(0);
        });
    }
    Ok(serde_json::json!({
        "launched": true,
        "pid": child.id(),
        "saved_to": target.to_string_lossy(),
        "bytes": bytes.len(),
        "src": src.to_string_lossy(),
        "dst": dst.to_string_lossy(),
        "log": "tracing"
    }))
}

/// 规范化路径 / file:// 前缀处理
fn normalize_file_arg(s: &str) -> Result<PathBuf> {
    let mut t = s.trim().to_string();
    if t.starts_with("file://") {
        t = t.trim_start_matches("file://").to_string();
        // windows style file:///C:/...
        if cfg!(windows) && t.starts_with('/') {
            if t.chars().nth(2) == Some(':') {
                t = t.trim_start_matches('/').to_string();
            }
        }
    }
    // 尝试 canonicalize；如果失败就退化为原始 PathBuf（避免类型推断问题）
    let p = match Path::new(&t).canonicalize() {
        Ok(pathbuf) => pathbuf,
        Err(_) => PathBuf::from(t),
    };
    Ok(p)
}

/// 这个函数用于子进程模式：当程序以 `--run-updater <src> <dst> <timeout>` 启动时，调用该函数执行替换。
pub fn run_updater_child(src: &Path, dst: &Path, timeout: Duration) -> Result<()> {
    info!("run_updater_child start src='{}' dst='{}' timeout={}s",
          src.display(), dst.display(), timeout.as_secs());

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
                    error!("无法删除目标（可能正在运行）: {} ; err={}", dst.display(), e);
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
        match std::process::Command::new(dst).spawn() {
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