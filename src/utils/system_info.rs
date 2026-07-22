#[cfg(target_os = "windows")]
use windows::Win32::Globalization::{
    GetACP, GetLocaleInfoW, GetUserDefaultUILanguage, LOCALE_SNAME,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::SystemInformation::{
    GetSystemInfo, PROCESSOR_ARCHITECTURE_AMD64, PROCESSOR_ARCHITECTURE_ARM,
    PROCESSOR_ARCHITECTURE_INTEL, SYSTEM_INFO,
};

/// 返回当前系统活动代码页（ANSI Code Page）
#[cfg(target_os = "windows")]
pub fn detect_system_encoding() -> u32 {
    // SAFETY: `GetACP` only reads the process-wide active Windows code page.
    unsafe { GetACP() }
}

#[cfg(not(target_os = "windows"))]
pub fn detect_system_encoding() -> u32 {
    65001
}

/// 返回系统 UI 语言的标准代码（如 "zh-CN"、"en-US"）
#[cfg(target_os = "windows")]
pub fn get_system_language() -> String {
    // SAFETY: The fixed buffer is valid for the duration of the Win32 locale query.
    unsafe {
        let lang_id = GetUserDefaultUILanguage();
        let mut buf = [0u16; 16];
        let len = GetLocaleInfoW(lang_id as u32, LOCALE_SNAME, Some(&mut buf));
        if len > 0 {
            String::from_utf16_lossy(&buf[..(len as usize - 1)]).replace('_', "-")
        } else {
            "en-US".to_string()
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn get_system_language() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .and_then(|locale| normalize_unix_locale(&locale))
        .unwrap_or_else(|| "en-US".to_string())
}

#[cfg(not(target_os = "windows"))]
fn normalize_unix_locale(locale: &str) -> Option<String> {
    let locale = locale.split(['.', '@']).next()?.trim();
    if locale.is_empty() || matches!(locale, "C" | "POSIX") {
        return None;
    }
    Some(locale.replace('_', "-"))
}

/// 返回 CPU 架构字符串，如 "x86"/"x64"/"ARM"
#[cfg(target_os = "windows")]
pub fn get_cpu_architecture() -> String {
    // SAFETY: `GetSystemInfo` initializes the provided `SYSTEM_INFO` buffer.
    unsafe {
        let mut info: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut info);
        match info.Anonymous.Anonymous.wProcessorArchitecture {
            PROCESSOR_ARCHITECTURE_INTEL => "x86".into(),
            PROCESSOR_ARCHITECTURE_AMD64 => "x64".into(),
            PROCESSOR_ARCHITECTURE_ARM => "ARM".into(),
            _ => "Unknown".into(),
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn get_cpu_architecture() -> String {
    std::env::consts::ARCH.to_string()
}
