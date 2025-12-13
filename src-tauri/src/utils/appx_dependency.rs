use std::cmp::Ordering;
use std::env;
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use fluent_bundle::FluentArgs;
use lazy_static::lazy_static;
use regex::Regex;
use reqwest::header::CONTENT_LENGTH;
use reqwest::Client;
use tokio::runtime::Runtime;
use tokio::time::Duration;
use tracing::{debug, error, info, warn};

use tokio::io::AsyncWriteExt;
use windows::core::PCWSTR;
use windows::Management::Deployment::PackageManager;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TASKDIALOG_COMMON_BUTTON_FLAGS,
    TASKDIALOG_FLAGS,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageW, IDCANCEL, IDNO, SW_SHOWNORMAL, WM_USER,
};

use crate::i18n::I18n;
use crate::utils::utils::to_wstr;

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
lazy_static! {
    static ref TASK_DIALOG_HWND_RAW: Mutex<Option<isize>> = Mutex::new(None);
}

extern "system" fn task_dialog_callback(
    hwnd: HWND,
    msg: u32,
    _wparam: usize,
    _lparam: isize,
    _lp_ref_data: isize,
) -> i32 {
    const TDN_CREATED: u32 = 0;
    if msg == TDN_CREATED {
        if let Ok(mut g) = TASK_DIALOG_HWND_RAW.lock() {
            *g = Some(hwnd.0 as isize);
        }
    }
    0
}

fn update_task_dialog_progress(pos: u32) {
    if let Ok(g) = TASK_DIALOG_HWND_RAW.lock() {
        if let Some(raw) = *g {
            let hwnd = HWND(raw as *mut _);
            const TDM_SET_PROGRESS_BAR_POS: u32 = WM_USER + 105;
            unsafe {
                let _res = SendMessageW(
                    hwnd,
                    TDM_SET_PROGRESS_BAR_POS,
                    Some(WPARAM(pos as usize)),
                    Some(LPARAM(0)),
                );
            }
        }
    }
}

/// 异步下载并安装一组 UWP 依赖（带下载进度回调显示）
pub async fn download_and_install_deps_async(deps: &[(&str, Option<&str>)]) -> Result<()> {
    let client = Client::builder()
        .user_agent("rust-uwp-dep-installer")
        .build()?;

    let re = Regex::new(r#"<a\s+href=\"(?P<href>[^\"]+)\"[^>]*>(?P<name>[^<]+)</a>"#)?;

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

        let resp_text = client
            .post("https://store.rg-adguard.net/api/GetFiles")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("type", "PackageFamilyName"),
                ("url", &pkg_family),
                ("ring", "RP"),
                ("lang", "en-US"),
            ])
            .send()
            .await
            .with_context(|| format!("请求下载页面失败: {}", pkg_family))?
            .text()
            .await
            .with_context(|| format!("读取页面内容失败: {}", pkg_family))?;

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
            continue;
        }

        match select_best_candidate(candidates, min_version) {
            Some((name, url)) => {
                debug!("选择的候选: {} -> {}", name, url);
                info!("开始下载 {} ...", name);

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
                        update_task_dialog_progress(percent);
                        debug!("下载 {}: {} / {} ({}%)", name, downloaded, total, percent);
                    } else {
                        let pseudo = (downloaded % 100) as u32;
                        update_task_dialog_progress(pseudo);
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
                loop {
                    if is_installed_with_min(pkg, min_version) {
                        debug!("检测到 {} 已安装/版本满足", pkg);
                        break;
                    }
                    if Instant::now() >= wait_deadline {
                        warn!("等待 {} 安装超时（60s），后续启动可能仍会检测为未安装", pkg);
                        break;
                    }

                    update_task_dialog_progress(90);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }

                update_task_dialog_progress(100);
                info!(
                    "安装流程对 {} 的处理已结束（可能成功或超时），请按需检查。",
                    name
                );
            }
            None => {
                warn!(
                    "没有可用的候选来安装 {}（min_version={:?}），跳过",
                    pkg_family, min_version
                );
                continue;
            }
        }
    }
    Ok(())
}

/// 确保依赖已安装，否则弹窗提示并可自动下载安装（主流程）
pub fn ensure_uwp_dependencies_or_prompt() {
    let deps: &[(&str, Option<&str>)] = &[
        ("Microsoft.VCLibs.140.00", Some("14.0.33519.0")),
        ("Microsoft.NET.Native.Runtime.1.4", None),
        ("Microsoft.VCLibs.140.00.UWPDesktop", None),
        ("Microsoft.Services.Store.Engagement", None),
        ("Microsoft.NET.Native.Framework.1.3", None),
        // ("Microsoft.Services.Store.Engagement",None),
    ];

    let missing: Vec<(&str, Option<&str>)> = deps
        .iter()
        .copied()
        .filter(|(k, min)| !is_installed_with_min(k, *min))
        .collect();

    if missing.is_empty() {
        info!("所有 UWP 依赖均已安装并满足最小版本要求（如有）");
        return;
    }

    debug!(
        "缺失或版本不足依赖: {:?}",
        missing
            .iter()
            .map(|(k, v)| format!("{}:{:?}", k, v))
            .collect::<Vec<_>>()
    );

    let missing_str = missing
        .iter()
        .map(|(k, v)| {
            if let Some(min) = v {
                format!("{} (min {})", k, min)
            } else {
                k.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut args = FluentArgs::new();
    args.set("missing", missing_str);

    let title = to_wstr(&I18n::t("appx-deps-title", None));
    let instr = to_wstr(&I18n::t("appx-deps-main", None));
    let mut content_str = I18n::t("appx-deps-content", Some(&args));
    content_str.push_str("\n\n");
    content_str.push_str(&I18n::t("appx-deps-choices", None));
    let content = to_wstr(&content_str);

    let btn_auto = to_wstr(&I18n::t("appx-deps-button-install", None));
    let btn_exit = to_wstr(&I18n::t("appx-deps-button-exit", None));

    let buttons = [
        TASKDIALOG_BUTTON {
            nButtonID: IDNO.0,
            pszButtonText: PCWSTR(btn_auto.as_ptr()),
        },
        TASKDIALOG_BUTTON {
            nButtonID: IDCANCEL.0,
            pszButtonText: PCWSTR(btn_exit.as_ptr()),
        },
    ];

    const TDF_ALLOW_DIALOG_CANCELLATION_LOCAL: u32 = 0x0008;
    const TDF_SHOW_PROGRESS_BAR_LOCAL: u32 = 0x0020;

    let mut cfg: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
    cfg.cbSize = size_of::<TASKDIALOGCONFIG>() as u32;
    cfg.hwndParent = HWND(std::ptr::null_mut());
    cfg.dwFlags = TASKDIALOG_FLAGS(
        (TDF_ALLOW_DIALOG_CANCELLATION_LOCAL | TDF_SHOW_PROGRESS_BAR_LOCAL) as i32,
    );
    cfg.dwCommonButtons = TASKDIALOG_COMMON_BUTTON_FLAGS(0);
    cfg.pszWindowTitle = PCWSTR(title.as_ptr());
    cfg.pszMainInstruction = PCWSTR(instr.as_ptr());
    cfg.pszContent = PCWSTR(content.as_ptr());
    cfg.cButtons = buttons.len() as u32;
    cfg.pButtons = buttons.as_ptr();
    cfg.nDefaultButton = IDNO.0;

    let mut pressed: i32 = 0;
    unsafe {
        let _ = TaskDialogIndirect(&mut cfg, Some(&mut pressed), None, None);
    }

    if pressed == IDNO.0 {
        info!("用户选择自动安装，开始下载并安装缺失依赖。");

        let owned_missing: Vec<(String, Option<String>)> = missing
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.map(|s| s.to_string())))
            .collect();

        std::thread::spawn(move || {
            let rt = match Runtime::new() {
                Ok(r) => r,
                Err(e) => {
                    error!("无法创建 Tokio runtime: {:?}", e);
                    // 弹错误对话后退出（与原逻辑一致）
                    let title = to_wstr("安装失败");
                    let instr = to_wstr("无法创建后台运行环境。");
                    let content = to_wstr(&format!("{:?}", e));
                    let btn_ok = to_wstr("确定");
                    let ok_button = [TASKDIALOG_BUTTON {
                        nButtonID: IDNO.0,
                        pszButtonText: PCWSTR(btn_ok.as_ptr()),
                    }];
                    let mut cfg_err: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
                    cfg_err.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
                    cfg_err.hwndParent = HWND(std::ptr::null_mut());
                    cfg_err.dwFlags = TASKDIALOG_FLAGS(0);
                    cfg_err.dwCommonButtons = TASKDIALOG_COMMON_BUTTON_FLAGS(0);
                    cfg_err.pszWindowTitle = PCWSTR(title.as_ptr());
                    cfg_err.pszMainInstruction = PCWSTR(instr.as_ptr());
                    cfg_err.pszContent = PCWSTR(content.as_ptr());
                    cfg_err.cButtons = ok_button.len() as u32;
                    cfg_err.pButtons = ok_button.as_ptr();
                    cfg_err.nDefaultButton = IDNO.0;
                    unsafe {
                        let _ = TaskDialogIndirect(&mut cfg_err, None, None, None);
                    };
                    std::process::exit(1);
                }
            };

            let refs: Vec<(&str, Option<&str>)> = {
                let mut v = Vec::with_capacity(owned_missing.len());
                for (k, vopt) in &owned_missing {
                    v.push((k.as_str(), vopt.as_deref()));
                }
                v
            };

            // 执行异步下载/安装
            let res = rt.block_on(download_and_install_deps_async(&refs));

            let (title2, instr2, content2) = match res {
                Ok(_) => (
                    to_wstr("安装成功"),
                    to_wstr("依赖已成功安装。"),
                    to_wstr("请重新启动程序以应用更改。"),
                ),
                Err(e) => (
                    to_wstr("安装失败"),
                    to_wstr("依赖安装过程中出现错误："),
                    to_wstr(&format!("{}", e)),
                ),
            };

            let btn_ok = to_wstr("确定");
            let ok_button = [TASKDIALOG_BUTTON {
                nButtonID: IDNO.0,
                pszButtonText: PCWSTR(btn_ok.as_ptr()),
            }];
            let mut cfg2: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
            cfg2.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
            cfg2.hwndParent = HWND(std::ptr::null_mut());
            cfg2.dwFlags = TASKDIALOG_FLAGS(0);
            cfg2.dwCommonButtons = TASKDIALOG_COMMON_BUTTON_FLAGS(0);
            cfg2.pszWindowTitle = PCWSTR(title2.as_ptr());
            cfg2.pszMainInstruction = PCWSTR(instr2.as_ptr());
            cfg2.pszContent = PCWSTR(content2.as_ptr());
            cfg2.cButtons = ok_button.len() as u32;
            cfg2.pButtons = ok_button.as_ptr();
            cfg2.nDefaultButton = IDNO.0;
            unsafe {
                let _ = TaskDialogIndirect(&mut cfg2, None, None, None);
            };

            std::process::exit(0);
        });

        return;
    } else {
        info!("用户取消安装依赖，程序退出");
        std::process::exit(0);
    }
}
