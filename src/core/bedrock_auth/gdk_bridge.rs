use std::io::Write as _;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub(crate) struct PreparedLaunchAuth {
    temporary_directory: TempDir,
    profile_id: String,
    pub(crate) gamertag: String,
    nonce: String,
}

impl PreparedLaunchAuth {
    pub(crate) fn get_env_vars(&self) -> Vec<(String, String)> {
        let device_path = self.temporary_directory.path().join("device.json");
        vec![
            ("BMCBL_XGAMERUNTIME_PROFILE".to_string(), self.profile_id.clone()),
            (
                "BMCBL_XGAMERUNTIME_PREAUTH".to_string(),
                device_path.to_string_lossy().to_string(),
            ),
            ("BMCBL_XGAMERUNTIME_NONCE".to_string(), self.nonce.clone()),
            ("BMCBL_XGAMERUNTIME_ENABLE_XUSER".to_string(), "1".to_string()),
        ]
    }
}

pub(super) fn prepare(
    profile_id: &str,
    gamertag: &str,
    device_json: &[u8],
) -> Result<PreparedLaunchAuth, String> {
    let temporary_directory = secure_temporary_directory()?;
    let device_path = temporary_directory.path().join("device.json");
    write_private_file(&device_path, device_json)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string();

    let _ = crate::core::inject::inject::grant_all_application_packages_access(temporary_directory.path());

    Ok(PreparedLaunchAuth {
        temporary_directory,
        profile_id: profile_id.to_string(),
        gamertag: gamertag.to_string(),
        nonce,
    })
}

fn secure_temporary_directory() -> Result<TempDir, String> {
    tempfile::Builder::new()
        .prefix("bmcbl-auth-")
        .tempdir()
        .map_err(|error| format!("创建登录凭证临时目录失败：{error}"))
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("创建 GDK 预认证文件失败：{error}"))?;
    file.write_all(contents)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("写入 GDK 预认证文件失败：{error}"))
}
