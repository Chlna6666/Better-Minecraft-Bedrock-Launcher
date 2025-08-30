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

/// 检查是否安装了关键词对应的 UWP 包（支持包名前缀匹配）
pub fn is_installed(prefix: &str) -> bool {
    let pm = PackageManager::new().unwrap();
    if let Ok(packages) = pm.FindPackages() {
        for pkg in packages {
            if let Ok(id) = pkg.Id() {
                if let Ok(name) = id.Name() {
                    let name_str = name.to_string();
                    if name_str.starts_with(prefix) {
                        debug!("已找到依赖: {}", name_str);
                        return true;
                    }
                }
            }
        }
    }
    debug!("未找到依赖前缀: {}", prefix);
    false
}

/// 异步下载并安装一组 UWP 依赖
pub async fn download_and_install_deps_async(deps: &[&str]) -> Result<()> {
    let client = Client::builder()
        .user_agent("rust-uwp-dep-installer")
        .build()?;

    let re = Regex::new(r#"<a\s+href=\"(?P<href>[^\"]+)\"[^>]*>(?P<name>[^<]+)</a>"#)?;

    for &pkg in deps {
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
                || lname.ends_with(".msixbundle"))
            {
                candidates.push((name.to_string(), href.to_string()));
            }
        }
        candidates.sort_by_key(|(n, _)| n.clone());
        debug!("候选文件列表: {:?}", candidates.iter().map(|(n, _)| n).collect::<Vec<_>>());

        if candidates.is_empty() {
            warn!("未找到匹配的安装包文件: {}，跳过安装", pkg_family);
            continue;
        }

        for (name, url) in &candidates {
            debug!("GET 请求 URL: {}", url);
            info!("下载 {} ...", name);

            // 获取临时目录路径
            let temp_dir = env::temp_dir();
            let file_path = temp_dir.join(name);

            // 下载并保存到临时目录
            let bytes = client
                .get(url)
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

            debug!("执行 PowerShell 安装: {}", ps_args);
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
            info!("已通过 PowerShell 安装 {} 相关依赖", name);
        }
    }
    Ok(())
}

/// 确保依赖已安装，否则弹窗提示
pub fn ensure_uwp_dependencies_or_prompt() {
    let deps = [
        "Microsoft.VCLibs.140.00",
        "Microsoft.NET.Native.Runtime.1.4",
        "Microsoft.VCLibs.140.00.UWPDesktop",
        "Microsoft.Services.Store.Engagement",
        "Microsoft.NET.Native.Framework.1.3",
    ];
    let missing: Vec<&str> = deps.iter().copied().filter(|k| !is_installed(k)).collect();
    if missing.is_empty() {
        info!("所有 UWP 依赖均已安装");
        return;
    }

    debug!("缺失依赖: {:?}", missing);

    let missing_str = missing.join("\n");
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
        let res = rt.block_on(download_and_install_deps_async(&missing));

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

