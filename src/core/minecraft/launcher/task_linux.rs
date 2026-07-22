use crate::core::linux_runtime::{RunnerKind, resolve_runner};
use crate::tasks::task_manager::{
    append_task_log, create_task_with_details, finish_task, register_task_abort_handle,
    register_task_stage_labels, set_total, update_progress,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tracing::{error, info, warn};

const LAUNCH_TOTAL_STEPS: u64 = 3;
const LAUNCHER_TASK_STAGE_LABELS: [(&str, &str); 3] = [
    ("resolving_runner", "检测兼容环境"),
    ("preparing_prefix", "准备 Proton Prefix"),
    ("launching", "启动游戏"),
];

#[derive(Clone, Debug)]
pub struct LaunchRequest {
    pub folder_name: Arc<str>,
    pub display_name: Arc<str>,
    pub version: Arc<str>,
    pub package_folder: Arc<str>,
    pub auto_start: bool,
    pub launch_args: Option<Arc<str>>,
}

impl LaunchRequest {
    pub fn new(
        folder_name: impl Into<String>,
        display_name: impl Into<String>,
        version: impl Into<String>,
        package_folder: impl Into<String>,
    ) -> Self {
        Self {
            folder_name: Arc::from(folder_name.into()),
            display_name: Arc::from(display_name.into()),
            version: Arc::from(version.into()),
            package_folder: Arc::from(package_folder.into()),
            auto_start: true,
            launch_args: None,
        }
    }
}

pub fn start_launch_task(request: LaunchRequest) -> String {
    register_task_stage_labels(LAUNCHER_TASK_STAGE_LABELS);
    let task_id = create_task_with_details(
        None,
        format!("启动 {}", request.display_name),
        Some(request.version.to_string()),
        "resolving_runner",
        Some(LAUNCH_TOTAL_STEPS),
        false,
    );
    set_total(&task_id, Some(LAUNCH_TOTAL_STEPS));
    append_task_log(&task_id, format!("准备启动 {}", request.display_name));

    let task_id_for_task = task_id.clone();
    let join_handle = tokio::spawn(async move {
        match launch_game(&request, &task_id_for_task).await {
            Ok(Some(process_id)) => {
                append_task_log(
                    &task_id_for_task,
                    format!("游戏进程已启动，PID {process_id}"),
                );
                finish_task(&task_id_for_task, "completed", Some("启动完成".to_string()));
            }
            Ok(None) => {
                finish_task(&task_id_for_task, "completed", Some("准备完成".to_string()));
            }
            Err(error) => {
                error!(task_id = %task_id_for_task, %error, "Linux game launch failed");
                append_task_log(&task_id_for_task, format!("启动失败：{error}"));
                finish_task(&task_id_for_task, "error", Some(error));
            }
        }
    });
    register_task_abort_handle(task_id.clone(), join_handle.abort_handle());
    task_id
}

async fn launch_game(request: &LaunchRequest, task_id: &str) -> Result<Option<u32>, String> {
    let runner = tokio::task::spawn_blocking(resolve_runner)
        .await
        .map_err(|error| format!("检测 Proton/Wine 任务失败：{error}"))??;
    append_task_log(
        task_id,
        format!("使用 {:?}：{}", runner.kind, runner.executable.display()),
    );
    update_progress(
        task_id,
        1,
        Some(LAUNCH_TOTAL_STEPS),
        Some("preparing_prefix"),
    );

    let package_path = PathBuf::from(request.package_folder.as_ref());
    let game_executable =
        tokio::task::spawn_blocking(move || resolve_game_executable(&package_path))
            .await
            .map_err(|error| format!("检测游戏可执行文件任务失败：{error}"))??;
    let prefix_path = proton_prefix_path(request.folder_name.as_ref())?;
    tokio::fs::create_dir_all(&prefix_path)
        .await
        .map_err(|error| format!("无法创建兼容环境目录 {}：{error}", prefix_path.display()))?;
    append_task_log(task_id, format!("兼容环境目录：{}", prefix_path.display()));
    update_progress(task_id, 1, Some(LAUNCH_TOTAL_STEPS), Some("launching"));

    if !request.auto_start {
        append_task_log(task_id, "已完成环境准备，未请求启动游戏");
        return Ok(None);
    }

    let mut command = tokio::process::Command::new(&runner.executable);
    match runner.kind {
        RunnerKind::Proton => {
            command.arg("run");
            command.env("STEAM_COMPAT_DATA_PATH", &prefix_path);
            if let Some(steam_root) = runner.steam_root.as_ref() {
                command.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_root);
            }
        }
        RunnerKind::Wine => {
            command.env("WINEPREFIX", &prefix_path);
        }
    }
    command.arg(&game_executable);
    if let Some(argument) = request
        .launch_args
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        command.arg(argument);
    }
    if let Some(working_directory) = game_executable.parent() {
        command.current_dir(working_directory);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    info!(
        task_id,
        runner = %runner.executable.display(),
        game_executable = %game_executable.display(),
        prefix = %prefix_path.display(),
        "starting Minecraft through Linux compatibility runner"
    );
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动兼容环境 {}：{error}", runner.executable.display()))?;
    let process_id = child
        .id()
        .ok_or_else(|| "兼容环境已启动，但没有返回进程 PID".to_string())?;

    if let Some(stdout) = child.stdout.take() {
        spawn_output_pump(task_id.to_string(), stdout, false);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_output_pump(task_id.to_string(), stderr, true);
    }
    spawn_process_monitor(task_id.to_string(), child);
    update_progress(task_id, 1, Some(LAUNCH_TOTAL_STEPS), Some("launching"));
    Ok(Some(process_id))
}

fn resolve_game_executable(package_path: &Path) -> Result<PathBuf, String> {
    if package_path.is_file()
        && package_path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Ok(package_path.to_path_buf());
    }

    [
        "Minecraft.Windows.exe",
        "Minecraft.exe",
        "Content/Minecraft.Windows.exe",
        "Content/Minecraft.exe",
    ]
    .into_iter()
    .map(|relative_path| package_path.join(relative_path))
    .find(|candidate| candidate.is_file())
    .ok_or_else(|| {
        format!(
            "在 {} 中没有找到可由 Proton/Wine 启动的 Minecraft 可执行文件",
            package_path.display()
        )
    })
}

fn proton_prefix_path(folder_name: &str) -> Result<PathBuf, String> {
    Ok(crate::utils::file_ops::prefixes_dir().join(sanitize_instance_folder_name(folder_name)))
}

fn sanitize_instance_folder_name(folder_name: &str) -> String {
    let safe_folder_name: String = folder_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if safe_folder_name.is_empty() || matches!(safe_folder_name.as_str(), "." | "..") {
        "default".to_string()
    } else {
        safe_folder_name
    }
}

fn spawn_output_pump<R>(task_id: String, reader: R, is_error: bool)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let prefix = if is_error { "runner stderr" } else { "runner" };
                    append_task_log(&task_id, format!("{prefix}: {line}"));
                }
                Ok(None) => break,
                Err(error) => {
                    warn!(task_id, %error, "failed to read compatibility runner output");
                    break;
                }
            }
        }
    });
}

fn spawn_process_monitor(task_id: String, mut child: tokio::process::Child) {
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => {
                append_task_log(&task_id, format!("游戏进程已退出：{status}"));
            }
            Err(error) => {
                warn!(task_id, %error, "failed to wait for compatibility runner process");
                append_task_log(&task_id, format!("等待游戏进程失败：{error}"));
            }
        };
    });
}

#[cfg(test)]
#[path = "task_linux_tests.rs"]
mod tests;
