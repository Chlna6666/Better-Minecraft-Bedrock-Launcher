use std::io;
use tracing::{error, info};
use windows::Foundation::Uri;
use windows::Management::Deployment::{DeploymentOptions, DeploymentResult, PackageManager};
use windows::core::{Error as WinError, HRESULT, HSTRING, Result as WinResult};

pub async fn register_appx_package_async(package_folder: &str) -> WinResult<DeploymentResult> {
    // 使用散装 AppX 的开发者注册模式，这样当前用户可直接注册，无需管理员。
    let mut manifest_path = package_folder.replace('\\', "/");
    if manifest_path.ends_with('/') {
        manifest_path.pop();
    }

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
    info!("注册 APPX (DevelopmentMode)，使用 URI：{}", uri_str);

    let package_manager = PackageManager::new().expect("无法创建 PackageManager");
    let uri = Uri::CreateUri(&HSTRING::from(uri_str))?;

    let async_op =
        package_manager.RegisterPackageAsync(&uri, None, DeploymentOptions::DevelopmentMode)?;
    let result: DeploymentResult = async_op.await?;

    // 检查结果信息
    let extended_error = result.ExtendedErrorCode()?; // 返回 HRESULT
    let error_text_h = result.ErrorText()?; // 返回 HSTRING
    let error_text = error_text_h.to_string_lossy();

    if extended_error == HRESULT(0) {
        info!("APPX DevelopmentMode 注册成功");
        Ok(result)
    } else {
        error!(
            "APPX DevelopmentMode 注册失败: {:?} - {}",
            extended_error, error_text
        );
        Err(WinError::new(extended_error, error_text))
    }
}
