#![cfg(target_os = "windows")]

use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

const RELEASE_FAMILY: &str = "Microsoft.MinecraftUWP_8wekyb3d8bbwe";
const PREVIEW_FAMILY: &str = "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MinecraftUwpChannel {
    Release,
    Preview,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemUwpRegistration {
    pub channel: MinecraftUwpChannel,
    pub family_name: String,
    pub version: Option<String>,
}

/// 查询当前用户是否存在 Microsoft Store / 系统安装方式的 Minecraft UWP 主包。
///
/// 这里只读取 PackageManager 注册元数据，不访问 LocalState，也不会扫描世界文件。
/// DevelopmentMode 包（包括 BMCBL 自己管理的 loose UWP）会被忽略。
pub fn system_registration(channel: MinecraftUwpChannel) -> Option<SystemUwpRegistration> {
    let family_name = match channel {
        MinecraftUwpChannel::Release => RELEASE_FAMILY,
        MinecraftUwpChannel::Preview => PREVIEW_FAMILY,
    };
    system_registration_for_family(channel, family_name)
}

/// 导入 APPX/ZIP 时尚未解析目标包家族，因此依次检查正式版和 Preview 的系统注册。
pub fn any_system_registration() -> Option<SystemUwpRegistration> {
    system_registration(MinecraftUwpChannel::Release)
        .or_else(|| system_registration(MinecraftUwpChannel::Preview))
}

fn system_registration_for_family(
    channel: MinecraftUwpChannel,
    family_name: &str,
) -> Option<SystemUwpRegistration> {
    let manager = PackageManager::new().ok()?;
    let packages = manager
        .FindPackagesByUserSecurityIdPackageFamilyName(
            &HSTRING::new(),
            &HSTRING::from(family_name),
        )
        .ok()?;

    for package in packages {
        let Ok(id) = package.Id() else {
            continue;
        };
        if id.ResourceId().is_ok_and(|resource_id| !resource_id.is_empty()) {
            continue;
        }
        if package.IsDevelopmentMode().unwrap_or(false) {
            continue;
        }

        let version = id.Version().ok().map(|version| {
            format!(
                "{}.{}.{}.{}",
                version.Major, version.Minor, version.Build, version.Revision
            )
        });
        return Some(SystemUwpRegistration {
            channel,
            family_name: family_name.to_string(),
            version,
        });
    }

    None
}
