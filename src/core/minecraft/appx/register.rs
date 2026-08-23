#![cfg(target_os = "windows")]
use std::io;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};
use windows::Foundation::Uri;
use windows::Management::Deployment::{DeploymentOptions, DeploymentResult, PackageManager};
use windows::core::{Error as WinError, HRESULT, HSTRING, Result as WinResult};

fn minecraft_package_family_name(identity_name: &str) -> String {
    // Minecraft 正式版、Preview、Education 等微软包均使用该 PublisherId。
    // 这里用于 RegisterPackageAsync 成功后的当前用户注册校验，不参与包身份生成。
    format!("{identity_name}_8wekyb3d8bbwe")
}

fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn same_windows_path(left: &Path, right: &Path) -> bool {
    let left = canonical_or_original(left);
    let right = canonical_or_original(right);
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

fn verify_development_registration(
    package_manager: &PackageManager,
    identity_name: &str,
    package_folder: &str,
) -> WinResult<()> {
    let family_name = minecraft_package_family_name(identity_name);
    let packages = package_manager.FindPackagesByUserSecurityIdPackageFamilyName(
        &HSTRING::new(),
        &HSTRING::from(family_name.as_str()),
    )?;
    let expected_path = Path::new(package_folder);
    let mut observed = Vec::new();

    for package in packages {
        let Ok(id) = package.Id() else {
            continue;
        };
        if id.ResourceId().is_ok_and(|resource_id| !resource_id.is_empty()) {
            continue;
        }
        let Ok(name) = id.Name() else {
            continue;
        };
        if name.to_string_lossy() != identity_name {
            continue;
        }

        let development_mode = package.IsDevelopmentMode().unwrap_or(false);
        let installed_path = package
            .InstalledLocation()
            .ok()
            .and_then(|location| location.Path().ok())
            .map(|path| PathBuf::from(path.to_string_lossy().to_string()));

        if development_mode
            && installed_path
                .as_deref()
                .is_some_and(|path| same_windows_path(path, expected_path))
        {
            info!(
                identity_name,
                family_name,
                installed_path = %installed_path.as_ref().expect("checked above").display(),
                "APPX 注册后校验通过：DevelopmentMode 与目标目录一致"
            );
            return Ok(());
        }

        observed.push(format!(
            "development_mode={development_mode}, path={}",
            installed_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string())
        ));
    }

    let detail = if observed.is_empty() {
        "未找到对应的当前用户主包注册".to_string()
    } else {
        observed.join("; ")
    };
    Err(WinError::from(io::Error::other(format!(
        "APPX 注册调用已返回成功，但注册后校验失败：期望 DevelopmentMode 且路径为 {}，实际：{detail}",
        expected_path.display()
    ))))
}

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
        windows::core::Error::from(io::Error::other(format!(
            "获取绝对路径失败: {e}"
        )))
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
        info!(identity_name, "APPX DevelopmentMode 注册调用成功，开始执行注册后校验");
        verify_development_registration(&package_manager, &identity_name, package_folder)?;

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
