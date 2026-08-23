#![cfg(target_os = "windows")]

use crate::utils::file_ops;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

/// Windows UWP 数据保护说明版本。
///
/// 只有当说明内容或实际迁移安全语义发生需要重新提示用户的重要变化时才递增。
pub const CURRENT_UWP_SAFETY_GUIDE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
struct UwpSafetyGuideConfig {
    acknowledged_version: u32,
}

fn config_path() -> PathBuf {
    file_ops::config_dir().join("uwp_safety_guide.toml")
}

fn read_config() -> io::Result<UwpSafetyGuideConfig> {
    let path = config_path();
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(UwpSafetyGuideConfig::default());
        }
        Err(error) => return Err(error),
    };

    toml::from_str(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse UWP safety guide config: {error}"),
        )
    })
}

#[must_use]
pub fn is_current_guide_acknowledged() -> bool {
    match read_config() {
        Ok(config) => config.acknowledged_version >= CURRENT_UWP_SAFETY_GUIDE_VERSION,
        Err(error) => {
            tracing::warn!(?error, "failed to read UWP safety guide config");
            false
        }
    }
}

pub fn acknowledge_current_guide() -> io::Result<()> {
    let config_dir = file_ops::config_dir();
    fs::create_dir_all(&config_dir)?;
    let content = toml::to_string(&UwpSafetyGuideConfig {
        acknowledged_version: CURRENT_UWP_SAFETY_GUIDE_VERSION,
    })
    .map_err(|error| {
        io::Error::other(format!("Failed to serialize UWP safety guide config: {error}"))
    })?;
    fs::write(config_path(), content)
}

#[cfg(test)]
mod tests {
    use super::{CURRENT_UWP_SAFETY_GUIDE_VERSION, UwpSafetyGuideConfig};

    #[test]
    fn empty_config_defaults_to_unacknowledged() {
        let config: UwpSafetyGuideConfig =
            toml::from_str("").expect("empty UWP safety config should deserialize");
        assert_eq!(config.acknowledged_version, 0);
    }

    #[test]
    fn current_version_round_trips() {
        let encoded = toml::to_string(&UwpSafetyGuideConfig {
            acknowledged_version: CURRENT_UWP_SAFETY_GUIDE_VERSION,
        })
        .expect("UWP safety config should serialize");
        let decoded: UwpSafetyGuideConfig =
            toml::from_str(&encoded).expect("UWP safety config should deserialize");
        assert_eq!(
            decoded.acknowledged_version,
            CURRENT_UWP_SAFETY_GUIDE_VERSION
        );
    }
}
