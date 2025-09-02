use std::cmp::Ordering;
use std::env;
use std::fs::File;
use std::io::Write;
use std::process::exit;
use anyhow::{Context, Result};
use fluent_bundle::FluentArgs;
use regex::Regex;
use reqwest::Client;
use tracing::{info, debug, warn};
use tokio::runtime::Runtime;
use tokio::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Controls::{
    TaskDialogIndirect, TASKDIALOG_BUTTON, TASKDIALOG_COMMON_BUTTON_FLAGS, TASKDIALOGCONFIG,
    TDF_ALLOW_DIALOG_CANCELLATION,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{IDNO, IDCANCEL, SW_SHOWNORMAL};
use windows::Management::Deployment::PackageManager;
use crate::i18n::I18n;
use crate::utils::utils::to_wstr;

/// 从字符串中提取第一个形如 `d+.d+.d+.d+` 的版本号（比如 `14.0.33519.0`）
fn extract_version(s: &str) -> Option<String> {
    let re = Regex::new(r"(\d+\.\d+\.\d+\.\d+)").ok()?;
    re.captures(s).and_then(|cap| cap.get(1).map(|m| m.as_str().to_string()))
}

/// 比较两个版本 `a` 和 `b`，格式假定为 `X.Y.Z.W`（各段可为任意非负整数）
/// 返回 Ordering::Greater if a > b, Equal if equal, Less if a < b
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
                // 取到包名用于日志/备用解析
                let mut name_str = "<unknown>".to_string();
                if let Ok(name) = id.Name() {
                    name_str = name.to_string();
                }

                if name_str.starts_with(prefix) {
                    debug!("找到已安装依赖（candidate）: {}", name_str);

                    if let Some(minv) = min_version {
                        // 优先尝试从 id.Version() 获取结构化版本信息（更可靠）
                        // 注意：不同 windows 绑定版本对 Version 的访问可能不同，若编译错误请按提示修改 .Major/.Minor 等访问方式
                        match id.Version() {
                            Ok(ver) => {
                                // ver.Major/Minor/Build/Revision 在多数 windows-rs 绑定中是可直接访问字段
                                // 如果你的版本是方法（如 ver.Major()），请相应调整。
                                let inst_ver = format!("{}.{}.{}.{}", ver.Major, ver.Minor, ver.Build, ver.Revision);
                                debug!("已安装包版本（来自Id().Version()）: {}", inst_ver);
                                match compare_versions(&inst_ver, minv) {
                                    Ordering::Greater | Ordering::Equal => {
                                        debug!("已安装版本 {} 满足最小版本 {}", inst_ver, minv);
                                        return true;
                                    }
                                    Ordering::Less => {
                                        debug!("已安装版本 {} 小于最小版本 {}", inst_ver, minv);
                                        // 继续查找其他已安装包
                                        continue;
                                    }
                                }
                            }
                            Err(_) => {
                                // 回退：尝试从包名中用正则提取版本（原有逻辑）
                                if let Some(inst_ver) = extract_version(&name_str) {
                                    debug!("已安装包版本（来自Name()提取）: {}", inst_ver);
                                    match compare_versions(&inst_ver, minv) {
                                        Ordering::Greater | Ordering::Equal => {
                                            return true;
                                        }
                                        Ordering::Less => {
                                            continue;
                                        }
                                    }
                                } else {
                                    debug!("无法从已安装包名提取版本: {}", name_str);
                                    continue;
                                }
                            }
                        }
                    } else {
                        // 未指定最小版本，只要存在任意匹配前缀的包就认为已安装
                        return true;
                    }
                }
            }
        }
    }
    debug!("未找到满足条件的依赖前缀: {} (min_version={:?})", prefix, min_version);
    false
}

/// 在候选下载文件中选择最合适的一个：
/// - 如果指定了 min_version，优先选择版本 >= min_version（选择最高的满足项）
/// - 否则选取版本最高的候选。如果候选没有版本信息，会回退到原来的顺序选择第一个
fn select_best_candidate(
    mut candidates: Vec<(String, String)>,
    min_version: Option<&str>,
) -> Option<(String, String)> {
    // 尝试把 candidates 按版本排序（没有版本信息的放到后面）
    candidates.sort_by(|a, b| {
        let va = extract_version(&a.0);
        let vb = extract_version(&b.0);
        match (va, vb) {
            (Some(va), Some(vb)) => compare_versions(&vb, &va), // 降序（最高版本在前）
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    });

    if let Some(minv) = min_version {
        // 优先找第一个 >= min_version（因为已按降序排序，找到的将是最高的满足项）
        for (name, url) in &candidates {
            if let Some(ver) = extract_version(name) {
                if compare_versions(&ver, minv) != Ordering::Less {
                    return Some((name.clone(), url.clone()));
                }
            } else {
                // 无法从文件名提取版本，跳过（不能确认是否满足）
                continue;
            }
        }
        // 如果没有任何候选满足 min_version，选最高版本（如有）并发出警告
        if let Some((name, url)) = candidates.first() {
            warn!(
                "未找到满足最小版本 {} 的候选安装包，回退到最高版本 {} 进行安装（可能仍然会失败）",
                minv, name
            );
            return Some((name.clone(), url.clone()));
        } else {
            return None;
        }
    } else {
        // 没有最小版本要求，返回排序后的第一个（最高版本或第一个没有版本信息的）
        candidates.into_iter().next()
    }
}

/// 异步下载并安装一组 UWP 依赖
/// deps: &[("PackageBaseName", Option<"min_version">)]
pub async fn download_and_install_deps_async(deps: &[(&str, Option<&str>)]) -> Result<()> {
    let client = Client::builder()
        .user_agent("rust-uwp-dep-installer")
        .build()?;

    let re = Regex::new(r#"<a\s+href=\"(?P<href>[^\"]+)\"[^>]*>(?P<name>[^<]+)</a>"#)?;

    for &(pkg, min_version) in deps {
        let pkg_family = if pkg.contains("_8wekyb3d8bbwe") {
            pkg.to_string()
        } else {
            format!("{}{}", pkg, "_8wekyb3d8bbwe")
        };
        debug!("准备请求 PackageFamilyName = {}", pkg_family);

        let resp_text = client
            .post("https://store.rg-adguard.net/api/GetFiles")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("type", "PackageFamilyName"),
                ("url", &pkg_family),
                ("ring", "RP"),
                ("lang", "en-US"),
            ])
            .send().await
            .with_context(|| format!("请求下载页面失败: {}", pkg_family))?
            .text().await
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

        // 选择最合适的候选（考虑最小版本）
        match select_best_candidate(candidates, min_version) {
            Some((name, url)) => {
                debug!("选择的候选: {} -> {}", name, url);
                info!("下载 {} ...", name);

                // 获取临时目录路径
                let temp_dir = env::temp_dir();
                let file_path = temp_dir.join(&name);

                // 下载并保存到临时目录
                let bytes = client
                    .get(&url)
                    .send().await
                    .with_context(|| format!("下载 {} 失败", name))?
                    .bytes().await
                    .with_context(|| format!("读取 {} 数据失败", name))?;

                let mut file = File::create(&file_path)
                    .with_context(|| format!("创建文件 {} 失败", file_path.display()))?;
                file.write_all(&bytes)
                    .with_context(|| format!("写入文件 {} 失败", file_path.display()))?;

                debug!("依赖 {} 已保存到缓存目录: {}", name, file_path.display());

                // 将 PowerShell 安装命令路径指向临时目录
                let install_cmd = format!(
                    "Add-AppxPackage -Path \"{}\" -ForceApplicationShutdown -ErrorAction Stop;",
                    file_path.display()
                );

                let ps_args = format!(
                    "-NoProfile -ExecutionPolicy Bypass -Command \"{}\"",
                    install_cmd
                );

                debug!("使用 ShellExecuteW 启动 powershell 以管理员权限安装: {}", ps_args);
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
                info!("已发起通过 PowerShell 安装 {} 的请求，开始等待安装完成...", name);

                // 等待安装生效：最多等待 60 秒，期间轮询检测包是否已安装/版本满足
                let wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                loop {
                    if is_installed_with_min(pkg, min_version) {
                        debug!("检测到 {} 已安装/版本满足", pkg);
                        break;
                    }
                    if std::time::Instant::now() >= wait_deadline {
                        warn!("等待 {} 安装超时（60s），后续启动可能仍会检测为未安装", pkg);
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }

                info!("安装流程对 {} 的处理已结束（可能成功或超时），请按需检查。", name);
            }
            None => {
                warn!("没有可用的候选来安装 {}（min_version={:?}），跳过", pkg_family, min_version);
                continue;
            }
        }
    }
    Ok(())
}

/// 确保依赖已安装，否则弹窗提示并可自动下载安装
pub fn ensure_uwp_dependencies_or_prompt() {
    // deps 列表可在这里填写最小版本（如果需要）
    let deps: &[(&str, Option<&str>)] = &[
        ("Microsoft.VCLibs.140.00", Some("14.0.33519.0")),
        ("Microsoft.NET.Native.Runtime.1.4", None),
        ("Microsoft.VCLibs.140.00.UWPDesktop", None),
        ("Microsoft.Services.Store.Engagement", None),
        ("Microsoft.NET.Native.Framework.1.3", None),
    ];

    // 找出缺失或版本不满足的依赖
    let missing: Vec<(&str, Option<&str>)> = deps
        .iter()
        .copied()
        .filter(|(k, min)| !is_installed_with_min(k, *min))
        .collect();

    if missing.is_empty() {
        info!("所有 UWP 依赖均已安装并满足最小版本要求（如有）");
        return;
    }

    debug!("缺失或版本不足依赖: {:?}", missing.iter().map(|(k, v)| format!("{}:{:?}", k, v)).collect::<Vec<_>>());

    let missing_str = missing.iter().map(|(k, v)| {
        if let Some(min) = v { format!("{} (min {})", k, min) } else { k.to_string() }
    }).collect::<Vec<_>>().join("\n");

    let mut args = FluentArgs::new();
    args.set("missing", missing_str);

    // 初始提示用户
    let title = to_wstr(&I18n::t("appx-deps-title", None));
    let instr = to_wstr(&I18n::t("appx-deps-main", None));
    let mut content_str = I18n::t("appx-deps-content", Some(&args));
    content_str.push_str("\n\n");
    content_str.push_str(&I18n::t("appx-deps-choices", None));
    let content = to_wstr(&content_str);

    let btn_auto = to_wstr(&I18n::t("appx-deps-button-install", None));
    let btn_exit = to_wstr(&I18n::t("appx-deps-button-exit", None));

    let buttons = [
        TASKDIALOG_BUTTON { nButtonID: IDNO.0, pszButtonText: PCWSTR(btn_auto.as_ptr()) },
        TASKDIALOG_BUTTON { nButtonID: IDCANCEL.0, pszButtonText: PCWSTR(btn_exit.as_ptr()) },
    ];

    let mut cfg: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
    cfg.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
    cfg.hwndParent = HWND(std::ptr::null_mut());
    cfg.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
    cfg.dwCommonButtons = TASKDIALOG_COMMON_BUTTON_FLAGS(0);
    cfg.pszWindowTitle = PCWSTR(title.as_ptr());
    cfg.pszMainInstruction = PCWSTR(instr.as_ptr());
    cfg.pszContent = PCWSTR(content.as_ptr());
    cfg.cButtons = buttons.len() as u32;
    cfg.pButtons = buttons.as_ptr();
    cfg.nDefaultButton = IDNO.0;

    let mut pressed = 0;
    unsafe { let _ = TaskDialogIndirect(&mut cfg, Some(&mut pressed), None, None); };

    if pressed == IDNO.0 {
        // 执行安装流程并捕获结果
        let rt = Runtime::new().unwrap();
        // 将 missing 转为调用函数可接受的切片类型
        let missing_slice: Vec<(&str, Option<&str>)> = missing.into_iter().collect();
        let res = rt.block_on(download_and_install_deps_async(&missing_slice));

        // 安装完成提示
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
        let ok_button = [TASKDIALOG_BUTTON { nButtonID: IDNO.0, pszButtonText: PCWSTR(btn_ok.as_ptr()) }];
        let mut cfg2: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        cfg2.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        cfg2.hwndParent = HWND(std::ptr::null_mut());
        cfg2.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
        cfg2.dwCommonButtons = TASKDIALOG_COMMON_BUTTON_FLAGS(0);
        cfg2.pszWindowTitle = PCWSTR(title2.as_ptr());
        cfg2.pszMainInstruction = PCWSTR(instr2.as_ptr());
        cfg2.pszContent = PCWSTR(content2.as_ptr());
        cfg2.cButtons = ok_button.len() as u32;
        cfg2.pButtons = ok_button.as_ptr();
        cfg2.nDefaultButton = IDNO.0;
        unsafe { let _ = TaskDialogIndirect(&mut cfg2, None, None, None); };

        exit(0);
    } else {
        info!("用户取消安装依赖，程序退出");
        exit(0);
    }
}
