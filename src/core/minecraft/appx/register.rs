#![cfg(target_os = "windows")]
use std::io;
use std::path::Path;
use tracing::{error, info, warn};
use windows::Foundation::Uri;
use windows::Management::Deployment::{DeploymentOptions, DeploymentResult, PackageManager};
use windows::core::{Error as WinError, HRESULT, HSTRING, Result as WinResult};

pub async fn register_appx_package_async(package_folder: &str) -> WinResult<DeploymentResult> {
    // 使用散装 AppX 的开发者注册模式，这样当前用户可直接注册，无需管理员。
    let mut manifest_path = package_folder.replace('\\', "/");
    if manifest_path.ends_with('/') {
        manifest_path.pop();
    }

    let identity_name = crate::core::minecraft::appx::utils::get_manifest_identity_from_dir_blocking(
        Path::new(package_folder),
    )
    .map(|(name, _version)| name)
    .map_err(|error| {
        WinError::from(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("解析 AppX Identity 失败: {error}"),
        ))
    })?;

    let manifest_file = format!("{}/AppxManifest.xml", manifest_path);
    let absolute_path = std::fs::canonicalize(&manifest_file).map_err(|e| {
        windows::core::Error::from(io::Error::new(
            io::ErrorKind::Other,
            format!("获取绝对路径失败: {}", e),
        ))
    })?;

    let mut uri_path = absolute_path.to_string_lossy().to_string();
    if uri_path.starts_with(r"\\?\") {
        uri_path = uri_path[4..].to_string();
    }

    let uri_str = format!("file:///{}", uri_path.replace("\\", "/"));
    info!(identity_name, "注册 APPX (DevelopmentMode)，使用 URI：{}", uri_str);

    let package_manager = PackageManager::new().expect("无法创建 PackageManager");
    let uri = Uri::CreateUri(&HSTRING::from(uri_str))?;
    let async_op =
        package_manager.RegisterPackageAsync(&uri, None, DeploymentOptions::DevelopmentMode)?;
    let result: DeploymentResult = async_op.await?;

    let extended_error = result.ExtendedErrorCode()?;
    let error_text_h = result.ErrorText()?;
    let error_text = error_text_h.to_string_lossy();

    if extended_error == HRESULT(0) {
        info!(identity_name, "APPX DevelopmentMode 注册成功");
        match crate::core::minecraft::uwp_migration::restore_pending_backup_for_identity(
            &identity_name,
        ) {
            Ok(Some(path)) => info!(
                identity_name,
                backup = %path.display(),
                "原版 UWP 用户数据已恢复到新注册的散装版本"
            ),
            Ok(None) => {}
            Err(error) => {
                warn!(
                    identity_name,
                    %error,
                    "散装 UWP 已注册，但原版数据自动恢复失败；保留外部迁移备份以供重试"
                );
                return Err(WinError::from(io::Error::other(format!(
                    "APPX 已注册，但 Minecraft 数据恢复失败: {error}"
                ))));
            }
        }
        Ok(result)
    } else {
        error!(
            "APPX DevelopmentMode 注册失败: {:?} - {}",
            extended_error, error_text
        );
        Err(WinError::new(extended_error, error_text))
    }
}
