use crate::utils::file_ops;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

/// 当前首次运行引导版本。
///
/// v2 将旧的静态说明弹窗升级为会切换真实页面的交互式功能导览。
/// 当后续新增必须再次展示给现有用户的重要迁移/安全步骤时继续递增。
pub const CURRENT_ONBOARDING_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
struct OnboardingVersionConfig {
    /// v1 旧字段。历史版本只在 Windows 上实现首次引导，因此继续把它视为
    /// Windows 完成状态；当 CURRENT_ONBOARDING_VERSION 提升时，旧值仍会自然触发新导览。
    completed_version: u32,
    windows_completed_version: u32,
    linux_completed_version: u32,
}

pub fn get_onboarding_config_file_path() -> PathBuf {
    file_ops::config_dir().join("onboarding.toml")
}

fn read_onboarding_config() -> io::Result<OnboardingVersionConfig> {
    let path = get_onboarding_config_file_path();
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(OnboardingVersionConfig::default());
        }
        Err(error) => return Err(error),
    };

    toml::from_str(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse onboarding version config: {error}"),
        )
    })
}

pub fn completed_onboarding_version() -> io::Result<u32> {
    let config = read_onboarding_config()?;

    #[cfg(target_os = "windows")]
    {
        return Ok(config
            .completed_version
            .max(config.windows_completed_version));
    }

    #[cfg(target_os = "linux")]
    {
        return Ok(config.linux_completed_version);
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Ok(config.completed_version)
    }
}

#[must_use]
pub fn is_current_onboarding_completed() -> bool {
    match completed_onboarding_version() {
        Ok(version) => version >= CURRENT_ONBOARDING_VERSION,
        Err(error) => {
            tracing::warn!(?error, "failed to read onboarding version config");
            false
        }
    }
}

pub fn complete_current_onboarding() -> io::Result<()> {
    let config_dir = file_ops::config_dir();
    fs::create_dir_all(&config_dir)?;
    let mut config = read_onboarding_config()?;

    #[cfg(target_os = "windows")]
    {
        // 同时保留旧字段，保证回退到尚未识别平台字段的 BMCBL 版本时不会重复弹窗。
        config.completed_version = CURRENT_ONBOARDING_VERSION;
        config.windows_completed_version = CURRENT_ONBOARDING_VERSION;
    }

    #[cfg(target_os = "linux")]
    {
        config.linux_completed_version = CURRENT_ONBOARDING_VERSION;
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        config.completed_version = CURRENT_ONBOARDING_VERSION;
    }

    let content = toml::to_string(&config)
        .map_err(|error| io::Error::other(format!("Failed to serialize onboarding config: {error}")))?;
    fs::write(get_onboarding_config_file_path(), content)
}

/// 允许调试/迁移场景显式清空所有平台的首次运行完成状态。
pub fn reset_onboarding() -> io::Result<()> {
    match fs::remove_file(get_onboarding_config_file_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{CURRENT_ONBOARDING_VERSION, OnboardingVersionConfig};

    #[test]
    fn empty_config_defaults_to_incomplete() {
        let config: OnboardingVersionConfig =
            toml::from_str("").expect("empty onboarding config should deserialize");
        assert_eq!(config.completed_version, 0);
        assert_eq!(config.windows_completed_version, 0);
        assert_eq!(config.linux_completed_version, 0);
    }

    #[test]
    fn platform_versions_round_trip() {
        let encoded = toml::to_string(&OnboardingVersionConfig {
            completed_version: CURRENT_ONBOARDING_VERSION,
            windows_completed_version: CURRENT_ONBOARDING_VERSION,
            linux_completed_version: CURRENT_ONBOARDING_VERSION,
        })
        .expect("onboarding config should serialize");
        let decoded: OnboardingVersionConfig =
            toml::from_str(&encoded).expect("onboarding config should deserialize");
        assert_eq!(decoded.completed_version, CURRENT_ONBOARDING_VERSION);
        assert_eq!(decoded.windows_completed_version, CURRENT_ONBOARDING_VERSION);
        assert_eq!(decoded.linux_completed_version, CURRENT_ONBOARDING_VERSION);
    }

    #[test]
    fn legacy_config_keeps_linux_incomplete() {
        let decoded: OnboardingVersionConfig = toml::from_str("completed_version = 1\n")
            .expect("legacy onboarding config should deserialize");
        assert_eq!(decoded.completed_version, 1);
        assert_eq!(decoded.windows_completed_version, 0);
        assert_eq!(decoded.linux_completed_version, 0);
        assert!(decoded.completed_version < CURRENT_ONBOARDING_VERSION);
    }
}
