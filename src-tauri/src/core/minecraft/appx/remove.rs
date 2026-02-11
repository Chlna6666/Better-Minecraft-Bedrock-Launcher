use tracing::{debug, error, info, warn};
use windows::core::{Error as WinError, Result as WinResult, HRESULT, HSTRING};
use windows::Management::Deployment::{DeploymentResult, PackageManager, RemovalOptions};

/// 卸载 Appx 包
///
/// 修复说明：
/// 1. 分离查找和卸载逻辑，避免在 await 期间持有 !Send 的迭代器。
/// 2. 自动查找 FullName 并处理 DevMode 卸载限制。
pub async fn remove_package(package_family_name: &str) -> WinResult<()> {
    let package_manager = PackageManager::new().map_err(|e| {
        error!("无法创建 PackageManager: {:?}", e);
        e
    })?;

    debug!("正在查找已安装的包实例: {}", package_family_name);

    // [关键修复]：先收集所有 FullName 到 Vec 中，而不是在迭代器循环中直接 await
    // 这样做是为了让迭代器 'packages' 在进入异步操作前就被销毁，避免 Send 错误
    let mut target_full_names = Vec::new();

    // 使用独立作用域确保迭代器被丢弃
    {
        let packages = package_manager.FindPackagesByUserSecurityIdPackageFamilyName(
            &HSTRING::new(), // 当前用户
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

    // 现在遍历 Vec，这里的 full_name (HSTRING) 是 Send 的，可以安全跨越 await
    for full_name in target_full_names {
        found_any = true;
        let full_name_str = full_name.to_string_lossy();
        debug!("找到实例: {} -> 准备卸载", full_name_str);

        // --- 执行卸载逻辑 ---

        // A. 优先尝试带 PreserveApplicationData (为了保护正常安装的存档)
        debug!("尝试卸载 (保留数据): {}", full_name_str);
        let async_op = package_manager.RemovePackageWithOptionsAsync(
            &full_name,
            RemovalOptions::PreserveApplicationData,
        )?;

        let result: DeploymentResult = async_op.await?;

        // 检查结果
        let mut extended_hr: HRESULT = result.ExtendedErrorCode()?;
        let mut error_text: String = result.ErrorText().map(|h| h.to_string_lossy()).unwrap_or_default();

        // B. 如果遇到 0x80073CFA (DevMode 限制)，降级为普通卸载
        if extended_hr == HRESULT(0x80073CFAu32 as i32) {
            warn!("卸载失败 (DevMode限制 0x80073CFA)，正在尝试普通卸载模式 (不保留数据)...");

            let async_op_retry = package_manager.RemovePackageWithOptionsAsync(
                &full_name,
                RemovalOptions::None,
            )?;

            let result_retry: DeploymentResult = async_op_retry.await?;
            extended_hr = result_retry.ExtendedErrorCode()?;
            error_text = result_retry.ErrorText().map(|h| h.to_string_lossy()).unwrap_or_default();
        }

        if extended_hr == HRESULT(0) {
            info!("包成功移除: {}", full_name_str);
        } else {
            error!(
                "移除包失败: {}, 代码: {:?}, 信息: {}",
                full_name_str, extended_hr, error_text
            );
            // 这里选择返回错误，这会中断后续操作（如果有多个包的话）
            return Err(WinError::new(extended_hr, error_text));
        }
    }

    if !found_any {
        info!("未找到已安装的包实例 ({})，跳过卸载。", package_family_name);
    }

    Ok(())
}