use tracing::{info, error};
use windows::core::{HRESULT, HSTRING};
use windows::Management::Deployment::{PackageManager, RemovalOptions};

/// 卸载并重注册 Appx 包：传入 packageFamilyName
pub async fn remove_package(package_family_name: &str) {
    let package_manager = PackageManager::new().expect("无法创建 PackageManager");

    match package_manager.RemovePackageWithOptionsAsync(
        &HSTRING::from(package_family_name),
        RemovalOptions::PreserveApplicationData,
    ) {
        Ok(async_op) => {
            let async_result = async_op.get().expect("等待异步操作失败");

            match async_result.ExtendedErrorCode() {
                Ok(hr) if hr == HRESULT(0) => {
                    info!("包成功移除");
                }
                Ok(hr) => {
                    error!("移除包失败，扩展错误代码: {:?}", hr);
                    if let Ok(error_text) = async_result.ErrorText() {
                        error!("错误文本: {:?}", error_text);
                    }
                }
                Err(err) => {
                    error!("获取扩展错误码失败: {:?}", err);
                }
            }
        }
        Err(err) => {
            error!("调用 RemovePackageAsync 出错: {:?}", err);
        }
    }
}