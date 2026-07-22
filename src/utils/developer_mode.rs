#![cfg(target_os = "windows")]
use std::io;
use std::mem::size_of;

use thiserror::Error;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
use windows::core::{HSTRING, PCWSTR};
#[cfg(target_os = "windows")]
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};

const DEVELOPER_MODE_REG_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock";
const DEVELOPER_MODE_VALUE_NAME: &str = "AllowDevelopmentWithoutDevLicense";

#[derive(Debug, Error)]
pub enum DeveloperModeError {
    #[error("developer mode requires administrator privileges")]
    AccessDenied,
    #[error("developer mode registry error: {0}")]
    Registry(String),
    #[error("failed to open developer settings: {0}")]
    OpenSettings(String),
}

pub fn is_process_elevated() -> bool {
    let elevated = unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned_length = 0u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_length,
        );
        let _ = CloseHandle(token);

        result.is_ok() && elevation.TokenIsElevated != 0
    };
    debug!(elevated, "已检查当前进程管理员权限状态");
    elevated
}

pub fn is_developer_mode_enabled() -> bool {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let enabled = hklm
        .open_subkey_with_flags(DEVELOPER_MODE_REG_PATH, KEY_READ | KEY_WOW64_64KEY)
        .ok()
        .and_then(|key| key.get_value::<u32, _>(DEVELOPER_MODE_VALUE_NAME).ok())
        .is_some_and(|value| value == 1);
    debug!(enabled, "已检查开发者模式开关状态");
    enabled
}

pub fn try_enable_developer_mode() -> Result<(), DeveloperModeError> {
    info!("开始通过注册表启用开发者模式");
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let (key, _) = hklm
        .create_subkey(DEVELOPER_MODE_REG_PATH)
        .map_err(map_registry_error)?;

    key.set_value(DEVELOPER_MODE_VALUE_NAME, &1u32)
        .map_err(map_registry_error)?;
    info!("开发者模式注册表写入成功");
    Ok(())
}

pub fn open_developer_settings() -> Result<(), DeveloperModeError> {
    info!("准备打开系统开发者模式设置页");
    unsafe {
        let operation = HSTRING::from("open");
        let uri = HSTRING::from("ms-settings:developers");
        let result = ShellExecuteW(
            Some(HWND::default()),
            PCWSTR(operation.as_ptr()),
            PCWSTR(uri.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOW,
        );

        if (result.0 as isize) > 32 {
            info!("系统开发者模式设置页已打开");
            Ok(())
        } else {
            warn!("打开系统开发者模式设置页失败: {:?}", result);
            Err(DeveloperModeError::OpenSettings(format!(
                "ShellExecuteW returned {:?}",
                result
            )))
        }
    }
}

fn map_registry_error(error: io::Error) -> DeveloperModeError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => DeveloperModeError::AccessDenied,
        _ => DeveloperModeError::Registry(error.to_string()),
    }
}
