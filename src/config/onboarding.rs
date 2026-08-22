use crate::utils::file_ops;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

/// 当前首次运行引导版本。
///
/// 当新增必须再次展示给现有用户的重要迁移/安全步骤时递增此值。
pub const CURRENT_ONBOARDING_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
struct OnboardingVersionConfig {
    completed_version: u32,
}

pub fn get_onboarding_config_file_path() -> PathBuf {
    file_ops::config_dir().join("onboarding.toml")
}

pub fn completed_onboarding_version() -> io::Result<u32> {
    let path = get_onboarding_config_file_path();
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };

    let config: OnboardingVersionConfig = toml::from_str(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse onboarding version config: {error}"),
        )
    })?;
    Ok(config.completed_version)
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

    let content = toml::to_string(&OnboardingVersionConfig {
        completed_version: CURRENT_ONBOARDING_VERSION,
    })
    .map_err(|error| io::Error::other(format!("Failed to serialize onboarding config: {error}")))?;

    fs::write(get_onboarding_config_file_path(), content)
}

/// 允许设置页或调试入口重新打开首次运行引导。
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
    }

    #[test]
    fn current_version_round_trips() {
        let encoded = toml::to_string(&OnboardingVersionConfig {
            completed_version: CURRENT_ONBOARDING_VERSION,
        })
        .expect("onboarding config should serialize");
        let decoded: OnboardingVersionConfig =
            toml::from_str(&encoded).expect("onboarding config should deserialize");
        assert_eq!(decoded.completed_version, CURRENT_ONBOARDING_VERSION);
    }
}
