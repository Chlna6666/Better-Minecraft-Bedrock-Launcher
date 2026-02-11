use std::cmp::Ordering;
use std::env;
use std::time::Instant;

use anyhow::{Context, Result};
use fluent_bundle::FluentArgs;
use regex::Regex;
use reqwest::header::CONTENT_LENGTH;
use tokio::time::Duration;
use tracing::{debug, info, warn};

use tokio::io::AsyncWriteExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use windows::core::PCWSTR;
use windows::Management::Deployment::PackageManager;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::http::proxy::get_client_for_proxy;
use crate::http::request::GLOBAL_CLIENT;
use crate::i18n::I18n;
use crate::utils::utils::to_wstr;

const MC_DEPS_WINDOW_LABEL: &str = "mc_deps";
const MC_DEPS_WINDOW_URL: &str = "mc_dependency.html";
const EVENT_MC_DEPS_LOG: &str = "mc-deps-log";
const EVENT_MC_DEPS_PROGRESS: &str = "mc-deps-progress";
const EVENT_MC_DEPS_DONE: &str = "mc-deps-done";

/// 从字符串中提取第一个形如 `d+.d+.d+.d+` 的版本号（比如 `14.0.33519.0`）
fn extract_version(s: &str) -> Option<String> {
    let re = Regex::new(r"(\d+\.\d+\.\d+\.\d+)").ok()?;
    re.captures(s)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

/// 比较两个版本 `a` 和 `b`，格式假定为 `X.Y.Z.W`
fn compare_versions(a: &str, b: &str) -> Ordering {
    let parse_parts = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    let pa = parse_parts(a);
    let pb = parse_parts(b);
    let max_len = std::cmp::max(pa.len(), pb.len());
    for i in 0..max_len {
        let va = *pa.get(i).unwrap_or(&0);
        let vb = *pb.get(i).unwrap_or(&0);
        match va.cmp(&vb) {
            Ordering::Less => return Ordering::Less,
            Ordering::Greater => return Ordering::Greater,
            Ordering::Equal => continue,
        }
    }
    Ordering::Equal
}

/// 检查是否安装了关键词对应的 UWP 包（支持包名前缀匹配）
/// 如果给定了 min_version，会确保存在已安装包的版本 >= min_version
pub fn is_installed_with_min(prefix: &str, min_version: Option<&str>) -> bool {
    let pm = match PackageManager::new() {
        Ok(p) => p,
        Err(e) => {
            debug!("无法创建 PackageManager: {:?}", e);
            return false;
        }
    };

    if let Ok(packages) = pm.FindPackages() {
        for pkg in packages {
            if let Ok(id) = pkg.Id() {
                let mut name_str = "<unknown>".to_string();
                if let Ok(name) = id.Name() {
                    name_str = name.to_string();
                }

                if name_str.starts_with(prefix) {
                    debug!("找到已安装依赖（candidate）: {}", name_str);

                    if let Some(minv) = min_version {
                        match id.Version() {
                            Ok(ver) => {
                                let inst_ver = format!(
                                    "{}.{}.{}.{}",
                                    ver.Major, ver.Minor, ver.Build, ver.Revision
                                );
                                debug!("已安装包版本（来自Id().Version()）: {}", inst_ver);
                                match compare_versions(&inst_ver, minv) {
                                    Ordering::Greater | Ordering::Equal => {
                                        debug!("已安装版本 {} 满足最小版本 {}", inst_ver, minv);
                                        return true;
                                    }
                                    Ordering::Less => {
                                        debug!("已安装版本 {} 小于最小版本 {}", inst_ver, minv);
                                        continue;
                                    }
                                }
                            }
                            Err(_) => {
                                if let Some(inst_ver) = extract_version(&name_str) {
                                    debug!("已安装包版本（来自Name()提取）: {}", inst_ver);
                                    match compare_versions(&inst_ver, minv) {
                                        Ordering::Greater | Ordering::Equal => return true,
                                        Ordering::Less => continue,
                                    }
                                } else {
                                    debug!("无法从已安装包名提取版本: {}", name_str);
                                    continue;
                                }
                            }
                        }
                    } else {
                        return true;
                    }
                }
            }
        }
    }
    debug!(
        "未找到满足条件的依赖前缀: {} (min_version={:?})",
        prefix, min_version
    );
    false
}

fn select_best_candidate(
    mut candidates: Vec<(String, String)>,
    min_version: Option<&str>,
) -> Option<(String, String)> {
    candidates.sort_by(|a, b| {
        let va = extract_version(&a.0);
        let vb = extract_version(&b.0);
        match (va, vb) {
            (Some(va), Some(vb)) => compare_versions(&vb, &va),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    });

    if let Some(minv) = min_version {
        for (name, url) in &candidates {
            if let Some(ver) = extract_version(name) {
                if compare_versions(&ver, minv) != Ordering::Less {
                    return Some((name.clone(), url.clone()));
                }
            } else {
                continue;
            }
        }
        if let Some((name, url)) = candidates.first() {
            warn!(
                "未找到满足最小版本 {} 的候选安装包，回退到最高版本 {} 进行安装（可能仍然会失败）",
                minv, name
            );
            Some((name.clone(), url.clone()))
        } else {
            None
        }
    } else {
        candidates.into_iter().next()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MissingUwpDependency {
    pub name: String,
    pub pfn: String,
    pub min_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McDepsPrompt {
    pub title: String,
    pub main: String,
    pub content: String,
    pub install_button: String,
    pub exit_button: String,
    pub missing: Vec<MissingUwpDependency>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McDepsProgress {
    pub percent: u32,
    pub stage: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McDepsLog {
    pub key: String,
    pub name: Option<String>,
    pub pkg: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McDepsDone {
    pub ok: bool,
    pub message: String,
}

fn emit_log(app: &AppHandle, key: impl Into<String>, name: Option<String>, pkg: Option<String>) {
    let payload = McDepsLog {
        key: key.into(),
        name,
        pkg,
    };
    let _ = app.emit(EVENT_MC_DEPS_LOG, payload);
}

fn emit_progress(app: &AppHandle, percent: u32, stage: impl Into<String>) {
    let payload = McDepsProgress {
        percent: percent.min(100),
        stage: stage.into(),
    };
    let _ = app.emit(EVENT_MC_DEPS_PROGRESS, payload);
}

fn emit_done(app: &AppHandle, ok: bool, message: impl Into<String>) {
    let payload = McDepsDone {
        ok,
        message: message.into(),
    };
    let _ = app.emit(EVENT_MC_DEPS_DONE, payload);
}

fn uwp_deps_list() -> &'static [(&'static str, Option<&'static str>)] {
    &[
        ("Microsoft.VCLibs.140.00", Some("14.0.33519.0")),
        ("Microsoft.NET.Native.Runtime.1.4", None),
        ("Microsoft.NET.Native.Runtime.2.2", Some("2.2.28604.0")),
        ("Microsoft.VCLibs.140.00.UWPDesktop", None),
        ("Microsoft.Services.Store.Engagement", None),
        ("Microsoft.NET.Native.Framework.1.3", None),
        ("Microsoft.NET.Native.Framework.2.2", Some("2.2.29512.0")),
        ("Microsoft.GamingServices", Some("33.108.12001.0")),
    ]
}

fn compute_missing_deps() -> Vec<MissingUwpDependency> {
    uwp_deps_list()
        .iter()
        .copied()
        .filter(|(name, min)| !is_installed_with_min(name, *min))
        .map(|(name, min)| MissingUwpDependency {
            name: name.to_string(),
            pfn: format!("{}_8wekyb3d8bbwe", name),
            min_version: min.map(|s| s.to_string()),
        })
        .collect()
}

pub fn get_or_create_mc_deps_window(app: &AppHandle) -> Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(MC_DEPS_WINDOW_LABEL) {
        return Ok(w);
    }

    let title = I18n::t("mc-deps-title", None);
    let w = WebviewWindowBuilder::new(
        app,
        MC_DEPS_WINDOW_LABEL,
        WebviewUrl::App(MC_DEPS_WINDOW_URL.into()),
    )
    .title(&title)
    .inner_size(640.0, 520.0)
    .min_inner_size(520.0, 420.0)
    .center()
    .resizable(true)
    .decorations(false)
    .transparent(false)
    .shadow(true)
    .visible(false)
    .skip_taskbar(false)
    .build()?;

    Ok(w)
}

pub fn maybe_open_mc_deps_window(app: &AppHandle) -> Result<bool> {
    let missing = compute_missing_deps();
    if missing.is_empty() {
        return Ok(false);
    }

    let w = get_or_create_mc_deps_window(app)?;
    let _ = w.show();
    let _ = w.set_focus();
    let _ = w.unminimize();

    Ok(true)
}

#[tauri::command]
pub fn get_mc_deps_prompt() -> McDepsPrompt {
    let missing = compute_missing_deps();

    let missing_str = missing
        .iter()
        .map(|d| match &d.min_version {
            Some(min) => format!("{} (min {})", d.name, min),
            None => d.name.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut args = FluentArgs::new();
    args.set("missing", missing_str);

    let title = I18n::t("mc-deps-title", None);
    let main = I18n::t("mc-deps-main", None);
    let content = I18n::t("mc-deps-content", Some(&args));
    let install_button = I18n::t("mc-deps-button-install", None);
    let exit_button = I18n::t("mc-deps-button-exit", None);

    McDepsPrompt {
        title,
        main,
        content,
        install_button,
        exit_button,
        missing,
    }
}

#[tauri::command]
pub fn open_ms_store_for_pfn(pfn: String) -> std::result::Result<(), String> {
    let uri = format!("ms-windows-store://pdp/?PFN={}", pfn);
    unsafe {
        ShellExecuteW(
            None,
            PCWSTR(to_wstr("open").as_ptr()),
            PCWSTR(to_wstr(&uri).as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
    Ok(())
}

/// 异步下载并安装一组 UWP 依赖（带下载进度回调显示）
pub async fn download_and_install_deps_async(
    deps: &[(&str, Option<&str>)],
    app: Option<AppHandle>,
) -> Result<()> {
    let client = get_client_for_proxy().unwrap_or_else(|_| GLOBAL_CLIENT.clone());

    let re = Regex::new(r#"<a\s+href=\"(?P<href>[^\"]+)\"[^>]*>(?P<name>[^<]+)</a>"#)?;
    let mut failures: Vec<String> = Vec::new();

    for (idx, &(pkg, min_version)) in deps.iter().enumerate() {
        let pkg_family = if pkg.contains("_8wekyb3d8bbwe") {
            pkg.to_string()
        } else {
            format!("{}{}", pkg, "_8wekyb3d8bbwe")
        };
        debug!(
            "准备请求 PackageFamilyName = {} (deps index {}/{})",
            pkg_family,
            idx + 1,
            deps.len()
        );

        let resp = client
            .post("https://store.rg-adguard.net/api/GetFiles")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Origin", "https://store.rg-adguard.net")
            .header("Referer", "https://store.rg-adguard.net/")
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .form(&[
                ("type", "PackageFamilyName"),
                ("url", &pkg_family),
                ("ring", "RP"),
                ("lang", "en-US"),
            ])
            .send()
            .await
            .with_context(|| format!("请求下载页面失败: {}", pkg_family))?;

        let status = resp.status();
        let resp_text = resp
            .text()
            .await
            .with_context(|| format!("读取页面内容失败: {}", pkg_family))?;

        if !status.is_success() {
            failures.push(format!("{}: HTTP {}", pkg_family, status));
            if let Some(ref app) = app {
                emit_log(app, "source_http_error", None, Some(pkg_family.clone()));
            }
            continue;
        }

        let mut candidates = Vec::new();
        for cap in re.captures_iter(&resp_text) {
            let name = &cap["name"];
            let href = &cap["href"];
            let lname = name.to_lowercase();
            debug!("解析到链接: name = {}, href = {}", name, href);
            if (lname.contains("x64") || lname.contains("neutral"))
                && (lname.ends_with(".appx")
                    || lname.ends_with(".appxbundle")
                    || lname.ends_with(".msixbundle")
                    || lname.ends_with(".msix"))
            {
                candidates.push((name.to_string(), href.to_string()));
            }
        }

        if candidates.is_empty() {
            warn!("未找到匹配的安装包文件: {}，跳过安装", pkg_family);
            failures.push(format!("{}: no candidates", pkg_family));
            if let Some(ref app) = app {
                emit_log(app, "no_candidates", None, Some(pkg_family.clone()));
            }
            continue;
        }

        match select_best_candidate(candidates, min_version) {
            Some((name, url)) => {
                debug!("选择的候选: {} -> {}", name, url);
                info!("开始下载 {} ...", name);
                if let Some(ref app) = app {
                    emit_log(app, "download_start", Some(name.clone()), None);
                    emit_progress(app, 0, "download");
                }

                let temp_dir = env::temp_dir();
                let file_path = temp_dir.join(&name);
                debug!("临时文件路径: {}", file_path.display());

                let mut resp = client
                    .get(&url)
                    .send()
                    .await
                    .with_context(|| format!("下载 {} 失败", name))?;

                let total_len_opt = resp
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok());

                debug!("Content-Length for {} = {:?}", name, total_len_opt);

                let mut file = tokio::fs::File::create(&file_path)
                    .await
                    .with_context(|| format!("创建文件 {} 失败", file_path.display()))?;

                let mut downloaded: u64 = 0;
                let start = Instant::now();
                while let Some(chunk_res) = resp.chunk().await.transpose() {
                    let chunk = chunk_res.with_context(|| format!("读取 {} 的数据块失败", name))?;
                    file.write_all(&chunk)
                        .await
                        .with_context(|| format!("写入文件 {} 失败", file_path.display()))?;
                    downloaded += chunk.len() as u64;

                    if let Some(total) = total_len_opt {
                        let percent = ((downloaded as f64 / total as f64) * 100.0).round() as u32;
                        if let Some(ref app) = app {
                            emit_progress(app, percent, "download");
                        }
                        debug!("下载 {}: {} / {} ({}%)", name, downloaded, total, percent);
                    } else {
                        let pseudo = (downloaded % 100) as u32;
                        if let Some(ref app) = app {
                            emit_progress(app, pseudo, "download");
                        }
                        debug!(
                            "下载 {}: {} bytes (无 Content-Length，可视化位置 {})",
                            name, downloaded, pseudo
                        );
                    }
                }

                let elapsed = start.elapsed();
                info!(
                    "下载完成 {}，总字节 {}，耗时 {:.2?}",
                    name, downloaded, elapsed
                );

                file.flush().await.ok();
                drop(file);

                let install_cmd = format!(
                    "Add-AppxPackage -Path \"{}\" -ForceApplicationShutdown -ErrorAction Stop;",
                    file_path.display()
                );

                let ps_args = format!(
                    "-NoProfile -ExecutionPolicy Bypass -Command \"{}\"",
                    install_cmd
                );

                debug!(
                    "使用 ShellExecuteW 启动 powershell 以管理员权限安装: {}",
                    ps_args
                );
                if let Some(ref app) = app {
                    emit_log(app, "request_admin", Some(name.clone()), None);
                    emit_progress(app, 100, "downloaded");
                }
                unsafe {
                    ShellExecuteW(
                        None,
                        PCWSTR(to_wstr("runas").as_ptr()),
                        PCWSTR(to_wstr("powershell.exe").as_ptr()),
                        PCWSTR(to_wstr(&ps_args).as_ptr()),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    );
                }
                info!("已发起通过 PowerShell 安装 {} 的请求。", name);

                // 等待安装生效：最多等待 60 秒，期间轮询检测包是否已安装/版本满足（同时保持进度显示）
                let wait_deadline = Instant::now() + Duration::from_secs(60);
                let mut installed_ok = false;
                loop {
                    if is_installed_with_min(pkg, min_version) {
                        debug!("检测到 {} 已安装/版本满足", pkg);
                        if let Some(ref app) = app {
                            emit_log(app, "detect_installed", None, Some(pkg.to_string()));
                        }
                        installed_ok = true;
                        break;
                    }
                    if Instant::now() >= wait_deadline {
                        warn!("等待 {} 安装超时（60s），后续启动可能仍会检测为未安装", pkg);
                        if let Some(ref app) = app {
                            emit_log(app, "wait_timeout", None, Some(pkg.to_string()));
                        }
                        break;
                    }

                    if let Some(ref app) = app {
                        emit_progress(app, 90, "installing");
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }

                if let Some(ref app) = app {
                    emit_progress(app, 100, "done-one");
                }
                info!(
                    "安装流程对 {} 的处理已结束（可能成功或超时），请按需检查。",
                    name
                );

                if !installed_ok {
                    failures.push(format!("{}: install not detected", pkg));
                }
            }
            None => {
                warn!(
                    "没有可用的候选来安装 {}（min_version={:?}），跳过",
                    pkg_family, min_version
                );
                failures.push(format!("{}: no selectable candidate", pkg_family));
                continue;
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("deps install failures: {}", failures.join("; ")))
    }
}

#[tauri::command]
pub async fn start_mc_deps_install(app: AppHandle) -> std::result::Result<(), String> {
    let missing = compute_missing_deps();
    if missing.is_empty() {
        emit_done(&app, true, "no-missing-deps");
        return Ok(());
    }

    let owned_missing: Vec<(String, Option<String>)> = missing
        .into_iter()
        .map(|d| (d.name, d.min_version))
        .collect();

    emit_log(&app, "start", None, None);
    emit_progress(&app, 0, "start");

    tauri::async_runtime::spawn(async move {
        let refs: Vec<(&str, Option<&str>)> = {
            let mut v = Vec::with_capacity(owned_missing.len());
            for (k, vopt) in &owned_missing {
                v.push((k.as_str(), vopt.as_deref()));
            }
            v
        };

        let res = download_and_install_deps_async(&refs, Some(app.clone())).await;
        match res {
            Ok(_) => {
                emit_done(&app, true, "");
            }
            Err(e) => {
                emit_done(&app, false, e.to_string());
            }
        }
    });

    Ok(())
}

// Backwards-compatible command names (deprecated): keep for older frontend builds.
#[tauri::command]
pub fn get_appx_deps_prompt() -> McDepsPrompt {
    get_mc_deps_prompt()
}

#[tauri::command]
pub async fn start_appx_deps_install(app: AppHandle) -> std::result::Result<(), String> {
    start_mc_deps_install(app).await
}
