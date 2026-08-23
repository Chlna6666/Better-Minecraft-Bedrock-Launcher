use crate::utils::file_ops;
use serde::Deserialize;
use std::{fs, io, path::PathBuf};

/// 当前内置用户协议版本。
///
/// 协议内容发生需要用户重新确认的变更时，只需递增此版本。
pub const CURRENT_AGREEMENT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default)]
struct LegacyAgreementVersionConfig {
    accepted_version: u32,
}

fn legacy_agreement_config_file_path() -> PathBuf {
    file_ops::config_dir().join("agreement.toml")
}

fn read_legacy_agreement_version() -> io::Result<u32> {
    let content = match fs::read_to_string(legacy_agreement_config_file_path()) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let config: LegacyAgreementVersionConfig = toml::from_str(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse legacy agreement config: {error}"),
        )
    })?;
    Ok(config.accepted_version)
}

fn remove_legacy_agreement_config() -> io::Result<()> {
    match fs::remove_file(legacy_agreement_config_file_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// 从统一 settings.toml 读取协议版本；旧 agreement.toml 只作为一次性迁移来源。
pub fn accepted_agreement_version() -> io::Result<u32> {
    let config = super::config::read_config()?;
    let current = config.app_state.agreement_accepted_version;
    let legacy_path = legacy_agreement_config_file_path();
    let legacy_file_present = legacy_path.is_file();
    let legacy_file_version = read_legacy_agreement_version()?;
    // 最早的 BMCBL 只有 agreement_accepted 布尔值，将它视为 v1，不能因此自动接受 v2。
    let legacy_bool_version = u32::from(config.agreement_accepted);
    let migrated = current.max(legacy_file_version).max(legacy_bool_version);

    if migrated != current || legacy_file_present {
        super::config::update_config(|config| {
            config.app_state.agreement_accepted_version = migrated;
            config.agreement_accepted = migrated > 0;
        })?;
        // 旧文件只有在 settings.toml 已同步落盘后才删除，避免崩溃窗口丢失接受状态。
        super::config::flush_config_now();
        remove_legacy_agreement_config()?;
    }

    Ok(migrated)
}

#[must_use]
pub fn is_current_agreement_accepted() -> bool {
    match accepted_agreement_version() {
        Ok(version) => version >= CURRENT_AGREEMENT_VERSION,
        Err(error) => {
            tracing::warn!(?error, "failed to read agreement version from main config");
            false
        }
    }
}

pub fn accept_current_agreement() -> io::Result<()> {
    super::config::update_config(|config| {
        config.app_state.agreement_accepted_version = CURRENT_AGREEMENT_VERSION;
        // 保留旧字段，兼容回退到尚未识别 app_state 的 BMCBL 版本。
        config.agreement_accepted = true;
    })?;
    super::config::flush_config_now();
    remove_legacy_agreement_config()
}

#[cfg(test)]
mod tests {
    use super::{CURRENT_AGREEMENT_VERSION, LegacyAgreementVersionConfig};

    #[test]
    fn legacy_empty_config_defaults_to_unaccepted() {
        let config: LegacyAgreementVersionConfig =
            toml::from_str("").expect("empty legacy agreement config should deserialize");
        assert_eq!(config.accepted_version, 0);
    }

    #[test]
    fn current_version_is_newer_than_legacy_boolean_version() {
        assert!(CURRENT_AGREEMENT_VERSION > 1);
    }
}
