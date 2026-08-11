#![cfg(target_os = "windows")]

#[cfg(target_os = "windows")]
#[path = "system_xbox_user_wam.rs"]
mod system_xbox_user_wam;

use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

const MAX_PROFILE_BYTES: u64 = 1024 * 1024;
const MAX_GAMERTAG_CHARS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SystemXboxUserState {
    SignedIn,
    SignedOut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SystemXboxUser {
    pub(crate) xuid: u64,
    pub(crate) gamertag: String,
    pub(crate) state: SystemXboxUserState,
    pub(crate) gamer_picture_png: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SystemXboxUserProbe {
    SignedIn(SystemXboxUser),
    SignedOut { hresult: Option<i32> },
    Unavailable { reason: String },
}

/// Returns only the locally cached Windows Xbox XUID without touching WAM,
/// gamer-picture APIs or Gaming Runtime. This is intentionally a non-secret
/// routing hint: BLoader still verifies any native handle returned by Microsoft
/// before it can be used for the same-account fast path.
pub(crate) fn probe_default_user_xuid() -> Option<u64> {
    for path in xbox_profile_candidates() {
        if let Ok(Some(user)) = read_profile_file(&path) {
            return Some(user.xuid);
        }
    }
    read_registry_profile().ok().flatten().map(|user| user.xuid)
}

pub(crate) fn probe_default_user() -> SystemXboxUserProbe {
    let mut diagnostics = Vec::new();

    for path in xbox_profile_candidates() {
        match read_profile_file(&path) {
            Ok(Some(user)) => {
                let user = attach_gamer_picture(user);
                tracing::info!(
                    source = %path.display(),
                    xbox_gamertag = %user.gamertag,
                    xuid = user.xuid,
                    "已从 Xbox 应用本地状态读取系统真实用户"
                );
                return SystemXboxUserProbe::SignedIn(user);
            }
            Ok(None) => {}
            Err(error) => diagnostics.push(format!("{}: {error}", path.display())),
        }
    }

    match read_registry_profile() {
        Ok(Some(user)) => {
            let user = attach_gamer_picture(user);
            tracing::info!(
                source = "HKCU Xbox identity registry",
                xbox_gamertag = %user.gamertag,
                xuid = user.xuid,
                "已从 Windows Xbox 注册表读取系统真实用户"
            );
            return SystemXboxUserProbe::SignedIn(user);
        }
        Ok(None) => {}
        Err(error) => diagnostics.push(format!("registry: {error}")),
    }

    if diagnostics.is_empty() {
        SystemXboxUserProbe::SignedOut { hresult: None }
    } else {
        SystemXboxUserProbe::Unavailable {
            reason: format!(
                "未能从 Xbox App/Gaming App 本地状态读取用户；{}",
                diagnostics.join(" | ")
            ),
        }
    }
}

#[cfg(target_os = "windows")]
fn attach_gamer_picture(mut user: SystemXboxUser) -> SystemXboxUser {
    match system_xbox_user_wam::gamer_picture_for_xuid(user.xuid) {
        Ok(Some(picture)) => {
            tracing::info!(
                xuid = user.xuid,
                bytes = picture.len(),
                "已通过 Windows WAM/Xbox Profile API 获取并校验本地 Xbox 头像"
            );
            user.gamer_picture_png = Some(picture);
        }
        Ok(None) => tracing::warn!(
            xuid = user.xuid,
            "Windows WAM/Xbox 未返回匹配的本地 Xbox 头像"
        ),
        Err(error) => tracing::warn!(xuid = user.xuid, %error, "本地 Xbox 头像获取失败"),
    }
    user
}

#[cfg(not(target_os = "windows"))]
fn attach_gamer_picture(user: SystemXboxUser) -> SystemXboxUser {
    user
}

fn xbox_profile_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let Some(local_app_data) = env::var_os("LOCALAPPDATA").map(PathBuf::from) else {
        return candidates;
    };

    for package in [
        "Microsoft.XboxApp_8wekyb3d8bbwe",
        "Microsoft.GamingApp_8wekyb3d8bbwe",
    ] {
        let local_state = local_app_data
            .join("Packages")
            .join(package)
            .join("LocalState");
        candidates.push(local_state.join("XboxLiveGamer.xml"));
        candidates.push(local_state.join("ModelManager").join("XboxLiveGamer.xml"));
        append_case_insensitive_matches(&local_state, 3, &mut candidates);
    }

    candidates.push(
        local_app_data
            .join("LocalState")
            .join("ModelManager")
            .join("XboxLiveGamer.xml"),
    );

    candidates.sort();
    candidates.dedup();
    candidates
}

fn append_case_insensitive_matches(root: &Path, depth: usize, output: &mut Vec<PathBuf>) {
    if depth == 0 || !root.is_dir() {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            append_case_insensitive_matches(&path, depth - 1, output);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("XboxLiveGamer.xml"))
        {
            output.push(path);
        }
    }
}

fn read_profile_file(path: &Path) -> Result<Option<SystemXboxUser>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取元数据失败：{error}")),
    };
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_PROFILE_BYTES {
        return Ok(None);
    }

    let bytes = fs::read(path).map_err(|error| format!("读取文件失败：{error}"))?;
    let text = decode_profile_text(&bytes)?;
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| format!("解析 JSON 失败：{error}"))?;
    Ok(extract_user_from_json(&value))
}

fn decode_profile_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units).map_err(|error| format!("解析 UTF-16LE 失败：{error}"));
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units).map_err(|error| format!("解析 UTF-16BE 失败：{error}"));
    }
    String::from_utf8(bytes.to_vec()).map_err(|error| format!("解析 UTF-8 失败：{error}"))
}

fn extract_user_from_json(value: &Value) -> Option<SystemXboxUser> {
    let xuid = find_json_field(value, &["XboxUserId", "XboxUserID", "Xuid", "XUID", "xuid"])
        .and_then(parse_xuid_value)?;
    let gamertag = find_json_field(
        value,
        &[
            "Gamertag",
            "GamerTag",
            "gamertag",
            "UniqueModernGamertag",
            "ModernGamertag",
            "DisplayName",
        ],
    )
    .and_then(Value::as_str)
    .and_then(sanitize_gamertag)?;

    Some(SystemXboxUser {
        xuid,
        gamertag,
        state: SystemXboxUserState::SignedIn,
        gamer_picture_png: None,
    })
}

fn find_json_field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if names.iter().any(|name| key.eq_ignore_ascii_case(name)) {
                    return Some(value);
                }
            }
            map.values().find_map(|value| find_json_field(value, names))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_json_field(value, names)),
        _ => None,
    }
}

fn parse_xuid_value(value: &Value) -> Option<u64> {
    match value {
        Value::String(value) => value.trim().parse::<u64>().ok().filter(|value| *value != 0),
        Value::Number(value) => value.as_u64().filter(|value| *value != 0),
        _ => None,
    }
}

fn sanitize_gamertag(value: &str) -> Option<String> {
    let value = value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_GAMERTAG_CHARS)
        .collect::<String>();
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn read_registry_profile() -> Result<Option<SystemXboxUser>, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let paths = [
        r"Software\Microsoft\XboxLive",
        r"Software\Microsoft\XboxLive\Identity",
        r"Software\Microsoft\XboxLive\Authentication",
        r"Software\Microsoft\Xbox\Identity",
        r"Software\Microsoft\XboxLive\User",
    ];

    for path in paths {
        let Ok(key) = hkcu.open_subkey(path) else {
            continue;
        };
        let xuid = read_registry_string(&key, &["UserXUID", "XboxUserId", "Xuid", "XUID"])
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value != 0);
        let gamertag = read_registry_string(
            &key,
            &[
                "Gamertag",
                "GamerTag",
                "DisplayName",
                "UniqueModernGamertag",
            ],
        )
        .and_then(|value| sanitize_gamertag(&value));

        if let (Some(xuid), Some(gamertag)) = (xuid, gamertag) {
            return Ok(Some(SystemXboxUser {
                xuid,
                gamertag,
                state: SystemXboxUserState::SignedIn,
                gamer_picture_png: None,
            }));
        }
    }
    Ok(None)
}

fn read_registry_string(key: &RegKey, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        key.get_value::<String, _>(*name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_xbox_live_gamer_json() {
        let value = json!({
            "XboxUserId": "2535433707460133",
            "Gamertag": "CivilRelic4341"
        });
        let user = extract_user_from_json(&value).expect("profile");
        assert_eq!(user.xuid, 2_535_433_707_460_133);
        assert_eq!(user.gamertag, "CivilRelic4341");
    }

    #[test]
    fn parses_nested_and_numeric_xuid() {
        let value = json!({
            "profile": {
                "XUID": 2535433707460133_u64,
                "UniqueModernGamertag": "Player"
            }
        });
        assert!(extract_user_from_json(&value).is_some());
    }

    #[test]
    fn rejects_incomplete_profile() {
        assert!(extract_user_from_json(&json!({"Gamertag": "Player"})).is_none());
        assert!(extract_user_from_json(&json!({"XboxUserId": "123"})).is_none());
    }
}
