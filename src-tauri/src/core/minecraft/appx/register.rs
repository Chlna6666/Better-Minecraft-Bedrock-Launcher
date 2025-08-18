use std::io;
use tracing::{info, error, debug};
use windows::core::{HSTRING, Result, HRESULT};
use windows::Foundation::{Uri};
use windows::Management::Deployment::{DeploymentOptions,  DeploymentResult, PackageManager};

use crate::core::minecraft::appx::utils::{ get_package_info};


pub async fn register_appx_package_async(package_folder: &str) -> Result<DeploymentResult> {
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

    let async_result = package_manager.RegisterPackageAsync(&uri, None, DeploymentOptions::DevelopmentMode)?;
    match async_result.get() {
        Ok(result) => {
            let extended_error = result.ExtendedErrorCode().unwrap_or(HRESULT(0));
            let error_text = result.ErrorText().unwrap_or(HSTRING::new()).to_string_lossy();

            if extended_error == HRESULT(0) {
                info!("APPX 注册成功");
                Ok(result)
            } else {
                error!("APPX 注册失败");
                error!("错误代码: {:?}", extended_error);
                error!("错误信息: {}", error_text);
                Err(windows::core::Error::new(extended_error, error_text))
            }
        }
        Err(e) => {
            error!("等待 APPX 注册异步操作失败: {:?}", e);
            Err(e)
        }
    }
}
