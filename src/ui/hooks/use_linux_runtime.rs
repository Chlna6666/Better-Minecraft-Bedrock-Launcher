use crate::core::linux_runtime::{check_linux_runtime, start_linux_runtime_install};
use crate::tasks::runtime::{BlockingTaskOptions, run_blocking};
use crate::tasks::task_manager::{self, TaskSnapshot};
use crate::ui::state::linux_runtime::{LinuxRuntimeState, LinuxRuntimeStatus};
use anyhow::Error;
use gpui::{App, AsyncApp, BorrowAppContext as _};
use std::sync::Arc;
use tracing::{info, warn};

pub fn start_startup_check(cx: &mut App) {
    let request_id = cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
        if state.check_started {
            None
        } else {
            state.begin_check(false)
        }
    });
    let Some(request_id) = request_id else {
        return;
    };
    spawn_check(request_id, cx);
}

pub fn recheck(cx: &mut App) {
    let request_id = cx.update_global(|state: &mut LinuxRuntimeState, _cx| state.begin_check(true));
    if let Some(request_id) = request_id {
        spawn_check(request_id, cx);
    }
}

pub fn dismiss(cx: &mut App) {
    cx.update_global(|state: &mut LinuxRuntimeState, _cx| state.dismiss());
}

pub fn open_proton_gdk_settings(cx: &mut App) {
    dismiss(cx);
    cx.update_global(
        |state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
            state.tab = crate::ui::views::settings::state::SettingsTab::ProtonGdk;
        },
    );
    crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Settings);
}

pub fn authorize_and_install(cx: &mut App) {
    let install = cx.update_global(|state: &mut LinuxRuntimeState, _cx| state.begin_install());
    let Some((request_id, plan)) = install else {
        return;
    };

    let updates = task_manager::subscribe_task_updates();
    let task_id: Arc<str> = Arc::from(start_linux_runtime_install(plan));
    cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
        state.attach_install_task(request_id, task_id.clone())
    });
    spawn_install_snapshot_pump(request_id, task_id, updates, cx);
}

fn spawn_install_snapshot_pump(
    request_id: u64,
    task_id: Arc<str>,
    mut updates: tokio::sync::broadcast::Receiver<Arc<TaskSnapshot>>,
    cx: &mut App,
) {
    if let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref()) {
        cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
            state.apply_install_snapshot(request_id, snapshot)
        });
    }

    cx.spawn(async move |cx| {
        loop {
            let snapshot = match updates.recv().await {
                Ok(snapshot) => snapshot,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(request_id, skipped, "Linux install progress updates lagged");
                    let Some(snapshot) = task_manager::get_snapshot_arc(task_id.as_ref()) else {
                        continue;
                    };
                    snapshot
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                        state.set_install_error(request_id, "安装进度通道已关闭，请重新检测。")
                    })?;
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
            let completed = snapshot.status.as_ref() == "completed";
            let terminal_message = snapshot.message.clone();
            cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                state.apply_install_snapshot(request_id, snapshot)
            })?;
            if !terminal {
                continue;
            }
            if completed {
                verify_installed_runtime(request_id, cx).await?;
            } else {
                let message = terminal_message
                    .map(|message| message.to_string())
                    .unwrap_or_else(|| "兼容环境安装失败，请查看安装输出。".to_string());
                cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                    state.set_install_error(request_id, message)
                })?;
            }
            break;
        }
        Ok::<(), Error>(())
    })
    .detach();
}

async fn verify_installed_runtime(request_id: u64, cx: &mut AsyncApp) -> Result<(), Error> {
    let check = run_blocking(
        BlockingTaskOptions::hidden("重新检测 Linux 兼容环境"),
        || Ok(check_linux_runtime()),
    )
    .await;
    match check {
        Ok(check) if check.is_ready() => {
            let runner = check.runner.as_ref().map(|runner| {
                format!(
                    "{} ({})",
                    runner.kind.display_name(),
                    runner.executable.display()
                )
            });
            info!(request_id, ?runner, "Linux compatibility runtime is ready");
            cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                state.apply_check(request_id, check)
            })?;
        }
        Ok(check) => {
            cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                state.check = Some(check);
                state.set_install_error(
                    request_id,
                    "安装已结束，但仍未检测到 Proton 或 Wine。请查看任务日志后重新检测。",
                )
            })?;
        }
        Err(error) => {
            cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                state.set_install_error(request_id, error)
            })?;
        }
    }
    Ok(())
}

fn spawn_check(request_id: u64, cx: &mut App) {
    cx.spawn(async move |cx| {
        let check = run_blocking(
            BlockingTaskOptions::hidden("检测 Linux 兼容环境"),
            || Ok(check_linux_runtime()),
        )
        .await;
        match check {
            Ok(check) => {
                if let Some(runner) = check.runner.as_ref() {
                    info!(
                        runner = runner.kind.display_name(),
                        executable = %runner.executable.display(),
                        "Linux compatibility runtime detected"
                    );
                }
                cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                    state.apply_check(request_id, check)
                })?;
            }
            Err(error) => {
                warn!(request_id, %error, "Linux runtime startup check failed");
                cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                    state.set_check_error(request_id, error)
                })?;
            }
        }
        Ok::<(), Error>(())
    })
    .detach();
}

pub fn can_authorize_install(state: &LinuxRuntimeState) -> bool {
    matches!(
        state.status,
        LinuxRuntimeStatus::Missing | LinuxRuntimeStatus::Error
    ) && state
        .check
        .as_ref()
        .and_then(|check| check.install_plan.as_ref())
        .is_some()
}
