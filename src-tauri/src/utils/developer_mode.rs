use std::ffi::OsStr;
use std::iter;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{ null_mut};
use std::process::Command;
use tracing::debug;

use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegOpenKeyExW, RegQueryValueExW, RegCloseKey, HKEY_LOCAL_MACHINE, KEY_READ,
};
use windows::Win32::UI::Controls::{
    TaskDialogIndirect, TASKDIALOGCONFIG, TASKDIALOG_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
};
use windows::Win32::UI::WindowsAndMessaging::{IDI_WARNING};
use crate::i18n::I18n;
use crate::utils::utils::to_wstr;

/// 检查“开发者模式”是否启用：
/// 从注册表读取：
///   HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock
///   值名 AllowDevelopmentWithoutDevLicense（DWORD）
/// 如果该键不存在或读取出错，或者值不为 1，就返回 false。
fn is_developer_mode_enabled() -> bool {
    // 将注册表路径和键名都转换成宽字符串
    let sub_key = to_wstr(r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock");
    let value_name = to_wstr("AllowDevelopmentWithoutDevLicense");

    unsafe {
        // 第一步：打开注册表键
        let mut hkey = Default::default();
        let result = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(sub_key.as_ptr()),
            Some(0),
            KEY_READ,
            &mut hkey,
        );
        if result.is_err() {
            // 打不开就认为没有启用
            return false;
        }

        // 第二步：读取 DWORD 值
        let mut data: u32 = 0;
        let mut data_len: u32 = std::mem::size_of::<u32>() as u32;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            None,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut data_len),
        );
        // 关闭注册表句柄
        let _ = RegCloseKey(hkey);

        if result.is_err() {
            // 读取失败也认为未启用
            return false;
        }

        // 只有当 data == 1 时才认为已启用
        data == 1
    }
}

/// 如果“开发者模式”未启用，则弹出 TaskDialog：
/// - “打开开发者设置” → 返回 true
/// - “取消” 或 关闭窗口 → 返回 false
fn show_developer_mode_dialog() -> bool {
    unsafe {
        // 定义两个按钮 ID，1001 表示“打开开发者设置”，2 表示“取消”（等同 IDCANCEL）
        const ID_BTN_OPEN_SETTINGS: i32 = 1001;
        const ID_BTN_CANCEL: i32 = 2;
        
        let title_w        = to_wstr(&I18n::t("developer-mode-title", None));
        let main_inst_w    = to_wstr(&I18n::t("developer-mode-main", None));
        let content_w      = to_wstr(&I18n::t("developer-mode-content", None));
        let open_txt_w     = to_wstr(&I18n::t("developer-mode-open", None));
        let cancel_txt_w   = to_wstr(&I18n::t("developer-mode-cancel", None));
  
        let buttons: [TASKDIALOG_BUTTON; 2] = [
            TASKDIALOG_BUTTON {
                nButtonID: ID_BTN_OPEN_SETTINGS,
                pszButtonText: PCWSTR(open_txt_w.as_ptr()),
            },
            TASKDIALOG_BUTTON {
                nButtonID: ID_BTN_CANCEL,
                pszButtonText: PCWSTR(cancel_txt_w.as_ptr()),
            },
        ];

      
        let mut config: TASKDIALOGCONFIG = std::mem::zeroed();
        config.cbSize = std::mem::size_of::<TASKDIALOGCONFIG>() as u32;
        config.hwndParent = windows::Win32::Foundation::HWND(null_mut()); // 无父窗口
        config.hInstance =  windows::Win32::Foundation::HINSTANCE(null_mut());  // 默认

        // 以下字段都要传宽字符串
        config.pszWindowTitle     = PCWSTR(title_w.as_ptr());
        config.pszMainInstruction = PCWSTR(main_inst_w.as_ptr());
        config.pszContent         = PCWSTR(content_w.as_ptr());

        // 不使用系统自带的 OK/Yes/No 按钮
        config.dwCommonButtons = windows::Win32::UI::Controls::TASKDIALOG_COMMON_BUTTON_FLAGS(0);
        // 默认焦点放在“打开开发者设置”
        config.nDefaultButton = ID_BTN_OPEN_SETTINGS;

        // 使用自定义按钮
        config.cButtons = buttons.len() as u16 as u32;
        config.pButtons = buttons.as_ptr();

        // 允许按 Esc 或 点击“X”关闭窗口（等价于取消）
        config.dwFlags = TDF_ALLOW_DIALOG_CANCELLATION;
        
        // 用于接收用户实际点击的按钮 ID
        let mut clicked: i32 = 0;
        let hr = TaskDialogIndirect(
            &config,
            Some(&mut clicked),
            None,
            None,
        );
        if hr.is_err() {
            // 如果调用失败，就当作用户取消
            return false;
        }

        // 只有当用户点了“打开开发者设置”才返回 true
        clicked == ID_BTN_OPEN_SETTINGS
    }
}

/// 入口函数：先检查是否启用了开发者模式，
/// 如果没启用就弹出对话框并根据用户选择打开设置页
pub fn ensure_developer_mode_enabled() {
    if is_developer_mode_enabled() {
        // 已经启用，直接返回
        debug!("开发者模式已启用");
        return;
    }

    debug!("检测到开发者模式未启用，弹出提示对话框...");
    let need_open = show_developer_mode_dialog();
    if need_open {
        // 用户选择“打开开发者设置”，用 cmd 启动 ms-settings:developers
        let _ = Command::new("cmd")
            .args(&["/C", "start ms-settings:developers"])
            .spawn();
    } else {
        debug!("用户取消了启用开发者模式的操作。");
    }
}
