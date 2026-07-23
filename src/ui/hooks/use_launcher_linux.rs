use crate::core::linux_runtime::check_linux_runtime;
use crate::core::minecraft::launcher::{LaunchRequest, start_launch_task};
use crate::tasks::task_manager::{self, TaskSnapshot};
use crate::ui::state::launcher::LauncherState;
use anyhow::Error;
use gpui::{
    App, AppContext as _, BorrowAppContext as _, ClipboardItem, Context, SharedString,
    Subscription, Timer,
};
use gpui_hooks::hooks::{UseRefHook, UseStateHook};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

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

pub fn read_launcher_snapshot(now: std::time::Instant, cx: &App) -> LauncherSnapshot {
    cx.read_global(|state: &LauncherState, _cx| {
        let logs = state
            .task_id
            .as_ref()
            .map(|task_id| task_manager::task_logs(task_id.as_ref()))
            .unwrap_or_else(|| Arc::<[Arc<str>]>::from([]));
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
            let current = read_launcher_snapshot(std::time::Instant::now(), cx);
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
        warn!(version_name = %version.name, "Linux launch request has no package path");
        return None;
    }
    if cx.read_global(|state: &LauncherState, _cx| state.launch_in_progress()) {
        debug!(version_name = %version.name, "ignored duplicate Linux launch request");
        return None;
    }

    let runtime_check = check_linux_runtime();
    if !runtime_check.is_ready() {
        cx.update_global(
            |state: &mut crate::ui::state::linux_runtime::LinuxRuntimeState, _cx| {
                if let Some(request_id) = state.begin_check(true) {
                    state.apply_check(request_id, runtime_check);
                }
            },
        );
        return None;
    }

    Some(begin_launch_task(version, cx))
}

pub fn close_launcher(cx: &mut App) {
    let now = std::time::Instant::now();
    cx.update_global(|state: &mut LauncherState, _cx| state.request_close(now));
}

pub fn minimize_launcher(cx: &mut App) {
    cx.update_global(|state: &mut LauncherState, _cx| state.dismiss_modal());
}

pub fn cancel_launcher(cx: &mut App) {
    let task_id = cx
        .global::<LauncherState>()
        .task_id
        .as_ref()
        .map(ToString::to_string);
    if let Some(task_id) = task_id {
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
        let failed = state
            .last_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.status.as_ref() == "error");
        if !failed || state.package_path.is_empty() {
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
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                Some(format!("{message}\n\n—— 启动日志 ——\n{joined}"))
            }
            (Some(message), true) => Some(message),
            (None, false) => Some(
                logs.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            (None, true) => None,
        }
    });
    let has_text = copy_text.as_ref().is_some_and(|text| !text.is_empty());
    debug!(
        has_text,
        text_len = copy_text.as_ref().map(|text| text.len()).unwrap_or(0),
        "copy_launcher_error invoked"
    );
    if let Some(text) = copy_text.filter(|text| !text.is_empty()) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        return true;
    }
    false
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
    let request = LaunchRequest {
        launch_args: version
            .launch_args
            .as_ref()
            .map(|value| Arc::from(value.as_ref())),
        ..LaunchRequest::new(
            version.folder.to_string(),
            version.name.to_string(),
            version.version.to_string(),
            version.path.to_string(),
        )
    };
    let task_id: Arc<str> = Arc::from(start_launch_task(request));
    info!(task_id = %task_id, version_name = %version.name, "created Linux Proton launch task");

    cx.update_global(|state: &mut LauncherState, _cx| {
        state.begin(
            task_id.clone(),
            version.folder,
            version.name,
            version.version,
            version.kind,
            version.path,
            version.launch_args,
            SharedString::from("Proton / Wine"),
            std::time::Instant::now(),
        );
    });
    spawn_launcher_snapshot_pump(task_id.clone(), cx);
    task_id
}

fn spawn_launcher_snapshot_pump(task_id: Arc<str>, cx: &mut App) {
    // Subscribe before reading the current snapshot so a fast task cannot
    // finish in the gap between the initial read and receiver creation.
    let mut updates = task_manager::subscribe_task_updates();
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
            loop {
                let snapshot = match updates.recv().await {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        warn!(%error, "Linux launcher snapshot pump closed");
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
                let snapshot_for_state = snapshot.clone();
                cx.update_global(|state: &mut LauncherState, _cx| {
                    if state.task_id.as_deref() == Some(task_id.as_ref()) {
                        state.apply_snapshot(snapshot_for_state);
                    }
                })?;
                if snapshot.status.as_ref() == "running"
                    && snapshot.stage.as_ref() == "running_game"
                {
                    Timer::after(Duration::from_millis(900)).await;
                    let now = std::time::Instant::now();
                    cx.update_global(|state: &mut LauncherState, _cx| {
                        if state.task_id.as_deref() == Some(task_id.as_ref()) {
                            state.request_close(now);
                        }
                    })?;
                }
                if terminal {
                    if snapshot.status.as_ref() == "completed" {
                        Timer::after(Duration::from_millis(900)).await;
                        let now = std::time::Instant::now();
                        cx.update_global(|state: &mut LauncherState, _cx| {
                            if state.task_id.as_deref() == Some(task_id.as_ref()) {
                                state.request_close(now);
                            }
                        })?;
                    }
                    break;
                }
            }
            Ok::<(), Error>(())
        }
    })
    .detach();
}

fn launcher_log_version(logs: &[Arc<str>], last_snapshot: Option<&TaskSnapshot>) -> u64 {
    let mut version = logs.len() as u64;
    if let Some(last) = logs.last() {
        let mut hash = 0xcbf29ce484222325_u64;
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
