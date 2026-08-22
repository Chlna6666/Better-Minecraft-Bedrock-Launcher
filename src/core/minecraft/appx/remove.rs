#![cfg(target_os = "windows")]
use std::io;
use std::path::PathBuf;
use tracing::{debug, error, info, warn};
use windows::Management::Deployment::{DeploymentResult, PackageManager, RemovalOptions};
use windows::core::{Error as WinError, HRESULT, HSTRING, Result as WinResult};

#[derive(Debug)]
struct RemovalTarget {
    full_name: HSTRING,
    installed_path: Option<PathBuf>,
    development_mode: bool,
    bmcbl_managed: bool,
}

/// 移除当前用户下已注册的同包家族主包。
///
/// 只有同时满足以下条件的注册才属于 BMCBL 自己管理的散装包：
/// - Windows 报告该包以 DevelopmentMode 部署；
/// - 安装路径位于 BMCBL/versions 下。
///
/// 对这种注册使用 PreserveApplicationData，以便不同 Minecraft 散装版本之间切换时
/// 保留 LocalState。Store/外部注册在卸载前必须先完成可验证的外部数据备份；
/// 备份失败则拒绝卸载。资源包（ResourceId 非空）由部署系统随主包处理，不参与
/// 注册来源判定，也不单独执行 RemovePackageWithOptionsAsync。
pub async fn remove_package(package_family_name: &str) -> WinResult<()> {
    let package_manager = PackageManager::new().map_err(|e| {
        error!("无法创建 PackageManager: {:?}", e);
        e
    })?;

    debug!("正在查找当前用户已安装的包实例: {}", package_family_name);
    let mut targets = Vec::new();

    {
        let packages = package_manager.FindPackagesByUserSecurityIdPackageFamilyName(
            &HSTRING::new(),
            &HSTRING::from(package_family_name),
        )?;

        for package in packages {
            let Ok(id) = package.Id() else { continue };
            if id.ResourceId().is_ok_and(|resource_id| !resource_id.is_empty()) {
                debug!(
                    family_name = package_family_name,
                    "跳过同包家族资源包，注册替换仅处理主包"
                );
                continue;
            }

            let Ok(full_name) = id.FullName() else { continue };
            let installed_path = package
                .InstalledLocation()
                .ok()
                .and_then(|location| location.Path().ok())
                .map(|path| PathBuf::from(path.to_string_lossy().to_string()));
            let development_mode = package.IsDevelopmentMode().unwrap_or(false);
            let bmcbl_managed = development_mode
                && installed_path
                    .as_deref()
                    .is_some_and(crate::core::minecraft::uwp_migration::is_bmcbl_managed_registration);
            targets.push(RemovalTarget {
                full_name,
                installed_path,
                development_mode,
                bmcbl_managed,
            });
        }
    }

    if targets.is_empty() {
        info!("未找到当前用户已安装的主包实例 ({})，跳过移除。", package_family_name);
        return Ok(());
    }

    let has_external_registration = targets.iter().any(|target| !target.bmcbl_managed);
    if has_external_registration {
        match crate::core::minecraft::uwp_migration::prepare_external_registration_backup(
            package_family_name,
        ) {
            Ok(Some(path)) => info!(
                family_name = package_family_name,
                backup = %path.display(),
                "外部/Store UWP 数据已完成迁移备份，允许继续替换注册"
            ),
            Ok(None) => info!(
                family_name = package_family_name,
                "外部/Store UWP 未检测到需要迁移的 com.mojang 数据"
            ),
            Err(error) => {
                error!(family_name = package_family_name, %error, "UWP 数据安全备份失败，拒绝卸载");
                return Err(WinError::from(io::Error::other(error)));
            }
        }
    }

    for target in targets {
        let full_name_str = target.full_name.to_string_lossy();
        let removal_options = if target.bmcbl_managed {
            RemovalOptions::PreserveApplicationData
        } else {
            RemovalOptions::None
        };
        debug!(
            full_name = %full_name_str,
            installed_path = ?target.installed_path,
            development_mode = target.development_mode,
            bmcbl_managed = target.bmcbl_managed,
            preserve_application_data = target.bmcbl_managed,
            "准备按当前用户模式移除 UWP 主包注册"
        );

        let async_op = package_manager
            .RemovePackageWithOptionsAsync(&target.full_name, removal_options)?;
        let result: DeploymentResult = async_op.await?;
        let extended_hr: HRESULT = result.ExtendedErrorCode()?;
        let error_text = result
            .ErrorText()
            .map(|h| h.to_string_lossy())
            .unwrap_or_default();

        if extended_hr == HRESULT(0) {
            info!(
                full_name = %full_name_str,
                development_mode = target.development_mode,
                preserve_application_data = target.bmcbl_managed,
                "UWP 主包注册成功移除"
            );
        } else {
            if extended_hr == HRESULT(0x80073CFAu32 as i32) {
                warn!("移除返回 0x80073CFA，当前实例可能不支持所请求的移除模式");
            }
            error!(
                "移除包失败: {}, 代码: {:?}, 信息: {}",
                full_name_str, extended_hr, error_text
            );
            return Err(WinError::new(extended_hr, error_text));
        }
    }

    Ok(())
}
