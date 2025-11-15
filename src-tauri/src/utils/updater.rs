// src-tauri/src/updater.rs
use anyhow::{Result};
use semver::Version;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command};
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tracing::{debug, error, info};
use regex::Regex;
use crate::downloads::manager::DownloaderManager;
use crate::http::proxy::{get_client_for_proxy};
use crate::result::{CoreError, CoreResult};
use crate::tasks::task_manager::{create_task, finish_task};
use crate::config::config::{read_config};

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
pub async fn check_updates(
    owner: String,
    repo: String,
    api_base: Option<String>,
) -> Result<serde_json::Value, String> {
    let config = read_config()
        .map_err(|e| format!("读取配置失败: {}", e))?;
    
    let update_channel = config.launcher.update_channel;
    let channel = match update_channel {
        // 使用全限定路径以防命名空间问题（根据你的项目路径调整）
        crate::config::config::UpdateChannel::Nightly => "nightly".to_string(),
        _ => "stable".to_string(),
    };
    let channel = channel.to_lowercase();

    info!("检查更新：{}/{} (api_base={:?}, channel={:?})", owner, repo, api_base, update_channel);
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
                let size = a.size;
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
            body: r.body.clone(),
        };

        if r.prerelease {
            // 选择最新的 prerelease（按 semver 比较 prerelease 内部顺序）
            let take = match &latest_prerelease_ver {
                Some(prev_v) => parsed_ver > *prev_v,
                None => true,
            };
            if take {
                latest_prerelease = Some(summary);
                latest_prerelease_ver = Some(parsed_ver);
            }
        } else {
            // 选择最新的 stable
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

    // 根据选择的 channel 决定是否有更新
    let mut update_available = false;
    let mut selected_release: Option<ReleaseSummary> = None;
    if channel == "nightly" {
        // 优先使用 prerelease（nightly）作为候选
        if let Some(ref npv) = latest_prerelease_ver {
            // semver 规则：pre-release 通常比同号正式版优先级低，
            // 但我们希望 nightly（例如 0.0.7-nightly.20251115）在 same major.minor.patch 下也能被视为“更新”。
            let newer = if npv > &current_ver {
                true
            } else {
                // 如果 major.minor.patch 相同，且 nightly 有 pre-release，而当前为正式版（no pre），则认为 nightly 可作为更新
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
            // 没有 prerelease 时回退到 stable
            if let Some(ref ls) = latest_stable_ver {
                if ls > &current_ver {
                    update_available = true;
                }
                selected_release = latest_stable.clone();
            }
        }
    } else {
        // stable channel
        if let Some(ref ls) = latest_stable_ver {
            if ls > &current_ver {
                update_available = true;
            }
            selected_release = latest_stable.clone();
        } else {
            // 没有 stable 时回退到 prerelease
            if let Some(ref npv) = latest_prerelease_ver {
                if npv > &current_ver {
                    update_available = true;
                } else {
                    // same-core nightly fallback logic for stable channel not required normally
                }
                selected_release = latest_prerelease.clone();
            }
        }
    }

    let latest_stable_changelog = latest_stable.as_ref().and_then(|s| s.body.clone());
    let latest_prerelease_changelog = latest_prerelease.as_ref().and_then(|s| s.body.clone());

    debug!("当前版本：{}", current);
    debug!("最新稳定版本：{:?}", latest_stable_ver);
    debug!("最新 prerelease：{:?}", latest_prerelease_ver);
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
    let client = get_client_for_proxy()
        .map_err(|e| format!("构建 HTTP 客户端失败: {}", e))?;
    let manager = DownloaderManager::with_client(client);
    let task_id = create_task("ready", None);
    let res = manager
        .download_with_options(
            &task_id,
            url,
            target.clone(),
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
            finish_task(&task_id, "cancelled", Some("download cancelled".into()));
            let _ = fs::remove_file(&target);
            return Err("下载已取消".to_string());
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
    // 现在应用更新（复用 apply_update 逻辑）
    let src = normalize_file_arg(&target.to_string_lossy()).map_err(|e| format!("处理下载路径失败: {}", e))?;
    let dst = if target_exe_path.trim().is_empty() {
        std::env::current_exe().map_err(|e| format!("获取当前 exe 失败: {}", e))?
    } else {
        normalize_file_arg(&target_exe_path).map_err(|e| format!("处理目标路径失败: {}", e))?
    };
    let exe = std::env::current_exe().map_err(|e| format!("获取 current_exe 失败: {}", e))?;
    let exe_str = exe.to_string_lossy().to_string();
    info!("[rust] download_and_apply called: exe='{}' src='{}' dst='{}' timeout={} auto_quit={}",
          exe_str, src.display(), dst.display(), timeout_secs, auto_quit);
    let updater_filename = format!("updater_runner_{}.exe", std::process::id());
    let updater_path = downloads_dir.join(&updater_filename);
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
        "log": "tracing",
        "task_id": task_id
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
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();

                // 跳过当前进程的 updater_runner_<pid>.exe
                if file_name.starts_with("updater_runner_") && file_name.ends_with(".exe") {
                    if let Some(pid_str) = file_name.strip_prefix("updater_runner_").and_then(|s| s.strip_suffix(".exe")) {
                        if pid_str == pid.to_string() {
                            continue;
                        }
                    }
                }

                // 只删除指定后缀文件
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if !["exe", "msi", "zip", "7z", "bin"].contains(&ext.as_str()) {
                    continue;
                }

                // 直接删除，不判断时间
                match fs::remove_file(&path) {
                    Ok(_) => info!("清理旧版本文件: {}", path.display()),
                    Err(e) => info!("删除旧版本文件失败: {} ; err={}", path.display(), e),
                }
            }
        }
    }
}
