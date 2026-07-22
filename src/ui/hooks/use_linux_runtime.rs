use crate::core::linux_runtime::{check_linux_runtime, install_linux_runtime};
use crate::tasks::runtime::{BlockingTaskOptions, run_blocking};
use crate::ui::state::linux_runtime::{LinuxRuntimeState, LinuxRuntimeStatus};
use anyhow::Error;
use gpui::{App, BorrowAppContext as _};
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

pub fn authorize_and_install(cx: &mut App) {
    let install = cx.update_global(|state: &mut LinuxRuntimeState, _cx| state.begin_install());
    let Some((request_id, plan)) = install else {
        return;
    };

    cx.spawn(async move |cx| {
        if let Err(error) = install_linux_runtime(plan).await {
            warn!(request_id, %error, "Linux runtime installation failed");
            cx.update_global(|state: &mut LinuxRuntimeState, _cx| {
                state.set_install_error(request_id, error)
            })?;
            return Ok::<(), Error>(());
        }

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
        Ok::<(), Error>(())
    })
    .detach();
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
