use crate::utils::file_ops;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

/// 当前内置用户协议版本。
///
/// 协议内容发生需要用户重新确认的变更时，只需递增此版本。
pub const CURRENT_AGREEMENT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
struct AgreementVersionConfig {
    accepted_version: u32,
}

pub fn get_agreement_config_file_path() -> PathBuf {
    file_ops::config_dir().join("agreement.toml")
}

pub fn accepted_agreement_version() -> io::Result<u32> {
    let path = get_agreement_config_file_path();
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };

    let config: AgreementVersionConfig = toml::from_str(&content).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse agreement version config: {error}"),
        )
    })?;
    Ok(config.accepted_version)
}

#[must_use]
pub fn is_current_agreement_accepted() -> bool {
    match accepted_agreement_version() {
        Ok(version) => version >= CURRENT_AGREEMENT_VERSION,
        Err(error) => {
            tracing::warn!(?error, "failed to read agreement version config");
            false
        }
    }
}

pub fn accept_current_agreement() -> io::Result<()> {
    let config_dir = file_ops::config_dir();
    fs::create_dir_all(&config_dir)?;

    let content = toml::to_string(&AgreementVersionConfig {
        accepted_version: CURRENT_AGREEMENT_VERSION,
    })
    .map_err(|error| {
        io::Error::other(format!(
            "Failed to serialize agreement version config: {error}"
        ))
    })?;

    fs::write(get_agreement_config_file_path(), content)
}

#[cfg(test)]
mod tests {
    use super::{AgreementVersionConfig, CURRENT_AGREEMENT_VERSION};

    #[test]
    fn missing_accepted_version_defaults_to_unaccepted() {
        let config: AgreementVersionConfig =
            toml::from_str("").expect("empty agreement config should deserialize");
        assert_eq!(config.accepted_version, 0);
    }

    #[test]
    fn current_version_round_trips() {
        let encoded = toml::to_string(&AgreementVersionConfig {
            accepted_version: CURRENT_AGREEMENT_VERSION,
        })
        .expect("agreement config should serialize");
        let decoded: AgreementVersionConfig =
            toml::from_str(&encoded).expect("agreement config should deserialize");
        assert_eq!(decoded.accepted_version, CURRENT_AGREEMENT_VERSION);
    }
}
