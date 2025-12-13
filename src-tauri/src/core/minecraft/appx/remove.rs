use tracing::{error, info};
use windows::core::{Error as WinError, Result as WinResult, HRESULT, HSTRING};
use windows::Management::Deployment::{DeploymentResult, PackageManager, RemovalOptions};

/// 卸载 Appx 包：传入 packageFamilyName
pub async fn remove_package(package_family_name: &str) -> WinResult<()> {
    let package_manager = PackageManager::new().map_err(|e| {
        error!("无法创建 PackageManager: {:?}", e);
        e
    })?;

    // 发起异步卸载请求（注意：这是一个 COM async 对象）
    let async_op = package_manager.RemovePackageWithOptionsAsync(
        &HSTRING::from(package_family_name),
        RemovalOptions::PreserveApplicationData,
    )?;

    // 关键：await 异步操作（而不是调用 .get()）
    let result: DeploymentResult = async_op.await?;

    // 获取扩展错误码（HRESULT）和错误文本（HSTRING -> String）
    let extended_hr: HRESULT = result.ExtendedErrorCode()?;
    let error_text: String = match result.ErrorText() {
        Ok(h) => h.to_string_lossy(),
        Err(_) => String::new(),
    };

    if extended_hr == HRESULT(0) {
        info!("包成功移除: {}", package_family_name);
        Ok(())
    } else {
        error!(
            "移除包失败，扩展错误代码: {:?}, 错误文本: {}",
            extended_hr, error_text
        );
        Err(WinError::new(extended_hr, error_text))
    }
}
