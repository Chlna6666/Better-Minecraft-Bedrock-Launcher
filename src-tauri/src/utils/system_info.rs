use windows::Win32::Globalization::{GetACP, GetUserDefaultUILanguage, GetLocaleInfoW, LOCALE_SENGLISHLANGUAGENAME, LOCALE_SNAME};
use windows::Win32::System::SystemInformation::{
    GetSystemInfo, PROCESSOR_ARCHITECTURE_AMD64,
    PROCESSOR_ARCHITECTURE_ARM, PROCESSOR_ARCHITECTURE_INTEL, SYSTEM_INFO,
};

/// 返回当前系统活动代码页（ANSI Code Page）
pub fn detect_system_encoding() -> u32 {
    unsafe { GetACP() }
}

/// 返回系统 UI 语言的标准代码（如 "zh-CN"、"en-US"）
#[tauri::command]
pub fn get_system_language() -> String {
    unsafe {
        let lang_id = GetUserDefaultUILanguage();
        let mut buf = [0u16; 16];
        let len = GetLocaleInfoW(lang_id as u32, LOCALE_SNAME, Some(&mut buf));
        if len > 0 {
            String::from_utf16_lossy(&buf[..(len as usize - 1)])
        } else {
            "en-US".to_string()
        }
    }
}

/// 返回 CPU 架构字符串，如 "x86"/"x64"/"ARM"
pub fn get_cpu_architecture() -> String {
    unsafe {
        let mut info: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut info);
        match info.Anonymous.Anonymous.wProcessorArchitecture {
            PROCESSOR_ARCHITECTURE_INTEL => "x86".into(),
            PROCESSOR_ARCHITECTURE_AMD64 => "x64".into(),
            PROCESSOR_ARCHITECTURE_ARM   => "ARM".into(),
            _                            => "Unknown".into(),
        }
    }
}
