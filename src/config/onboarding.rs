use crate::utils::file_ops;
use serde::Deserialize;
use std::{fs, io, path::PathBuf};

use super::config::AppStateConfig;

/// 当前首次运行引导版本。
///
/// v2 将旧的静态说明弹窗升级为会切换真实页面的交互式功能导览。
/// v3 从新用户实际操作路径重新组织导览，并补齐任务、管理内容、设置与工具页面。
/// v4 将引导改为自适应聚焦式布局：说明卡根据真实高光区域自动避让，窄窗口
/// 也保留 spotlight；演示数据只在真实任务/版本为空时出现。
/// 当后续新增必须再次展示给现有用户的重要功能教学时继续递增。
pub const CURRENT_ONBOARDING_VERSION: u32 = 4;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
struct LegacyOnboardingVersionConfig {
    /// v1 旧字段。历史版本只在 Windows 上实现首次引导，因此继续把它视为 Windows 完成状态。
    completed_version: u32,
    windows_completed_version: u32,
    linux_completed_version: u32,
}

fn legacy_onboarding_config_file_path() -> PathBuf {
    file_ops::config_dir().join("onboarding.toml")
}

fn read_legacy_onboarding_config() -> io::Result<LegacyOnboardingVersionConfig> {
    let content = match fs::read_to_string(legacy_onboarding_config_file_path()) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LegacyOnboardingVersionConfig::default());
        }
        Err(error) => return Err(error),
    };

    toml::from_str(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse legacy onboarding config: {error}"),
        )
    })
}

fn remove_legacy_onboarding_config() -> io::Result<()> {
    match fs::remove_file(legacy_onboarding_config_file_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn app_state_with_legacy_migration() -> io::Result<AppStateConfig> {
    let config = super::config::read_config()?;
    let mut state = config.app_state.clone();
    let legacy = read_legacy_onboarding_config()?;

    let before = state.clone();
    state.onboarding_completed_version = state
        .onboarding_completed_version
        .max(legacy.completed_version);
    state.onboarding_windows_completed_version = state
        .onboarding_windows_completed_version
        .max(legacy.completed_version)
        .max(legacy.windows_completed_version);
    state.onboarding_linux_completed_version = state
        .onboarding_linux_completed_version
        .max(legacy.linux_completed_version);

    let legacy_present = legacy.completed_version > 0
        || legacy.windows_completed_version > 0
        || legacy.linux_completed_version > 0;
    if state != before || legacy_present {
        let migrated = state.clone();
        super::config::update_config(|config| {
            config.app_state = migrated;
        })?;
        // 保证统一配置已经同步写盘，再移除旧 onboarding.toml。
        super::config::flush_config_now();
        remove_legacy_onboarding_config()?;
    }

    Ok(state)
}

pub fn completed_onboarding_version() -> io::Result<u32> {
    let state = app_state_with_legacy_migration()?;

    #[cfg(target_os = "windows")]
    {
        return Ok(state
            .onboarding_completed_version
            .max(state.onboarding_windows_completed_version));
    }

    #[cfg(target_os = "linux")]
    {
        return Ok(state.onboarding_linux_completed_version);
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Ok(state.onboarding_completed_version)
    }
}

#[must_use]
pub fn is_current_onboarding_completed() -> bool {
    match completed_onboarding_version() {
        Ok(version) => version >= CURRENT_ONBOARDING_VERSION,
        Err(error) => {
            tracing::warn!(?error, "failed to read onboarding version from main config");
            false
        }
    }
}

pub fn complete_current_onboarding() -> io::Result<()> {
    super::config::update_config(|config| {
        #[cfg(target_os = "windows")]
        {
            // 同时保留通用字段，保证回退到只认识单一完成版本的后续兼容实现时不重复弹窗。
            config.app_state.onboarding_completed_version = CURRENT_ONBOARDING_VERSION;
            config.app_state.onboarding_windows_completed_version = CURRENT_ONBOARDING_VERSION;
        }

        #[cfg(target_os = "linux")]
        {
            config.app_state.onboarding_linux_completed_version = CURRENT_ONBOARDING_VERSION;
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            config.app_state.onboarding_completed_version = CURRENT_ONBOARDING_VERSION;
        }
    })?;
    super::config::flush_config_now();
    remove_legacy_onboarding_config()
}

/// 允许调试/迁移场景显式清空所有平台的首次运行完成状态。
pub fn reset_onboarding() -> io::Result<()> {
    super::config::update_config(|config| {
        config.app_state.onboarding_completed_version = 0;
        config.app_state.onboarding_windows_completed_version = 0;
        config.app_state.onboarding_linux_completed_version = 0;
    })?;
    super::config::flush_config_now();
    remove_legacy_onboarding_config()
}

#[cfg(test)]
mod tests {
    use super::{CURRENT_ONBOARDING_VERSION, LegacyOnboardingVersionConfig};

    #[test]
    fn legacy_empty_config_defaults_to_incomplete() {
        let config: LegacyOnboardingVersionConfig =
            toml::from_str("").expect("empty legacy onboarding config should deserialize");
        assert_eq!(config.completed_version, 0);
        assert_eq!(config.windows_completed_version, 0);
        assert_eq!(config.linux_completed_version, 0);
    }

    #[test]
    fn legacy_v1_is_older_than_current_tour() {
        let decoded: LegacyOnboardingVersionConfig = toml::from_str("completed_version = 1\n")
            .expect("legacy onboarding config should deserialize");
        assert_eq!(decoded.completed_version, 1);
        assert!(decoded.completed_version < CURRENT_ONBOARDING_VERSION);
    }
}
