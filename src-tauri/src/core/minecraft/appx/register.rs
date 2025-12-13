use std::io;
use tracing::{error, info};
use windows::core::{Error as WinError, Result as WinResult, HRESULT, HSTRING};
use windows::Foundation::Uri;
use windows::Management::Deployment::{DeploymentOptions, DeploymentResult, PackageManager};

pub async fn register_appx_package_async(package_folder: &str) -> WinResult<DeploymentResult> {
    // 准备 manifest 路径
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
    info!("注册 APPX，使用 URI：{}", uri_str);

    let package_manager = PackageManager::new().expect("无法创建 PackageManager");
    let uri = Uri::CreateUri(&HSTRING::from(uri_str))?;

    // 启动异步注册并 await 完成
    let async_op =
        package_manager.RegisterPackageAsync(&uri, None, DeploymentOptions::DevelopmentMode)?;
    let result: DeploymentResult = async_op.await?; // ← 关键：使用 await 而不是 get()

    // 检查结果信息
    let extended_error = result.ExtendedErrorCode()?; // 返回 HRESULT
    let error_text_h = result.ErrorText()?; // 返回 HSTRING
    let error_text = error_text_h.to_string_lossy();

    if extended_error == HRESULT(0) {
        info!("APPX 注册成功");
        Ok(result)
    } else {
        error!("APPX 注册失败: {:?} - {}", extended_error, error_text);
        Err(WinError::new(extended_error, error_text))
    }
}
