use crate::i18n::I18n;
use crate::utils::utils::to_wstr;
use std::process::exit;
use tracing::info;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    KEY_READ,
};
use windows::Win32::UI::Controls::{
    TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TASKDIALOG_COMMON_BUTTON_FLAGS,
    TDF_ALLOW_DIALOG_CANCELLATION,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{IDCANCEL, IDNO, IDYES, SW_SHOWNORMAL};

/// 读取注册表中 WebView2 Runtime 的 pv（版本）值
fn read_webview2_version_from_registry() -> Option<String> {
    let subkeys = [
        (
            HKEY_CURRENT_USER.0 as isize,
            r"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        ),
        (
            HKEY_LOCAL_MACHINE.0 as isize,
            r"Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        ),
        (
            HKEY_LOCAL_MACHINE.0 as isize,
            r"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        ),
    ];

    for (hkey_raw, path) in &subkeys {
        let mut hkey = HKEY(std::ptr::null_mut());
        let path_w = to_wstr(path);
        let result = unsafe {
            RegOpenKeyExW(
                HKEY(*hkey_raw as _),
                PCWSTR(path_w.as_ptr()),
                Some(0),
                KEY_READ,
                &mut hkey,
            )
        };
        if result.is_ok() {
            // pv 值最大 128 字节足够
            let mut buf = [0u16; 128];
            let mut size = (buf.len() * 2) as u32;
            let value_w = to_wstr("pv");
            let res = unsafe {
                RegQueryValueExW(
                    hkey,
                    PCWSTR(value_w.as_ptr()),
                    None,
                    None,
                    Some(buf.as_mut_ptr() as *mut u8),
                    Some(&mut size),
                )
            };
            let _ = unsafe { RegCloseKey(hkey) }.ok();
            if res.is_ok() {
                let len = (size as usize) / 2;
                let version = String::from_utf16_lossy(&buf[..len]);
                if !version.trim().is_empty() {
                    return Some(version.trim_end_matches('\0').to_string());
                }
            }
        }
    }
    None
}

/// 如果安装了 WebView2 Runtime，返回版本；否则返回 None
pub fn detect_webview2_runtime() -> Option<String> {
    read_webview2_version_from_registry()
}

/// 检测 WebView2，有则返回版本，否则弹 TaskDialog，让用户选择操作
pub fn ensure_webview2_or_fallback() -> Result<String, ()> {
    const WEBVIEW2_DOWNLOAD_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
    if let Some(ver) = detect_webview2_runtime() {
        return Ok(ver);
    }

    // 没有检测到 WebView2 Runtime，弹 TaskDialog 让用户三选
    let title_w = to_wstr("BMCBL");

    let main_instruction = to_wstr(&I18n::t("webview2-main", None));
    // 子提示文本
    let content = I18n::t("webview2-content", None);
    let content_w = to_wstr(&content);

    // 按钮文本
    let btn_download = to_wstr(&I18n::t("webview2-button-download", None));
    let btn_install = to_wstr(&I18n::t("webview2-button-install", None));
    let btn_exit = to_wstr(&I18n::t("webview2-button-exit", None));

    // TASKDIALOG_BUTTON 中的 nButtonID 用 MESSAGEBOX_RESULT，因此这里直接给 IDYES、IDNO、IDCANCEL
    let buttons: [TASKDIALOG_BUTTON; 3] = [
        TASKDIALOG_BUTTON {
            nButtonID: IDYES.0, // MESSAGEBOX_RESULT 是一个新类型，底层 .0 才是 i32
            pszButtonText: PCWSTR(btn_download.as_ptr()),
        },
        TASKDIALOG_BUTTON {
            nButtonID: IDNO.0,
            pszButtonText: PCWSTR(btn_install.as_ptr()),
        },
        TASKDIALOG_BUTTON {
            nButtonID: IDCANCEL.0,
            pszButtonText: PCWSTR(btn_exit.as_ptr()),
        },
    ];

    // 准备一个 TASKDIALOGCONFIG，注意所有 PCWSTR 类型的字段都要用 PCWSTR(...) 或 PCWSTR::null()
    let mut config: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
    config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
    config.hwndParent = HWND(std::ptr::null_mut());
    config.hInstance = HINSTANCE(std::ptr::null_mut());
    config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
    config.dwCommonButtons = TASKDIALOG_COMMON_BUTTON_FLAGS(0);
    config.pszWindowTitle = PCWSTR(title_w.as_ptr());
    config.pszMainInstruction = PCWSTR(main_instruction.as_ptr());
    config.pszContent = PCWSTR(content_w.as_ptr());
    config.cButtons = buttons.len() as u32;
    config.pButtons = buttons.as_ptr();
    config.nDefaultButton = IDYES.0;
    // 下面这些可选文本全部传 PCWSTR::null()
    config.cRadioButtons = 0;
    config.pRadioButtons = std::ptr::null();
    config.nDefaultRadioButton = 0;
    config.pszVerificationText = PCWSTR::null();
    config.pszExpandedInformation = PCWSTR::null();
    config.pszExpandedControlText = PCWSTR::null();
    config.pszCollapsedControlText = PCWSTR::null();
    config.pszFooter = PCWSTR::null();
    config.pfCallback = None;
    config.lpCallbackData = 0;
    config.cxWidth = 0;

    // 存放用户点击后返回的按钮 ID，必须是 i32
    let mut button_pressed: i32 = 0;
    // TaskDialogIndirect 返回的是 Result<(), windows::core::Error>
    let hr: windows::core::Result<()> =
        unsafe { TaskDialogIndirect(&mut config, Some(&mut button_pressed), None, None) };

    // 如果 hr 是 Err，那么说明弹窗失败，直接退出
    if hr.is_err() {
        unsafe { exit(0) };
    }

    // 根据用户选择（button_pressed）分三种情况
    match button_pressed {
        x if x == IDYES.0 => {
            // 使用浏览器下载
            let url_w = to_wstr(WEBVIEW2_DOWNLOAD_URL);
            unsafe {
                ShellExecuteW(
                    None,                             // Option<HWND>
                    PCWSTR(to_wstr("open").as_ptr()), // open 命令
                    PCWSTR(url_w.as_ptr()),           // URL
                    PCWSTR::null(),                   // 没有额外参数
                    PCWSTR::null(),                   // 当前目录
                    SW_SHOWNORMAL,
                );
            }
            exit(0);
        }
        x if x == IDNO.0 => {
            // 自动下载安装（以管理员权限调用 PowerShell）
            let ps_cmd = r#"
        & {
            $arch = $env:PROCESSOR_ARCHITECTURE;
            if ($arch -eq 'AMD64') {
                $url = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';
            } elseif ($arch -eq 'ARM64') {
                $url = 'https://go.microsoft.com/fwlink/p/?LinkId=2124712';
            } else {
                $url = 'https://go.microsoft.com/fwlink/p/?LinkId=2124715';
            }
            $out = Join-Path $env:TEMP 'WebView2Bootstrapper.exe';
            Invoke-WebRequest -Uri $url -OutFile $out -UseBasicParsing;
            Start-Process -FilePath $out -ArgumentList '/install' -Wait;
        }
    "#;
            // 注意：这里需要对双引号做转义
            let args = format!(
                "-NoProfile -ExecutionPolicy Bypass -WindowStyle Normal -Command \"{}\"",
                ps_cmd.trim().replace('"', r#"\""#).replace('\n', " ")
            );
            let verb = to_wstr("runas"); // 请求管理员权限
            let exe = to_wstr("powershell.exe");
            let parm = to_wstr(&args);
            unsafe {
                ShellExecuteW(
                    None,
                    PCWSTR(verb.as_ptr()),
                    PCWSTR(exe.as_ptr()),
                    PCWSTR(parm.as_ptr()),
                    PCWSTR::null(),
                    SW_SHOWNORMAL,
                );
            }
            info!("已请求管理员权限安装 WebView2，完成后请重启应用。");
            exit(0);
        }
        _ => {
            // IDCANCEL 或者对话框被关闭
            exit(0);
        }
    }
}
