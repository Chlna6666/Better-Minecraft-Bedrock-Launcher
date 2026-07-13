use crate::utils::developer_mode;
use crate::utils::mc_dependency::{
    GameInputInstallPlan, MissingUwpDependency, WindowsAppSdkInstallPlan,
    compute_missing_uwp_dependencies, plan_game_input_install, plan_windows_app_sdk_install,
};
use tracing::{debug, info};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchPlatform {
    Uwp,
    Gdk,
}

#[derive(Clone, Debug)]
pub struct LaunchPrerequisiteCheck {
    pub platform: LaunchPlatform,
    pub developer_mode_required: bool,
    pub missing_uwp_dependencies: Vec<MissingUwpDependency>,
    pub game_input_plan: Option<GameInputInstallPlan>,
    pub windows_app_sdk_plan: Option<WindowsAppSdkInstallPlan>,
}

impl LaunchPrerequisiteCheck {
    pub fn has_issues(&self) -> bool {
        self.developer_mode_required
            || !self.missing_uwp_dependencies.is_empty()
            || self.game_input_plan.is_some()
            || self.windows_app_sdk_plan.is_some()
    }
}

pub fn detect_launch_platform(kind: &str) -> LaunchPlatform {
    let platform = if kind.eq_ignore_ascii_case("gdk") {
        LaunchPlatform::Gdk
    } else {
        LaunchPlatform::Uwp
    };
    debug!(kind, platform = ?platform, "已识别启动平台");
    platform
}

pub fn check_launch_prerequisites(kind: &str, package_folder: &str) -> LaunchPrerequisiteCheck {
    let platform = detect_launch_platform(kind);
    let check = match platform {
        LaunchPlatform::Uwp => LaunchPrerequisiteCheck {
            platform,
            developer_mode_required: !developer_mode::is_developer_mode_enabled(),
            missing_uwp_dependencies: compute_missing_uwp_dependencies(),
            game_input_plan: None,
            windows_app_sdk_plan: None,
        },
        LaunchPlatform::Gdk => LaunchPrerequisiteCheck {
            platform,
            developer_mode_required: false,
            missing_uwp_dependencies: Vec::new(),
            game_input_plan: plan_game_input_install(package_folder),
            windows_app_sdk_plan: plan_windows_app_sdk_install(package_folder),
        },
    };

    info!(
        kind,
        package_folder,
        platform = ?check.platform,
        developer_mode_required = check.developer_mode_required,
        missing_uwp_dependencies = check.missing_uwp_dependencies.len(),
        game_input_required = check.game_input_plan.is_some(),
        windows_app_sdk_required = check.windows_app_sdk_plan.is_some(),
        "启动前检查已完成"
    );
    check
}
