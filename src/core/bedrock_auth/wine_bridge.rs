use secrecy::{ExposeSecret as _, SecretString};
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use tempfile::TempDir;

const WINE_GDK_SECTION: &str = "[Software\\\\Wine\\\\WineGDK]";
const REFRESH_TOKEN_VALUE: &str = "\"RefreshToken\"=";

pub(crate) struct PreparedLaunchAuth {
    temporary_directory: TempDir,
    registry_path: PathBuf,
}

impl PreparedLaunchAuth {
    pub(crate) fn apply_to_command(
        &self,
        command: &mut tokio::process::Command,
    ) -> Result<(), String> {
        let device_path = self.temporary_directory.path().join("device.json");
        command.env("WINEGDK_PREAUTH_DEVICE", wine_z_path(&device_path)?);
        Ok(())
    }
}

impl Drop for PreparedLaunchAuth {
    fn drop(&mut self) {
        let Ok(_guard) = super::ACCOUNT_LOCK.lock() else {
            tracing::warn!("could not acquire Xbox credential cleanup lock");
            return;
        };
        if let Err(error) = remove_refresh_token_from_registry(&self.registry_path) {
            tracing::warn!(%error, "failed to remove temporary WineGDK credential");
        }
    }
}

pub(super) fn prepare(
    prefix_path: &Path,
    refresh_token: &SecretString,
    device_json: &[u8],
) -> Result<PreparedLaunchAuth, String> {
    let registry_path = prefix_path.join("pfx/system.reg");
    if !registry_path.is_file() {
        return Err(format!(
            "Wine/Proton 前缀缺少系统注册表：{}",
            registry_path.display()
        ));
    }

    let temporary_directory = secure_temporary_directory()?;
    let device_path = temporary_directory.path().join("device.json");
    write_private_file(&device_path, device_json)?;
    set_refresh_token_in_registry(&registry_path, refresh_token.expose_secret())?;

    Ok(PreparedLaunchAuth {
        temporary_directory,
        registry_path,
    })
}

pub(super) fn clear_all_prefix_credentials() -> Result<(), String> {
    let prefixes_directory = crate::utils::file_ops::prefixes_dir();
    let entries = match std::fs::read_dir(&prefixes_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "读取兼容环境目录 {} 失败：{error}",
                prefixes_directory.display()
            ));
        }
    };
    let mut failures = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                let registry_path = entry.path().join("pfx/system.reg");
                if registry_path.is_file()
                    && let Err(error) = remove_refresh_token_from_registry(&registry_path)
                {
                    failures.push(error);
                }
            }
            Err(error) => failures.push(format!("读取兼容环境条目失败：{error}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("；"))
    }
}

pub(super) fn clear_stale_temporary_credentials() -> Result<(), String> {
    let shared_memory = Path::new("/dev/shm");
    if !shared_memory.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(shared_memory)
        .map_err(|error| format!("读取临时凭证目录失败：{error}"))?;
    let mut failures = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failures.push(format!("读取临时凭证条目失败：{error}"));
                continue;
            }
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("bmcbl-auth-") {
            continue;
        }
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() && !file_type.is_symlink() => {
                if let Err(error) = std::fs::remove_dir_all(entry.path()) {
                    failures.push(format!("清除旧的登录临时凭证失败：{error}"));
                }
            }
            Ok(_) => {}
            Err(error) => failures.push(format!("检查登录临时凭证失败：{error}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("；"))
    }
}

fn secure_temporary_directory() -> Result<TempDir, String> {
    let preferred = Path::new("/dev/shm");
    let temporary_directory = if preferred.is_dir() {
        tempfile::Builder::new()
            .prefix("bmcbl-auth-")
            .tempdir_in(preferred)
    } else {
        tempfile::Builder::new().prefix("bmcbl-auth-").tempdir()
    }
    .map_err(|error| format!("创建登录凭证临时目录失败：{error}"))?;
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(
        temporary_directory.path(),
        std::fs::Permissions::from_mode(0o700),
    )
    .map_err(|error| format!("限制登录凭证临时目录权限失败：{error}"))?;
    Ok(temporary_directory)
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("创建 WineGDK 预认证文件失败：{error}"))?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("写入 WineGDK 预认证文件失败：{error}"))
}

fn set_refresh_token_in_registry(path: &Path, token: &str) -> Result<(), String> {
    let token = escape_registry_string(token)?;
    update_registry(path, |contents| {
        update_registry_section(contents, Some(&format!("\"{token}\"")))
    })
}

fn remove_refresh_token_from_registry(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    update_registry(path, |contents| update_registry_section(contents, None))
}

fn update_registry(path: &Path, update: impl FnOnce(&str) -> String) -> Result<(), String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("读取 WineGDK 注册表失败：{error}"))?;
    if !contents.starts_with("WINE REGISTRY Version") {
        return Err("拒绝修改格式无效的 Wine 系统注册表".to_string());
    }
    let updated = update(&contents);
    if updated == contents {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "WineGDK 注册表路径没有父目录".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("创建 WineGDK 注册表临时文件失败：{error}"))?;
    temporary
        .write_all(updated.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("写入 WineGDK 注册表临时文件失败：{error}"))?;
    if let Ok(metadata) = std::fs::metadata(path) {
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(|error| format!("保留 WineGDK 注册表权限失败：{error}"))?;
    }
    temporary
        .persist(path)
        .map_err(|error| format!("原子保存 WineGDK 注册表失败：{}", error.error))?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("同步 WineGDK 注册表目录失败：{error}"))?;
    Ok(())
}

fn update_registry_section(contents: &str, value: Option<&str>) -> String {
    let mut output = Vec::new();
    let mut in_section = false;
    let mut section_found = false;
    let mut value_written = false;

    for line in contents.lines() {
        let is_section = line.starts_with('[');
        if is_section && in_section && !value_written {
            if let Some(value) = value {
                output.push(format!("{REFRESH_TOKEN_VALUE}{value}"));
                value_written = true;
            }
        }
        if is_section {
            in_section = line.starts_with(WINE_GDK_SECTION);
            section_found |= in_section;
        }
        if in_section && line.starts_with(REFRESH_TOKEN_VALUE) {
            if let Some(value) = value
                && !value_written
            {
                output.push(format!("{REFRESH_TOKEN_VALUE}{value}"));
                value_written = true;
            }
            continue;
        }
        output.push(line.to_string());
    }

    if in_section && !value_written {
        if let Some(value) = value {
            output.push(format!("{REFRESH_TOKEN_VALUE}{value}"));
        }
    } else if !section_found {
        if let Some(value) = value {
            if !output.last().is_none_or(String::is_empty) {
                output.push(String::new());
            }
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs());
            output.push(format!("{WINE_GDK_SECTION} {timestamp}"));
            output.push(format!("{REFRESH_TOKEN_VALUE}{value}"));
        }
    }

    if contents.ends_with('\n') || output.is_empty() {
        format!("{}\n", output.join("\n"))
    } else {
        output.join("\n")
    }
}

fn escape_registry_string(value: &str) -> Result<String, String> {
    if value
        .chars()
        .any(|character| matches!(character, '\0' | '\r' | '\n'))
    {
        return Err("Microsoft 登录凭证包含无法写入注册表的字符".to_string());
    }
    Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn wine_z_path(path: &Path) -> Result<OsString, String> {
    if !path.is_absolute() {
        return Err(format!("无法转换相对的 Wine 路径：{}", path.display()));
    }
    let mut windows_path = OsString::from("Z:");
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(value) => {
                windows_path.push(OsStr::new("\\"));
                windows_path.push(value);
            }
            Component::ParentDir | Component::Prefix(_) => {
                return Err(format!("无法转换 Wine 路径：{}", path.display()));
            }
        }
    }
    Ok(windows_path)
}

#[cfg(test)]
mod tests {
    use super::{escape_registry_string, update_registry_section};

    #[test]
    fn registry_value_is_replaced_and_removed() {
        let input = "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\WineGDK] 1\n\"Old\"=\"x\"\n\"RefreshToken\"=\"old\"\n\n[System] 2\n";
        let replaced = update_registry_section(input, Some("\"new\""));
        assert!(replaced.contains("\"RefreshToken\"=\"new\""));
        assert!(!replaced.contains("\"RefreshToken\"=\"old\""));
        let removed = update_registry_section(&replaced, None);
        assert!(!removed.contains("\"RefreshToken\"="));
        assert!(removed.contains("\"Old\"=\"x\""));
    }

    #[test]
    fn registry_section_is_created_when_missing() {
        let output = update_registry_section("WINE REGISTRY Version 2\n", Some("\"token\""));
        assert!(output.contains("[Software\\\\Wine\\\\WineGDK] "));
        assert!(output.contains("\"RefreshToken\"=\"token\""));
    }

    #[test]
    fn registry_escaping_rejects_line_breaks() {
        assert!(escape_registry_string("bad\nvalue").is_err());
        assert_eq!(
            escape_registry_string("a\\b\"c").expect("valid registry value"),
            "a\\\\b\\\"c"
        );
    }
}
