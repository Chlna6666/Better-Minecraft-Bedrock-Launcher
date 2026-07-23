use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;
use gpui::{
    App, AppContext, AsyncApp, BorrowAppContext, ClipboardItem, Context, SharedString,
    Subscription, Timer,
};
use gpui_hooks::hooks::{UseRefHook, UseStateHook};
use tokio::sync::mpsc::{UnboundedReceiver, error::TryRecvError, unbounded_channel};
use tracing::{debug, info, warn};

use crate::core::minecraft::launcher::preflight::{
    LaunchPrerequisiteCheck, check_launch_prerequisites,
};
use crate::core::minecraft::launcher::task::embedded_dll_version_string;
use crate::core::minecraft::launcher::{LaunchRequest, start_launch_task};
use crate::i18n::Locale;
use crate::tasks::task_manager::{self, TaskSnapshot};
use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use crate::ui::state::launch_prereq::{
    LaunchPrereqOperation, LaunchPrereqState, PendingLaunchVersion,
};
use crate::ui::state::launcher::LauncherState;
use crate::utils::developer_mode::{self, DeveloperModeError};
use crate::utils::mc_dependency::{self, DependencyEvent};

const DEPENDENCY_EVENT_BATCH_DELAY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Default)]
pub struct LauncherSnapshot {
    pub show_modal: bool,
    pub modal_visible: bool,
    pub modal_animating: bool,
    pub modal_factor: f32,
    pub task_id: Option<Arc<str>>,
    pub version_folder: SharedString,
    pub version_name: SharedString,
    pub version: SharedString,
    pub kind: SharedString,
    pub package_path: SharedString,
    pub loader_version: SharedString,
    pub last_snapshot: Option<Arc<TaskSnapshot>>,
    pub logs: Arc<[Arc<str>]>,
    pub log_version: u64,
}

#[derive(Clone, Debug)]
pub struct LaunchVersionDescriptor {
    pub folder: SharedString,
    pub name: SharedString,
    pub version: SharedString,
    pub kind: SharedString,
    pub path: SharedString,
    pub launch_args: Option<SharedString>,
}

#[derive(Clone)]
struct ActiveLaunchPrereq {
    request_id: u64,
    version: PendingLaunchVersion,
    check: Option<LaunchPrerequisiteCheck>,
    locale: Locale,
    busy: bool,
}

#[derive(Default)]
struct DependencyEventBatch {
    logs: Vec<SharedString>,
    progress: Option<DependencyProgressUpdate>,
    admin_notice: Option<SharedString>,
}

struct DependencyProgressUpdate {
    percent: u32,
    stage: SharedString,
    target: Option<SharedString>,
}

impl DependencyEventBatch {
    fn push(&mut self, event: DependencyEvent) {
        match event {
            DependencyEvent::Log(message) => {
                self.logs.push(SharedString::from(message));
            }
            DependencyEvent::Progress {
                percent,
                stage,
                target,
            } => {
                self.progress = Some(DependencyProgressUpdate {
                    percent,
                    stage: SharedString::from(stage),
                    target: target.map(SharedString::from),
                });
            }
            DependencyEvent::AdminRequired(message) => {
                self.admin_notice = Some(SharedString::from(message));
            }
        }
    }

    fn apply(self, request_id: u64, state: &mut LaunchPrereqState) -> bool {
        let mut changed = false;

        for log in self.logs {
            changed |= state.push_log_if_matches(request_id, log);
        }

        if let Some(progress) = self.progress {
            changed |= state.update_progress_if_matches(
                request_id,
                progress.percent,
                progress.stage,
                progress.target,
            );
        }

        if let Some(admin_notice) = self.admin_notice {
            changed |= state.set_admin_notice_if_matches(request_id, admin_notice);
        }

        changed
    }
}

pub fn read_launcher_snapshot(now: std::time::Instant, cx: &App) -> LauncherSnapshot {
    cx.read_global(|state: &LauncherState, _cx| {
        let logs = state
            .task_id
            .as_ref()
            .map(|task_id| task_manager::task_logs(task_id.as_ref()))
            .unwrap_or_else(|| Arc::<[Arc<str>]>::from(Vec::<Arc<str>>::new()));
        let log_version = launcher_log_version(&logs, state.last_snapshot.as_deref());
        LauncherSnapshot {
            show_modal: state.show_modal,
            modal_visible: state.modal_visible,
            modal_animating: state.is_modal_animating(now),
            modal_factor: state.modal_animation_factor(now),
            task_id: state.task_id.clone(),
            version_folder: state.version_folder.clone(),
            version_name: state.version_name.clone(),
            version: state.version.clone(),
            kind: state.kind.clone(),
            package_path: state.package_path.clone(),
            loader_version: state.loader_version.clone(),
            last_snapshot: state.last_snapshot.clone(),
            logs,
            log_version,
        }
    })
}

fn launcher_log_version(logs: &[Arc<str>], last_snapshot: Option<&TaskSnapshot>) -> u64 {
    let mut version = logs.len() as u64;
    if let Some(last) = logs.last() {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in last.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        version ^= hash.rotate_left(7);
    }
    if let Some(snapshot) = last_snapshot {
        version ^= snapshot.sequence.rotate_left(17);
    }
    version
}

pub fn use_launcher<Hooks, View>(hooks: &Hooks, cx: &mut Context<View>) -> LauncherSnapshot
where
    Hooks: UseStateHook + UseRefHook,
    View: 'static,
{
    let now = std::time::Instant::now();
    let snapshot = hooks.use_state(|| read_launcher_snapshot(now, cx));
    let subscription = hooks.use_ref(|| None::<Subscription>);

    if subscription.borrow().is_none() {
        let snapshot = snapshot.clone();
        let observer = cx.observe_global::<LauncherState>(move |_, cx| {
            let now = std::time::Instant::now();
            let current = read_launcher_snapshot(now, cx);
            if snapshot.with(|value| launcher_snapshot_changed(value, &current)) {
                snapshot.set(current);
                cx.notify();
            }
        });
        *subscription.borrow_mut() = Some(observer);
    }

    let current = read_launcher_snapshot(now, cx);
    if snapshot.with(|value| launcher_snapshot_changed(value, &current)) {
        snapshot.set(current.clone());
    }

    current
}

pub fn start_launcher(version: LaunchVersionDescriptor, cx: &mut App) -> Option<Arc<str>> {
    if version.path.is_empty() {
        warn!(
            version_name = %version.name,
            version = %version.version,
            kind = %version.kind,
            "启动请求缺少包路径，已忽略"
        );
        return None;
    }

    if launch_flow_is_busy(cx) {
        debug!(
            version_name = %version.name,
            version = %version.version,
            kind = %version.kind,
            package_path = %version.path,
            "启动请求已忽略：当前已有启动流程正在执行"
        );
        return None;
    }

    let pending_version = pending_launch_version(&version);
    let request_id =
        cx.update_global(|state: &mut LaunchPrereqState, _cx| state.begin(pending_version.clone()));
    info!(
        request_id,
        version_name = %version.name,
        version = %version.version,
        kind = %version.kind,
        package_path = %version.path,
        "收到启动请求，开始执行启动前检查"
    );
    spawn_launch_prereq_check_for_request(pending_version, request_id, cx);
    Some(launch_prereq_ticket(request_id))
}

fn launch_flow_is_busy(cx: &App) -> bool {
    let prereq_busy = cx.read_global(|state: &LaunchPrereqState, _cx| state.has_active_request());
    let launcher_busy = cx.read_global(|state: &LauncherState, _cx| state.launch_in_progress());
    prereq_busy || launcher_busy
}

pub fn close_launcher(cx: &mut App) {
    let now = std::time::Instant::now();
    cx.update_global(|state: &mut LauncherState, _cx| {
        state.request_close(now);
    });
}

pub fn minimize_launcher(cx: &mut App) {
    cx.update_global(|state: &mut LauncherState, _cx| {
        state.dismiss_modal();
    });
}

pub fn cancel_launcher(cx: &mut App) {
    let task_id = cx
        .global::<LauncherState>()
        .task_id
        .as_ref()
        .map(|task_id| task_id.to_string());
    if let Some(task_id) = task_id {
        info!(task_id, "用户请求取消启动任务");
        task_manager::cancel_task(&task_id);
        let now = std::time::Instant::now();
        cx.update_global(|state: &mut LauncherState, _cx| {
            if state.task_id.as_deref() == Some(task_id.as_str()) {
                state.request_close(now);
            }
        });
    }
}

pub fn retry_launcher(cx: &mut App) -> Option<Arc<str>> {
    let descriptor = cx.read_global(|state: &LauncherState, _cx| {
        let is_error = state
            .last_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.status.as_ref() == "error");
        if !is_error || state.package_path.is_empty() {
            return None;
        }

        Some(LaunchVersionDescriptor {
            folder: state.version_folder.clone(),
            name: state.version_name.clone(),
            version: state.version.clone(),
            kind: state.kind.clone(),
            path: state.package_path.clone(),
            launch_args: (!state.launch_args.is_empty()).then(|| state.launch_args.clone()),
        })
    });

    descriptor.and_then(|version| start_launcher(version, cx))
}

pub fn copy_launcher_error(cx: &mut App) -> bool {
    let copy_text = cx.read_global(|state: &LauncherState, _cx| {
        let logs = state
            .task_id
            .as_ref()
            .map(|task_id| task_manager::task_logs(task_id.as_ref()))
            .unwrap_or_else(|| Arc::<[Arc<str>]>::from([]));
        let message = state
            .last_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.message.as_ref())
            .map(ToString::to_string);

        match (message, logs.is_empty()) {
            (Some(message), false) => {
                let joined = logs
                    .iter()
                    .map(|line| line.as_ref().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(format!("{message}\n\n—— 启动日志 ——\n{joined}"))
            }
            (Some(message), true) => Some(message),
            (None, false) => Some(
                logs.iter()
                    .map(|line| line.as_ref().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (None, true) => None,
        }
    });

    if let Some(text) = copy_text.filter(|text| !text.is_empty()) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        return true;
    }

    false
}

pub fn dismiss_launch_prereq(cx: &mut App) {
    cx.update_global(|state: &mut LaunchPrereqState, _cx| {
        if state.is_busy() {
            return;
        }
        state.dismiss();
    });
}

pub fn cancel_launch_prereq(cx: &mut App) {
    let cancelled = cx.update_global(|state: &mut LaunchPrereqState, _cx| {
        let version = state.version.as_ref().map(|version| version.name.clone());
        let request_id = state.cancel_active_request();
        (request_id, version)
    });

    if let (Some(request_id), Some(version_name)) = cancelled {
        info!(
            request_id,
            version_name = %version_name,
            "用户取消了启动前检查或依赖安装流程"
        );
    }
}

pub fn recheck_launch_prereq(cx: &mut App) {
    let version = cx.read_global(|state: &LaunchPrereqState, _cx| {
        if state.is_busy() {
            return None;
        }
        state.version.clone()
    });
    let Some(version) = version else {
        return;
    };

    let request_id =
        cx.update_global(|state: &mut LaunchPrereqState, _cx| state.begin(version.clone()));
    info!(
        request_id,
        version_name = %version.name,
        version = %version.version,
        kind = %version.kind,
        "用户请求重新执行启动前检查"
    );
    spawn_launch_prereq_check_for_request(version, request_id, cx);
}

pub fn open_launch_prereq_developer_settings(cx: &mut App) {
    let context = read_active_launch_prereq(cx);
    let Some(context) = context else {
        return;
    };
    if context.busy {
        return;
    }

    info!(
        request_id = context.request_id,
        version_name = %context.version.name,
        "打开开发者模式设置页"
    );
    let start_log = cx
        .global::<I18n>()
        .t("LaunchPrereq.logs.openDeveloperSettings");
    cx.update_global(|state: &mut LaunchPrereqState, _cx| {
        if state.set_operation_if_matches(
            context.request_id,
            LaunchPrereqOperation::OpeningDeveloperSettings,
        ) {
            let _ = state.push_log_if_matches(context.request_id, start_log);
        }
    });

    cx.spawn(async move |cx| {
        let result = tokio::task::spawn_blocking(developer_mode::open_developer_settings).await;
        match result {
            Ok(Ok(())) => {
                info!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    "开发者模式设置页已打开"
                );
                let opened_log = async_i18n_text(cx, "LaunchPrereq.logs.openedDeveloperSettings");
                let waiting_log = async_i18n_text(cx, "LaunchPrereq.logs.waitingUserAction");
                let _ = cx.update_global(|state: &mut LaunchPrereqState, _cx| {
                    if !state.request_matches(context.request_id) {
                        return;
                    }
                    state.operation = None;
                    state.progress_percent = None;
                    state.progress_stage = SharedString::default();
                    state.progress_target = None;
                    state.error_message = None;
                    let _ = state.push_log_if_matches(context.request_id, opened_log);
                    let _ = state.push_log_if_matches(context.request_id, waiting_log);
                });
            }
            Ok(Err(error)) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "打开开发者模式设置页失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.openSettingsFailed",
                    &error.to_string(),
                    cx,
                );
            }
            Err(error) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "开发者模式设置页任务执行失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.openSettingsFailed",
                    &error.to_string(),
                    cx,
                );
            }
        }

        Ok::<(), Error>(())
    })
    .detach();
}

pub fn enable_launch_prereq_developer_mode(cx: &mut App) {
    let context = read_active_launch_prereq(cx);
    let Some(context) = context else {
        return;
    };
    if context.busy {
        return;
    }

    let start_log = cx
        .global::<I18n>()
        .t("LaunchPrereq.logs.enableDeveloperMode");
    cx.update_global(|state: &mut LaunchPrereqState, _cx| {
        if state.set_operation_if_matches(
            context.request_id,
            LaunchPrereqOperation::EnablingDeveloperMode,
        ) {
            let _ = state.push_log_if_matches(context.request_id, start_log);
        }
    });
    info!(
        request_id = context.request_id,
        version_name = %context.version.name,
        "开始尝试启用开发者模式"
    );

    cx.spawn(async move |cx| {
        let result = tokio::task::spawn_blocking(developer_mode::try_enable_developer_mode).await;
        match result {
            Ok(Ok(())) => {
                info!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    "开发者模式启用成功，准备重新检查依赖"
                );
                if let Err(error) =
                    schedule_launch_prereq_check(context.version, context.request_id, cx)
                {
                    warn!("schedule developer mode recheck failed: {error:?}");
                }
            }
            Ok(Err(DeveloperModeError::AccessDenied)) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    "启用开发者模式需要管理员权限，改为引导用户手动开启"
                );
                let admin_notice = async_i18n_text(cx, "LaunchPrereq.adminRunRequired");
                let manual_toast =
                    async_i18n_text(cx, "LaunchPrereq.issueDeveloperMode.manualToast");
                let _ = cx.update_global(|state: &mut LaunchPrereqState, _cx| {
                    if !state.request_matches(context.request_id) {
                        return;
                    }
                    state.operation = None;
                    state.progress_percent = None;
                    state.progress_stage = SharedString::default();
                    state.progress_target = None;
                    state.error_message = None;
                    state.admin_notice = Some(admin_notice);
                });

                match tokio::task::spawn_blocking(developer_mode::open_developer_settings).await {
                    Ok(Ok(())) => {
                        info!(
                            request_id = context.request_id,
                            version_name = %context.version.name,
                            "已打开开发者模式设置页"
                        );
                        toast::push_async(cx, toast::ToastKind::Info, manual_toast);
                    }
                    Ok(Err(error)) => {
                        apply_launch_prereq_failure(
                            context.request_id,
                            "LaunchPrereq.errors.openSettingsFailed",
                            &error.to_string(),
                            cx,
                        );
                    }
                    Err(error) => {
                        apply_launch_prereq_failure(
                            context.request_id,
                            "LaunchPrereq.errors.openSettingsFailed",
                            &error.to_string(),
                            cx,
                        );
                    }
                }
            }
            Ok(Err(error)) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "启用开发者模式失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.enableDeveloperModeFailed",
                    &error.to_string(),
                    cx,
                );
            }
            Err(error) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "启用开发者模式任务执行失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.enableDeveloperModeFailed",
                    &error.to_string(),
                    cx,
                );
            }
        }

        Ok::<(), Error>(())
    })
    .detach();
}

pub fn install_launch_prereq_uwp_dependencies(cx: &mut App) {
    let context = read_active_launch_prereq(cx);
    let Some(context) = context else {
        return;
    };
    if context.busy {
        return;
    }

    let Some(check) = context.check.clone() else {
        return;
    };
    if check.missing_uwp_dependencies.is_empty() {
        return;
    }

    let start_log = cx
        .global::<I18n>()
        .t("LaunchPrereq.logs.installUwpDependencies");
    cx.update_global(|state: &mut LaunchPrereqState, _cx| {
        if state.set_operation_if_matches(
            context.request_id,
            LaunchPrereqOperation::InstallingUwpDependencies,
        ) {
            let _ = state.push_log_if_matches(context.request_id, start_log);
        }
    });
    info!(
        request_id = context.request_id,
        version_name = %context.version.name,
        missing_count = check.missing_uwp_dependencies.len(),
        dependencies = ?check
            .missing_uwp_dependencies
            .iter()
            .map(|dependency| dependency.name.clone())
            .collect::<Vec<_>>(),
        "开始安装缺失的 UWP 依赖"
    );

    let (sender, receiver) = unbounded_channel();
    spawn_dependency_event_pump(context.request_id, receiver, cx);

    cx.spawn(async move |cx| {
        let result = tokio::spawn(async move {
            mc_dependency::install_missing_uwp_dependencies(
                context.locale,
                check.missing_uwp_dependencies,
                Some(sender),
            )
            .await
        })
        .await;

        match result {
            Ok(Ok(())) => {
                info!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    "UWP 依赖安装完成，准备重新检查"
                );
                if let Err(error) =
                    schedule_launch_prereq_check(context.version, context.request_id, cx)
                {
                    warn!("schedule UWP dependency recheck failed: {error:?}");
                }
            }
            Ok(Err(error)) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "UWP 依赖安装失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.installUwpFailed",
                    &error.to_string(),
                    cx,
                );
            }
            Err(error) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "UWP 依赖安装任务执行失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.installUwpFailed",
                    &error.to_string(),
                    cx,
                );
            }
        }

        Ok::<(), Error>(())
    })
    .detach();
}

pub fn install_launch_prereq_game_input(cx: &mut App) {
    let context = read_active_launch_prereq(cx);
    let Some(context) = context else {
        return;
    };
    if context.busy {
        return;
    }

    let Some(plan) = context
        .check
        .as_ref()
        .and_then(|check| check.game_input_plan.clone())
    else {
        return;
    };

    let start_log = cx.global::<I18n>().t("LaunchPrereq.logs.installGameInput");
    cx.update_global(|state: &mut LaunchPrereqState, _cx| {
        if state.set_operation_if_matches(
            context.request_id,
            LaunchPrereqOperation::InstallingGameInput,
        ) {
            let _ = state.push_log_if_matches(context.request_id, start_log);
        }
    });
    info!(
        request_id = context.request_id,
        version_name = %context.version.name,
        installer_path = %plan.installer_path.display(),
        installer_source = ?plan.source,
        "开始安装 GameInput Runtime"
    );

    let (sender, receiver) = unbounded_channel();
    spawn_dependency_event_pump(context.request_id, receiver, cx);

    cx.spawn(async move |cx| {
        let result = tokio::spawn(async move {
            mc_dependency::install_game_input_runtime(context.locale, plan, Some(sender)).await
        })
        .await;

        match result {
            Ok(Ok(())) => {
                info!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    "GameInput Runtime 安装完成，准备重新检查"
                );
                if let Err(error) =
                    schedule_launch_prereq_check(context.version, context.request_id, cx)
                {
                    warn!("schedule GameInput recheck failed: {error:?}");
                }
            }
            Ok(Err(error)) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "GameInput Runtime 安装失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.installGameInputFailed",
                    &error.to_string(),
                    cx,
                );
            }
            Err(error) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "GameInput Runtime 安装任务执行失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.installGameInputFailed",
                    &error.to_string(),
                    cx,
                );
            }
        }

        Ok::<(), Error>(())
    })
    .detach();
}

pub fn install_launch_prereq_windows_app_sdk(cx: &mut App) {
    let context = read_active_launch_prereq(cx);
    let Some(context) = context else {
        return;
    };
    if context.busy {
        return;
    }

    let Some(plan) = context
        .check
        .as_ref()
        .and_then(|check| check.windows_app_sdk_plan.clone())
    else {
        return;
    };

    let start_log = cx
        .global::<I18n>()
        .t("LaunchPrereq.logs.installWindowsAppSdk");
    cx.update_global(|state: &mut LaunchPrereqState, _cx| {
        if state.set_operation_if_matches(
            context.request_id,
            LaunchPrereqOperation::InstallingWindowsAppSdk,
        ) {
            let _ = state.push_log_if_matches(context.request_id, start_log);
        }
    });
    info!(
        request_id = context.request_id,
        version_name = %context.version.name,
        installer_path = %plan.installer_path.display(),
        installer_source = ?plan.source,
        version = plan.version_label,
        "开始安装 Windows App SDK Runtime"
    );

    let (sender, receiver) = unbounded_channel();
    spawn_dependency_event_pump(context.request_id, receiver, cx);

    cx.spawn(async move |cx| {
        let result = tokio::spawn(async move {
            mc_dependency::install_windows_app_sdk_runtime(context.locale, plan, Some(sender)).await
        })
        .await;

        match result {
            Ok(Ok(())) => {
                info!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    "Windows App SDK Runtime 安装完成，准备重新检查"
                );
                if let Err(error) =
                    schedule_launch_prereq_check(context.version, context.request_id, cx)
                {
                    warn!("schedule Windows App SDK recheck failed: {error:?}");
                }
            }
            Ok(Err(error)) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "Windows App SDK Runtime 安装失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.installWindowsAppSdkFailed",
                    &error.to_string(),
                    cx,
                );
            }
            Err(error) => {
                warn!(
                    request_id = context.request_id,
                    version_name = %context.version.name,
                    error = %error,
                    "Windows App SDK Runtime 安装任务执行失败"
                );
                apply_launch_prereq_failure(
                    context.request_id,
                    "LaunchPrereq.errors.installWindowsAppSdkFailed",
                    &error.to_string(),
                    cx,
                );
            }
        }

        Ok::<(), Error>(())
    })
    .detach();
}

pub fn sync_launcher_state(now: std::time::Instant, cx: &mut App) {
    let should_finish_close = {
        let state = cx.global::<LauncherState>();
        state.show_modal && !state.modal_visible && !state.is_modal_animating(now)
    };
    if should_finish_close {
        cx.update_global(|state: &mut LauncherState, _cx| {
            state.finish_close_if_elapsed(now);
        });
    }
}

fn begin_launch_task(version: LaunchVersionDescriptor, cx: &mut App) -> Arc<str> {
    let request = LaunchRequest::new(
        version.folder.to_string(),
        version.name.to_string(),
        version.version.to_string(),
        version.path.to_string(),
    );
    let request = LaunchRequest {
        launch_args: version
            .launch_args
            .as_ref()
            .map(|value| Arc::from(value.as_ref())),
        ..request
    };
    let task_id = start_launch_task(request);
    let task_id_arc: Arc<str> = Arc::from(task_id.as_str());
    info!(
        task_id = %task_id_arc,
        version_name = %version.name,
        version = %version.version,
        kind = %version.kind,
        package_path = %version.path,
        "启动前检查已通过，开始创建正式启动任务"
    );
    let now = std::time::Instant::now();
    let loader_version = embedded_dll_version_string()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from("unknown"));

    cx.update_global(|state: &mut LauncherState, _cx| {
        state.begin(
            task_id_arc.clone(),
            version.folder,
            version.name,
            version.version,
            version.kind,
            version.path,
            version.launch_args,
            loader_version,
            now,
        );
    });

    spawn_launcher_snapshot_pump(task_id_arc.clone(), cx);
    task_id_arc
}

fn pending_launch_version(version: &LaunchVersionDescriptor) -> PendingLaunchVersion {
    PendingLaunchVersion {
        folder: version.folder.clone(),
        name: version.name.clone(),
        version: version.version.clone(),
        kind: version.kind.clone(),
        path: version.path.clone(),
        launch_args: version.launch_args.clone(),
    }
}

fn launch_version_from_pending(version: &PendingLaunchVersion) -> LaunchVersionDescriptor {
    LaunchVersionDescriptor {
        folder: version.folder.clone(),
        name: version.name.clone(),
        version: version.version.clone(),
        kind: version.kind.clone(),
        path: version.path.clone(),
        launch_args: version.launch_args.clone(),
    }
}

fn launch_prereq_ticket(request_id: u64) -> Arc<str> {
    Arc::from(format!("launch-prereq-{request_id}"))
}

fn read_active_launch_prereq(cx: &App) -> Option<ActiveLaunchPrereq> {
    let locale = cx.global::<I18n>().locale();
    cx.read_global(|state: &LaunchPrereqState, _cx| {
        Some(ActiveLaunchPrereq {
            request_id: state.request_id,
            version: state.version.clone()?,
            check: state.check.clone(),
            locale,
            busy: state.is_busy(),
        })
    })
}

fn spawn_launch_prereq_check_for_request(
    version: PendingLaunchVersion,
    request_id: u64,
    cx: &mut App,
) {
    let kind = version.kind.to_string();
    let package_path = version.path.to_string();
    debug!(
        request_id,
        version_name = %version.name,
        version = %version.version,
        kind = %version.kind,
        package_path = %version.path,
        "启动前检查任务已提交"
    );

    cx.spawn(async move |cx| {
        let result =
            tokio::task::spawn_blocking(move || check_launch_prerequisites(&kind, &package_path))
                .await;

        match result {
            Ok(check) if check.has_issues() => {
                info!(
                    request_id,
                    version_name = %version.name,
                    version = %version.version,
                    platform = ?check.platform,
                    developer_mode_required = check.developer_mode_required,
                    missing_uwp_dependencies = check.missing_uwp_dependencies.len(),
                    game_input_required = check.game_input_plan.is_some(),
                    windows_app_sdk_required = check.windows_app_sdk_plan.is_some(),
                    "启动前检查发现缺失依赖或环境项"
                );
                let issue_logs = build_issue_logs(&check, cx);
                let _ = cx.update_global(|state: &mut LaunchPrereqState, _cx| {
                    if state.set_check_if_matches(request_id, check) {
                        for line in issue_logs {
                            let _ = state.push_log_if_matches(request_id, line);
                        }
                    }
                });
            }
            Ok(check) => {
                info!(
                    request_id,
                    version_name = %version.name,
                    version = %version.version,
                    platform = ?check.platform,
                    "启动前检查通过，进入正式启动"
                );
                if let Err(error) = finish_launch_prereq_and_start_launch(version, request_id, cx) {
                    warn!("finish launch prereq failed: {error:?}");
                }
            }
            Err(error) => {
                warn!(
                    request_id,
                    version_name = %version.name,
                    version = %version.version,
                    error = %error,
                    "启动前检查任务执行失败"
                );
                apply_launch_prereq_failure(
                    request_id,
                    "LaunchPrereq.errors.checkFailed",
                    &error.to_string(),
                    cx,
                );
            }
        }

        Ok::<(), Error>(())
    })
    .detach();
}

fn finish_launch_prereq_and_start_launch(
    version: PendingLaunchVersion,
    request_id: u64,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    cx.update(|cx| {
        let should_start =
            cx.read_global(|state: &LaunchPrereqState, _cx| state.request_matches(request_id));
        if !should_start {
            debug!(
                request_id,
                version_name = %version.name,
                "启动前检查结果已过期，本次不会继续启动"
            );
            return;
        }

        info!(
            request_id,
            version_name = %version.name,
            version = %version.version,
            kind = %version.kind,
            "清理启动前检查覆盖层并进入正式启动任务"
        );
        cx.update_global(|state: &mut LaunchPrereqState, _cx| {
            let _ = state.clear_if_matches(request_id);
        });
        begin_launch_task(launch_version_from_pending(&version), cx);
    })?;
    Ok(())
}

fn schedule_launch_prereq_check(
    version: PendingLaunchVersion,
    request_id: u64,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    cx.update(|cx| {
        debug!(
            request_id,
            version_name = %version.name,
            version = %version.version,
            "准备重新执行启动前检查"
        );
        cx.update_global(|state: &mut LaunchPrereqState, _cx| {
            let _ = state.set_operation_if_matches(request_id, LaunchPrereqOperation::Checking);
        });
        spawn_launch_prereq_check_for_request(version, request_id, cx);
    })?;
    Ok(())
}

fn spawn_dependency_event_pump(
    request_id: u64,
    mut receiver: UnboundedReceiver<DependencyEvent>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        while let Some(event) = receiver.recv().await {
            let mut batch = DependencyEventBatch::default();
            batch.push(event);

            Timer::after(DEPENDENCY_EVENT_BATCH_DELAY).await;

            loop {
                match receiver.try_recv() {
                    Ok(event) => batch.push(event),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }

            let log_count = batch.logs.len();
            let has_progress = batch.progress.is_some();
            let has_admin_notice = batch.admin_notice.is_some();
            let applied = cx
                .update_global(|state: &mut LaunchPrereqState, _cx| batch.apply(request_id, state))
                .unwrap_or(false);

            if applied {
                debug!(
                    request_id,
                    log_count, has_progress, has_admin_notice, "启动依赖事件批量回灌完成"
                );
            }
        }

        Ok::<(), Error>(())
    })
    .detach();
}

fn apply_launch_prereq_failure(request_id: u64, key: &str, message: &str, cx: &mut AsyncApp) {
    warn!(
        request_id,
        error_key = key,
        error_message = message,
        "启动前检查流程失败"
    );
    let localized_message = async_i18n_text_args(cx, key, message);
    let admin_notice = async_i18n_text(cx, "LaunchPrereq.adminRunRequired");
    let requires_admin = requires_admin_notice(message);

    if let Err(error) = cx.update_global(|state: &mut LaunchPrereqState, _cx| {
        if requires_admin {
            let _ = state.set_admin_notice_if_matches(request_id, admin_notice);
        }
        let _ = state.set_error_if_matches(request_id, localized_message);
    }) {
        warn!("launch prereq failure update failed: {error:?}");
    }
}

fn async_i18n_text(cx: &mut AsyncApp, key: &str) -> SharedString {
    let key_string = key.to_string();
    cx.read_global({
        let key_string = key_string.clone();
        move |i18n: &I18n, _cx| i18n.t(&key_string)
    })
    .unwrap_or_else(|_| SharedString::from(key_string))
}

fn async_i18n_text_items(cx: &mut AsyncApp, key: &str, items: &str) -> SharedString {
    let key_string = key.to_string();
    let items_string = items.to_string();
    cx.read_global({
        let key_string = key_string.clone();
        let items_string = items_string.clone();
        move |i18n: &I18n, _cx| {
            i18n.t_args(&key_string, crate::i18n_args![("items", &items_string)])
        }
    })
    .unwrap_or_else(|_| SharedString::from(format!("{key_string}: {items_string}")))
}

fn async_i18n_text_required(cx: &mut AsyncApp, key: &str, required: &str) -> SharedString {
    let key_string = key.to_string();
    let required_string = required.to_string();
    cx.read_global({
        let key_string = key_string.clone();
        let required_string = required_string.clone();
        move |i18n: &I18n, _cx| {
            i18n.t_args(
                &key_string,
                crate::i18n_args![("required", &required_string)],
            )
        }
    })
    .unwrap_or_else(|_| SharedString::from(format!("{key_string}: {required_string}")))
}

fn async_i18n_text_current_required(
    cx: &mut AsyncApp,
    key: &str,
    current: &str,
    required: &str,
) -> SharedString {
    let key_string = key.to_string();
    let current_string = current.to_string();
    let required_string = required.to_string();
    cx.read_global({
        let key_string = key_string.clone();
        let current_string = current_string.clone();
        let required_string = required_string.clone();
        move |i18n: &I18n, _cx| {
            i18n.t_args(
                &key_string,
                crate::i18n_args![("current", &current_string), ("required", &required_string)],
            )
        }
    })
    .unwrap_or_else(|_| {
        SharedString::from(format!(
            "{key_string}: current={current_string}, required={required_string}"
        ))
    })
}

fn async_i18n_text_path(cx: &mut AsyncApp, key: &str, path: &str) -> SharedString {
    let key_string = key.to_string();
    let path_string = path.to_string();
    cx.read_global({
        let key_string = key_string.clone();
        let path_string = path_string.clone();
        move |i18n: &I18n, _cx| i18n.t_args(&key_string, crate::i18n_args![("path", &path_string)])
    })
    .unwrap_or_else(|_| SharedString::from(format!("{key_string}: {path_string}")))
}

fn async_i18n_text_url(cx: &mut AsyncApp, key: &str, url: &str) -> SharedString {
    let key_string = key.to_string();
    let url_string = url.to_string();
    cx.read_global({
        let key_string = key_string.clone();
        let url_string = url_string.clone();
        move |i18n: &I18n, _cx| i18n.t_args(&key_string, crate::i18n_args![("url", &url_string)])
    })
    .unwrap_or_else(|_| SharedString::from(format!("{key_string}: {url_string}")))
}

fn async_i18n_text_args(cx: &mut AsyncApp, key: &str, message: &str) -> SharedString {
    let key_string = key.to_string();
    let message_string = message.to_string();
    cx.read_global({
        let key_string = key_string.clone();
        let message_string = message_string.clone();
        move |i18n: &I18n, _cx| {
            i18n.t_args(&key_string, crate::i18n_args![("message", &message_string)])
        }
    })
    .unwrap_or_else(|_| SharedString::from(format!("{key_string}: {message_string}")))
}

fn build_issue_logs(check: &LaunchPrerequisiteCheck, cx: &mut AsyncApp) -> Vec<SharedString> {
    let mut logs = Vec::new();

    if check.developer_mode_required {
        logs.push(async_i18n_text(
            cx,
            "LaunchPrereq.logs.developerModeRequired",
        ));
    }

    if !check.missing_uwp_dependencies.is_empty() {
        let issue_items = format_uwp_dependency_issue_list(check, cx);
        logs.push(async_i18n_text_items(
            cx,
            "LaunchPrereq.logs.missingUwpDependencies",
            &issue_items,
        ));
    }

    if let Some(plan) = check.game_input_plan.as_ref() {
        match plan.source {
            mc_dependency::GameInputInstallerSource::Local => {
                logs.push(async_i18n_text_path(
                    cx,
                    "LaunchPrereq.logs.missingGameInputLocal",
                    &plan.installer_path.display().to_string(),
                ));
            }
            mc_dependency::GameInputInstallerSource::Download => {
                logs.push(async_i18n_text_url(
                    cx,
                    "LaunchPrereq.logs.missingGameInputDownload",
                    mc_dependency::GAMEINPUT_RELEASES_URL,
                ));
            }
        }
    }

    if let Some(plan) = check.windows_app_sdk_plan.as_ref() {
        match plan.source {
            mc_dependency::WindowsAppSdkInstallerSource::Local => {
                logs.push(async_i18n_text_path(
                    cx,
                    "LaunchPrereq.logs.missingWindowsAppSdkLocal",
                    &plan.installer_path.display().to_string(),
                ));
            }
            mc_dependency::WindowsAppSdkInstallerSource::Download => {
                logs.push(async_i18n_text_url(
                    cx,
                    "LaunchPrereq.logs.missingWindowsAppSdkDownload",
                    mc_dependency::WINDOWS_APP_SDK_RELEASES_URL,
                ));
            }
        }
    }

    logs.push(async_i18n_text(cx, "LaunchPrereq.logs.waitingUserAction"));
    logs
}

fn format_uwp_dependency_issue_list(check: &LaunchPrerequisiteCheck, cx: &mut AsyncApp) -> String {
    check
        .missing_uwp_dependencies
        .iter()
        .map(|dependency| format_uwp_dependency_issue_label(dependency, cx))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_uwp_dependency_issue_label(
    dependency: &mc_dependency::MissingUwpDependency,
    cx: &mut AsyncApp,
) -> String {
    let reason = match &dependency.issue_kind {
        mc_dependency::UwpDependencyIssueKind::Missing => {
            async_i18n_text(cx, "LaunchPrereq.issueUwpDependencies.reasonMissing")
        }
        mc_dependency::UwpDependencyIssueKind::VersionMismatch {
            installed_version: Some(installed_version),
            required_version,
        } => async_i18n_text_current_required(
            cx,
            "LaunchPrereq.issueUwpDependencies.reasonVersionMismatch",
            installed_version,
            required_version,
        ),
        mc_dependency::UwpDependencyIssueKind::VersionMismatch {
            installed_version: None,
            required_version,
        } => async_i18n_text_required(
            cx,
            "LaunchPrereq.issueUwpDependencies.reasonVersionUnknown",
            required_version,
        ),
    };

    format!("{} ({reason})", dependency.name)
}

fn requires_admin_notice(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("access denied")
        || lower.contains("requires administrator")
        || lower.contains("permission denied")
        || lower.contains("0x80070005")
        || message.contains("权限")
        || message.contains("拒绝")
        || message.contains("管理员")
}

fn spawn_launcher_snapshot_pump(task_id: Arc<str>, cx: &mut App) {
    if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref()) {
        cx.update_global(|state: &mut LauncherState, _cx| {
            if state.task_id.as_deref() == Some(task_id.as_ref()) {
                state.apply_snapshot(snapshot);
            }
        });
    }

    cx.spawn({
        let task_id = task_id.clone();
        async move |cx| {
            let mut updates = task_manager::subscribe_task_updates();
            loop {
                let snapshot = match updates.recv().await {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        warn!("launcher snapshot pump closed: {error}");
                        break;
                    }
                };
                if snapshot.id.as_ref() != task_id.as_ref() {
                    continue;
                }

                let terminal = matches!(
                    snapshot.status.as_ref(),
                    "completed" | "cancelled" | "error"
                );
                let snapshot_clone = snapshot.clone();
                if let Err(error) = cx.update_global(|state: &mut LauncherState, _cx| {
                    if state.task_id.as_deref() == Some(task_id.as_ref()) {
                        state.apply_snapshot(snapshot_clone);
                    }
                }) {
                    warn!("launcher snapshot update failed: {error:?}");
                    break;
                }

                if terminal {
                    if snapshot.status.as_ref() == "completed" {
                        tokio::time::sleep(Duration::from_millis(900)).await;
                        let now = std::time::Instant::now();
                        let _ = cx.update_global(|state: &mut LauncherState, _cx| {
                            if state.task_id.as_deref() == Some(task_id.as_ref()) {
                                state.request_close(now);
                            }
                        });
                    }
                    break;
                }
            }
            Ok::<(), Error>(())
        }
    })
    .detach();
}

fn launcher_snapshot_changed(previous: &LauncherSnapshot, current: &LauncherSnapshot) -> bool {
    previous.show_modal != current.show_modal
        || previous.modal_visible != current.modal_visible
        || previous.modal_animating != current.modal_animating
        || (previous.modal_factor - current.modal_factor).abs() > 0.001
        || previous.task_id != current.task_id
        || previous.version_folder != current.version_folder
        || previous.version_name != current.version_name
        || previous.version != current.version
        || previous.kind != current.kind
        || previous.package_path != current.package_path
        || previous.loader_version != current.loader_version
        || previous
            .last_snapshot
            .as_ref()
            .map(|snapshot| snapshot.sequence)
            != current
                .last_snapshot
                .as_ref()
                .map(|snapshot| snapshot.sequence)
        || previous.log_version != current.log_version
}
