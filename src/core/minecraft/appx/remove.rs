use tracing::{debug, error, info, warn};
use windows::Management::Deployment::{DeploymentResult, PackageManager, RemovalOptions};
use windows::core::{Error as WinError, HRESULT, HSTRING, Result as WinResult};

/// 移除当前用户下通过 DevelopmentMode 注册的散装 AppX 包。
pub async fn remove_package(package_family_name: &str) -> WinResult<()> {
    let package_manager = PackageManager::new().map_err(|e| {
        error!("无法创建 PackageManager: {:?}", e);
        e
    })?;

    debug!(
        "正在查找当前用户已安装的 DevelopmentMode 包实例: {}",
        package_family_name
    );

    let mut target_full_names = Vec::new();

    {
        let packages = package_manager.FindPackagesByUserSecurityIdPackageFamilyName(
            &HSTRING::new(),
            &HSTRING::from(package_family_name),
        )?;

        for pkg in packages {
            if let Ok(id) = pkg.Id() {
                if let Ok(full_name) = id.FullName() {
                    target_full_names.push(full_name);
                }
            }
        }
    } // <--- packages 迭代器在此处被释放

    let mut found_any = false;

    for full_name in target_full_names {
        found_any = true;
        let full_name_str = full_name.to_string_lossy();
        debug!("找到实例: {} -> 准备按当前用户模式移除", full_name_str);

        let async_op =
            package_manager.RemovePackageWithOptionsAsync(&full_name, RemovalOptions::None)?;

        let result: DeploymentResult = async_op.await?;
        let extended_hr: HRESULT = result.ExtendedErrorCode()?;
        let error_text = result
            .ErrorText()
            .map(|h| h.to_string_lossy())
            .unwrap_or_default();

        if extended_hr == HRESULT(0) {
            info!("DevelopmentMode 包成功移除: {}", full_name_str);
        } else {
            if extended_hr == HRESULT(0x80073CFAu32 as i32) {
                warn!(
                    "移除返回 DevMode 相关错误 0x80073CFA，当前实例可能不是当前用户的散装注册包。"
                );
            }
            error!(
                "移除包失败: {}, 代码: {:?}, 信息: {}",
                full_name_str, extended_hr, error_text
            );
            return Err(WinError::new(extended_hr, error_text));
        }
    }

    if !found_any {
        info!(
            "未找到当前用户已安装的包实例 ({})，跳过移除。",
            package_family_name
        );
    }

    Ok(())
}
