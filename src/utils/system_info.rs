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
    resolve_unix_language(
        ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok()),
    )
    .or_else(read_unix_locale_config)
    .unwrap_or_else(|| "en-US".to_string())
}

#[cfg(not(target_os = "windows"))]
fn read_unix_locale_config() -> Option<String> {
    let user_config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|home| home.join(".config"))
        })
        .map(|config| config.join("locale.conf"));

    user_config
        .into_iter()
        .chain([
            std::path::PathBuf::from("/etc/locale.conf"),
            std::path::PathBuf::from("/etc/default/locale"),
        ])
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .find_map(|contents| parse_unix_locale_config(&contents))
}

#[cfg(not(target_os = "windows"))]
fn parse_unix_locale_config(contents: &str) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"]
        .into_iter()
        .filter_map(|name| {
            contents.lines().find_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                let (key, value) = line.split_once('=')?;
                (key.trim() == name).then(|| value.trim().trim_matches(['\'', '"']))
            })
        })
        .find_map(normalize_unix_locale)
}

#[cfg(not(target_os = "windows"))]
fn resolve_unix_language<I, S>(locales: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    locales
        .into_iter()
        .find_map(|locale| normalize_unix_locale(locale.as_ref()))
}

#[cfg(not(target_os = "windows"))]
fn normalize_unix_locale(locale: &str) -> Option<String> {
    locale.split(':').find_map(|candidate| {
        let candidate = candidate.split(['.', '@']).next()?.trim();
        if candidate.is_empty()
            || candidate.eq_ignore_ascii_case("C")
            || candidate.eq_ignore_ascii_case("POSIX")
        {
            return None;
        }
        Some(candidate.replace('_', "-"))
    })
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

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::{normalize_unix_locale, parse_unix_locale_config, resolve_unix_language};

    #[test]
    fn neutral_overrides_fall_through_to_lang() {
        assert_eq!(
            resolve_unix_language(["C.UTF-8", "C.UTF-8", "zh_CN.UTF-8"]),
            Some("zh-CN".to_string())
        );
    }

    #[test]
    fn neutral_locale_does_not_resolve_to_a_language() {
        assert_eq!(normalize_unix_locale("C.UTF-8"), None);
        assert_eq!(normalize_unix_locale("POSIX"), None);
    }

    #[test]
    fn linux_locale_is_normalized_to_i18n_code() {
        assert_eq!(
            normalize_unix_locale("zh_CN.UTF-8"),
            Some("zh-CN".to_string())
        );
    }

    #[test]
    fn language_priority_list_uses_first_concrete_locale() {
        assert_eq!(
            normalize_unix_locale("C:zh_CN.UTF-8:en_US"),
            Some("zh-CN".to_string())
        );
    }

    #[test]
    fn locale_modifier_is_removed() {
        assert_eq!(
            normalize_unix_locale("zh_TW@traditional"),
            Some("zh-TW".to_string())
        );
    }

    #[test]
    fn fedora_locale_config_resolves_system_language() {
        assert_eq!(
            parse_unix_locale_config("LANG=\"zh_CN.UTF-8\"\n"),
            Some("zh-CN".to_string())
        );
    }

    #[test]
    fn neutral_config_entry_falls_through_to_lang() {
        assert_eq!(
            parse_unix_locale_config("LC_MESSAGES=C.UTF-8\nLANG=zh_CN.UTF-8\n"),
            Some("zh-CN".to_string())
        );
    }
}
