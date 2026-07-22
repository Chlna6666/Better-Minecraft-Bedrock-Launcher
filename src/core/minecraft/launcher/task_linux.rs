use crate::core::linux_runtime::{RunnerKind, resolve_runner, validate_proton_game_runtime};
use crate::tasks::task_manager::{
    append_task_log, create_task_with_details, finish_task, register_task_abort_handle,
    register_task_stage_labels, set_total, update_progress,
};
use std::collections::VecDeque;
use std::env;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

const LAUNCH_TOTAL_STEPS: u64 = 3;
const EARLY_EXIT_GRACE_PERIOD: Duration = Duration::from_secs(8);
const GAME_INPUT_INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const GAME_INPUT_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);
const RECENT_RUNNER_OUTPUT_LIMIT: usize = 32;
const LAUNCHER_TASK_STAGE_LABELS: [(&str, &str); 4] = [
    ("resolving_runner", "检测兼容环境"),
    ("preparing_prefix", "准备 Proton Prefix"),
    ("launching", "启动游戏"),
    ("running_game", "游戏运行中"),
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
    let package_path_for_probe = package_path.clone();
    let game_executable =
        tokio::task::spawn_blocking(move || resolve_game_executable(&package_path_for_probe))
            .await
            .map_err(|error| format!("检测游戏可执行文件任务失败：{error}"))??;
    if runner.kind == RunnerKind::Wine
        && game_executable
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("Minecraft.Windows.exe"))
    {
        return Err(
            "当前 Linux 版本是 UWP/GDK 游戏，原生 Wine 无法启动；请安装并选择 Proton runner"
                .to_string(),
        );
    }
    tokio::task::spawn_blocking({
        let runner = runner.clone();
        move || validate_proton_game_runtime(&runner)
    })
    .await
    .map_err(|error| format!("检查 Proton-GDK 兼容性任务失败：{error}"))??;
    let prefix_path = proton_prefix_path(request.folder_name.as_ref())?;
    tokio::fs::create_dir_all(&prefix_path)
        .await
        .map_err(|error| format!("无法创建兼容环境目录 {}：{error}", prefix_path.display()))?;
    append_task_log(task_id, format!("兼容环境目录：{}", prefix_path.display()));

    match runner.kind {
        RunnerKind::Proton => {
            install_proton_game_input(&runner, &prefix_path, &package_path, task_id).await?;
            stop_lingering_proton_processes(&runner, &prefix_path, task_id).await?;
        }
        RunnerKind::Wine => {
            initialize_wine_prefix(&runner.executable, &prefix_path, task_id).await?;
            install_wine_game_input(&runner.executable, &prefix_path, &package_path, task_id)
                .await?;
        }
    }

    update_progress(task_id, 1, Some(LAUNCH_TOTAL_STEPS), Some("launching"));

    if !request.auto_start {
        append_task_log(task_id, "已完成环境准备，未请求启动游戏");
        return Ok(None);
    }

    let mut command = Command::new(&runner.executable);
    match runner.kind {
        RunnerKind::Proton => {
            configure_proton_environment(&mut command, &runner, &prefix_path, task_id, false)
                .await?;
            let windows_game_executable = wine_z_path(&game_executable)?;
            append_task_log(
                task_id,
                format!(
                    "Proton-GDK 游戏路径：{}",
                    windows_game_executable.to_string_lossy()
                ),
            );
            command
                // GDK-Proton uses UMU_ID to bypass its Steam launcher shim. A
                // Windows Z: path is required here: passing the Unix path makes
                // Proton dispatch through start.exe /unix and detach early.
                .env("UMU_ID", "bmcbl-minecraft-bedrock")
                .env("STORE", "none")
                .arg("run")
                .arg(windows_game_executable);
        }
        RunnerKind::Wine => {
            command
                .env("WINEPREFIX", &prefix_path)
                .env("WINEARCH", "win64")
                .env("WINEDLLOVERRIDES", "dxgi,d3d11,d3d10core,d3d9=b")
                .arg(&game_executable);
        }
    }
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
    debug!(task_id, command = ?command, "prepared compatibility runner command");
    append_task_log(task_id, "正在启动 Minecraft Bedrock");

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

    let recent_output = Arc::new(Mutex::new(VecDeque::new()));
    let stdout_pump = child.stdout.take().map(|stdout| {
        spawn_output_pump(
            task_id.to_string(),
            stdout,
            false,
            recent_output.clone(),
            None,
        )
    });
    let stderr_pump = child.stderr.take().map(|stderr| {
        spawn_output_pump(
            task_id.to_string(),
            stderr,
            true,
            recent_output.clone(),
            None,
        )
    });

    tokio::time::sleep(EARLY_EXIT_GRACE_PERIOD).await;
    match child.try_wait() {
        Ok(Some(status)) => {
            finish_output_pumps(task_id, stdout_pump, stderr_pump).await;
            let output = recent_runner_output(&recent_output);
            let detail = if output.is_empty() {
                "兼容环境没有输出可用的错误信息".to_string()
            } else {
                output
            };
            let message = if status.success() {
                format!("Minecraft 未保持运行：Proton 在启动检测期内正常退出（{status}）\n{detail}")
            } else {
                format!("兼容环境启动后立即退出（{status}）\n{detail}")
            };
            return Err(message);
        }
        Ok(None) => {}
        Err(error) => return Err(format!("检查兼容环境进程状态失败：{error}")),
    }
    spawn_process_monitor(task_id.to_string(), child);
    update_progress(task_id, 1, Some(LAUNCH_TOTAL_STEPS), Some("launching"));
    update_progress(task_id, 0, Some(LAUNCH_TOTAL_STEPS), Some("running_game"));
    Ok(Some(process_id))
}

async fn stop_lingering_proton_processes(
    runner: &crate::core::linux_runtime::Runner,
    prefix_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    let proton_root = runner
        .executable
        .parent()
        .ok_or_else(|| "无法确定 Proton-GDK 安装目录".to_string())?;
    let wineserver = [
        proton_root.join("files/bin/wineserver"),
        proton_root.join("files/bin-wow64/wineserver"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .ok_or_else(|| {
        format!(
            "Proton-GDK 中没有找到 wineserver：{}",
            proton_root.display()
        )
    })?;

    append_task_log(task_id, "正在清理该实例遗留的 Wine 进程");
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new(&wineserver)
            .arg("-k")
            .arg("-w")
            .env("WINEPREFIX", prefix_path.join("pfx"))
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| "清理遗留的 Wine 进程超时".to_string())?
    .map_err(|error| format!("无法执行 wineserver {} 清理：{error}", wineserver.display()))?;
    append_command_output(task_id, &output.stdout, false);
    append_command_output(task_id, &output.stderr, true);
    if !output.status.success() {
        return Err(format!(
            "清理遗留的 Wine 进程失败，退出代码 {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

fn proton_steam_client_path(
    runner: &crate::core::linux_runtime::Runner,
) -> Result<PathBuf, String> {
    if let Some(steam_root) = runner.steam_root.as_ref() {
        return Ok(steam_root.clone());
    }
    if let Some(configured) = env::var_os("STEAM_COMPAT_CLIENT_INSTALL_PATH") {
        return Ok(PathBuf::from(configured));
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".local/share/Steam"))
        .ok_or_else(|| "无法确定 HOME，不能设置 STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string())
}

async fn configure_proton_environment(
    command: &mut Command,
    runner: &crate::core::linux_runtime::Runner,
    prefix_path: &Path,
    task_id: &str,
    use_wow64: bool,
) -> Result<(), String> {
    let steam_client_path = proton_steam_client_path(runner)?;
    tokio::fs::create_dir_all(&steam_client_path)
        .await
        .map_err(|error| {
            format!(
                "无法创建 Proton Steam 兼容目录 {}：{error}",
                steam_client_path.display()
            )
        })?;
    append_task_log(
        task_id,
        format!("Proton Steam 客户端目录：{}", steam_client_path.display()),
    );

    command
        .env("STEAM_COMPAT_DATA_PATH", prefix_path)
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_client_path)
        .env("WINEDLLOVERRIDES", "dxgi,d3d11,d3d10core,d3d9=b");

    if let Some(proton_root) = runner.executable.parent() {
        // GDK-Proton ships a 64-bit WoW64 loader specifically for hosts that
        // do not provide the legacy i386 ELF interpreter. Only opt in when the
        // selected local runner actually contains it.
        if use_wow64 && proton_root.join("files/bin-wow64/wine").is_file() {
            command.env("PROTON_USE_WOW64", "1");
        }
        let proton_lib_path = [
            proton_root.join("files/lib64"),
            proton_root.join("files/lib"),
        ]
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        if !proton_lib_path.is_empty() {
            let inherited = env::var_os("LD_LIBRARY_PATH")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            let library_path = std::iter::once(proton_lib_path.join(":"))
                .chain((!inherited.is_empty()).then_some(inherited))
                .collect::<Vec<_>>()
                .join(":");
            command.env("LD_LIBRARY_PATH", library_path);
        }
    }
    Ok(())
}

async fn initialize_wine_prefix(
    wine_executable: &Path,
    prefix_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    if prefix_path.join("drive_c").is_dir() && prefix_path.join("dosdevices").is_dir() {
        append_task_log(task_id, "Wine prefix 已存在，跳过初始化");
        return Ok(());
    }

    let wineboot = wine_executable
        .parent()
        .map(|parent| parent.join("wineboot"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("wineboot"));
    append_task_log(
        task_id,
        format!("初始化 Wine prefix：{}", prefix_path.display()),
    );

    let output = tokio::time::timeout(
        Duration::from_secs(120),
        Command::new(&wineboot)
            .arg("-u")
            .env("WINEPREFIX", prefix_path)
            .env("WINEARCH", "win64")
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| "初始化 Wine prefix 超时".to_string())?
    .map_err(|error| format!("无法执行 wineboot {}：{error}", wineboot.display()))?;

    append_command_output(task_id, &output.stdout, false);
    append_command_output(task_id, &output.stderr, true);
    if !output.status.success() {
        return Err(format!(
            "Wine prefix 初始化失败，退出代码 {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

async fn install_wine_game_input(
    wine_executable: &Path,
    prefix_path: &Path,
    package_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    let marker = prefix_path.join(".bmcbl-gameinput-installed");
    if marker.is_file() {
        return Ok(());
    }

    let installer = find_game_input_installer(package_path);
    let Some(installer) = installer else {
        append_task_log(task_id, "未找到 GameInput 安装包，跳过 Wine 组件安装");
        return Ok(());
    };

    // GDK archives seen in the wild may contain a literal `\\` in the Unix
    // filename. Wine interprets that character as a Windows separator, so
    // provide a normalized temporary name before invoking msiexec.
    let normalized_installer = if installer
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains('\\'))
    {
        let normalized = prefix_path.join("GameInputRedist.msi");
        std::fs::copy(&installer, &normalized)
            .map_err(|error| format!("复制 GameInput 安装包失败：{error}"))?;
        normalized
    } else {
        installer.clone()
    };
    append_task_log(
        task_id,
        format!("安装 Wine GameInput：{}", normalized_installer.display()),
    );
    // Wine cannot reliably execute an MSI by passing it directly to `wine`;
    // invoke the Windows Installer entry point instead (the Proton wrapper
    // handles this dispatch internally).
    let msiexec = wine_executable
        .parent()
        .map(|parent| parent.join("msiexec"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("msiexec"));
    let output = tokio::time::timeout(
        GAME_INPUT_INSTALL_TIMEOUT,
        Command::new(&msiexec)
            .arg("/i")
            .arg(&normalized_installer)
            .env("WINEPREFIX", prefix_path)
            .env("WINEARCH", "win64")
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| "安装 Wine GameInput 超时".to_string())?
    .map_err(|error| format!("无法启动 Wine Installer {}：{error}", msiexec.display()))?;

    append_command_output(task_id, &output.stdout, false);
    append_command_output(task_id, &output.stderr, true);
    if !output.status.success() {
        append_task_log(
            task_id,
            format!(
                "Wine GameInput 安装未完成（退出代码 {}），继续尝试启动游戏",
                output.status.code().unwrap_or(-1)
            ),
        );
        return Ok(());
    }
    if normalized_installer != installer {
        std::fs::remove_file(&normalized_installer)
            .map_err(|error| format!("清理 GameInput 临时安装包失败：{error}"))?;
    }
    std::fs::write(&marker, b"installed\n")
        .map_err(|error| format!("写入 GameInput 状态失败：{error}"))?;
    Ok(())
}

async fn install_proton_game_input(
    runner: &crate::core::linux_runtime::Runner,
    prefix_path: &Path,
    package_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    let marker = prefix_path.join(".bmcbl-proton-gameinput-installed");
    if marker.is_file() {
        append_task_log(task_id, "Proton-GDK GameInput 已安装，跳过初始化");
        return Ok(());
    }
    if proton_game_input_is_registered(prefix_path).await? {
        tokio::fs::write(&marker, b"installed\n")
            .await
            .map_err(|error| format!("写入 Proton-GDK GameInput 状态失败：{error}"))?;
        append_task_log(
            task_id,
            "已在 Proton prefix 中检测到 GameInput，跳过重复安装",
        );
        return Ok(());
    }

    let Some(installer) = find_game_input_installer(package_path) else {
        return Err(format!(
            "缺少 Proton-GDK 必需组件：在 {} 中未找到 Installers/GameInputRedist.msi",
            package_path.display()
        ));
    };

    let temporary_installer = if installer
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains('\\'))
    {
        let normalized = prefix_path.join("GameInputRedist.msi");
        tokio::fs::copy(&installer, &normalized)
            .await
            .map_err(|error| format!("复制 Proton-GDK GameInput 安装包失败：{error}"))?;
        Some(normalized)
    } else {
        None
    };
    let normalized_installer = temporary_installer.as_deref().unwrap_or(&installer);

    append_task_log(
        task_id,
        format!(
            "使用 Proton-GDK 安装 GameInput：{}",
            normalized_installer.display()
        ),
    );
    let mut command = Command::new(&runner.executable);
    configure_proton_environment(&mut command, runner, prefix_path, task_id, true).await?;
    let windows_installer = wine_z_path(normalized_installer)?;
    command
        // `runinprefix` uses Proton's 32-bit `files/bin/wine`. On distributions
        // without `/lib/ld-linux.so.2` that executable reports ENOENT even
        // though the file exists. UMU mode makes `run` dispatch this 64-bit
        // installer through `wine64` without the Steam wrapper.
        .env("UMU_ID", "bmcbl-gameinput")
        .arg("run")
        .arg("msiexec.exe")
        .arg("/i")
        .arg(&windows_installer)
        .arg("/quiet")
        .arg("/norestart")
        .current_dir(normalized_installer.parent().unwrap_or(package_path))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    debug!(task_id, command = ?command, "prepared Proton-GDK GameInput installer command");
    append_task_log(task_id, "正在通过 Proton-GDK 静默安装 GameInput Runtime");

    let mut child = command.spawn().map_err(|error| {
        format!(
            "无法通过 Proton-GDK 启动 GameInput 安装器 {}：{error}",
            runner.executable.display()
        )
    })?;
    let recent_output = Arc::new(Mutex::new(VecDeque::new()));
    let (failure_sender, mut failure_receiver) = mpsc::unbounded_channel();
    if let Some(stdout) = child.stdout.take() {
        drop(spawn_output_pump(
            task_id.to_string(),
            stdout,
            false,
            recent_output.clone(),
            Some(failure_sender.clone()),
        ));
    }
    if let Some(stderr) = child.stderr.take() {
        drop(spawn_output_pump(
            task_id.to_string(),
            stderr,
            true,
            recent_output.clone(),
            Some(failure_sender),
        ));
    }

    enum InstallOutcome {
        Exited(std::io::Result<std::process::ExitStatus>),
        RunnerFailure(String),
        TimedOut,
    }

    let outcome = tokio::select! {
        result = child.wait() => InstallOutcome::Exited(result),
        Some(failure) = failure_receiver.recv() => InstallOutcome::RunnerFailure(failure),
        () = tokio::time::sleep(GAME_INPUT_INSTALL_TIMEOUT) => InstallOutcome::TimedOut,
    };
    let status = match outcome {
        InstallOutcome::Exited(result) => {
            result.map_err(|error| format!("等待 Proton-GDK GameInput 安装失败：{error}"))?
        }
        InstallOutcome::RunnerFailure(failure) => {
            if let Err(error) = child.kill().await {
                append_task_log(task_id, format!("终止失败的 GameInput 安装器失败：{error}"));
            }
            return Err(failure);
        }
        InstallOutcome::TimedOut => {
            if let Err(error) = child.kill().await {
                append_task_log(task_id, format!("终止超时的 GameInput 安装器失败：{error}"));
            }
            let detail = recent_runner_output(&recent_output);
            return Err(if detail.is_empty() {
                "Proton-GDK GameInput 安装超时，安装器没有输出诊断信息".to_string()
            } else {
                format!("Proton-GDK GameInput 安装超时\n{detail}")
            });
        }
    };
    if !status.success() {
        let detail = recent_runner_output(&recent_output);
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!("\n{detail}")
        };
        return Err(format!(
            "Proton-GDK GameInput 安装失败，退出代码 {}{detail}",
            status.code().unwrap_or(-1)
        ));
    }

    append_task_log(task_id, "等待 Proton 写入 GameInput 注册状态");
    if !wait_for_proton_game_input_registration(prefix_path, GAME_INPUT_REGISTRATION_TIMEOUT)
        .await?
    {
        // Proton may return before wineserver flushes system.reg. BedrockBoot
        // treats a successful installer exit as completion; retain the extra
        // verification without turning delayed persistence into a false error.
        warn!(
            task_id,
            prefix = %prefix_path.display(),
            "GameInput installer succeeded but registry persistence is still pending"
        );
        append_task_log(
            task_id,
            "GameInput 安装器已成功退出，注册表仍在异步写入，按成功结果继续",
        );
    }

    if let Some(temporary_installer) = temporary_installer {
        tokio::fs::remove_file(&temporary_installer)
            .await
            .map_err(|error| format!("清理 Proton-GDK GameInput 临时安装包失败：{error}"))?;
    }

    tokio::fs::write(&marker, b"installed\n")
        .await
        .map_err(|error| format!("写入 Proton-GDK GameInput 状态失败：{error}"))?;
    append_task_log(task_id, "Proton-GDK GameInput 安装完成");
    Ok(())
}

async fn proton_game_input_is_registered(prefix_path: &Path) -> Result<bool, String> {
    for registry_path in [
        prefix_path.join("pfx/system.reg"),
        prefix_path.join("pfx/user.reg"),
    ] {
        match tokio::fs::read_to_string(&registry_path).await {
            Ok(registry) => {
                if registry.contains("GameInput3Redist")
                    || registry.contains("GameInput Redist Service")
                {
                    return Ok(true);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "读取 Proton GameInput 注册状态 {} 失败：{error}",
                    registry_path.display()
                ));
            }
        }
    }
    Ok(false)
}

async fn wait_for_proton_game_input_registration(
    prefix_path: &Path,
    timeout: Duration,
) -> Result<bool, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if proton_game_input_is_registered(prefix_path).await? {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn wine_z_path(path: &Path) -> Result<OsString, String> {
    if !path.is_absolute() {
        return Err(format!(
            "无法将相对路径转换为 Wine 路径：{}",
            path.display()
        ));
    }

    let mut windows_path = OsString::from("Z:");
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(value) => {
                windows_path.push("\\");
                windows_path.push(value);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("Wine 安装路径不能包含父目录：{}", path.display()));
            }
            Component::Prefix(_) => {
                return Err(format!("无法转换当前平台路径：{}", path.display()));
            }
        }
    }
    Ok(windows_path)
}

fn find_game_input_installer(package_path: &Path) -> Option<PathBuf> {
    [
        package_path.join("Installers").join("GameInputRedist.msi"),
        package_path.join("Installers\\GameInputRedist.msi"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn append_command_output(task_id: &str, output: &[u8], is_error: bool) {
    let stream = if is_error { "stderr" } else { "stdout" };
    let text = String::from_utf8_lossy(output);
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        append_task_log(task_id, format!("{stream}: {line}"));
    }
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

fn spawn_output_pump<R>(
    task_id: String,
    reader: R,
    is_error: bool,
    recent_output: Arc<Mutex<VecDeque<String>>>,
    failure_sender: Option<mpsc::UnboundedSender<String>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let prefix = if is_error { "runner stderr" } else { "runner" };
                    let displayed_line = normalize_runner_output_line(&line);
                    let log_line = format!("{prefix}: {displayed_line}");
                    append_task_log(&task_id, log_line.clone());
                    if let Some(failure) = classify_runner_failure(&line)
                        && let Some(sender) = failure_sender.as_ref()
                        && let Err(error) = sender.send(failure)
                    {
                        debug!(task_id, %error, "runner failure receiver already closed");
                    }
                    let mut output = recent_output
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if output.len() >= RECENT_RUNNER_OUTPUT_LIMIT {
                        output.pop_front();
                    }
                    output.push_back(log_line);
                }
                Ok(None) => break,
                Err(error) => {
                    warn!(task_id, %error, "failed to read compatibility runner output");
                    break;
                }
            }
        }
    })
}

async fn finish_output_pumps(
    task_id: &str,
    stdout_pump: Option<tokio::task::JoinHandle<()>>,
    stderr_pump: Option<tokio::task::JoinHandle<()>>,
) {
    let drain = async {
        if let Some(stdout_pump) = stdout_pump
            && let Err(error) = stdout_pump.await
        {
            warn!(task_id, %error, "runner stdout pump failed");
        }
        if let Some(stderr_pump) = stderr_pump
            && let Err(error) = stderr_pump.await
        {
            warn!(task_id, %error, "runner stderr pump failed");
        }
    };
    if tokio::time::timeout(Duration::from_secs(2), drain)
        .await
        .is_err()
    {
        warn!(
            task_id,
            "timed out while draining compatibility runner output"
        );
    }
}

fn normalize_runner_output_line(line: &str) -> &str {
    if line.contains("Skipping fix execution. We are probably running a unit test.") {
        "ProtonFixes: 外部启动器模式，跳过游戏专用 fixes"
    } else {
        line
    }
}

fn classify_runner_failure(line: &str) -> Option<String> {
    if line.contains("unimplemented function combase.dll.RoOriginateErrorW") {
        return Some(
            "当前 Proton-GDK 的 combase.dll 没有实现 RoOriginateErrorW，无法启动该 Minecraft 版本。请在 Proton-GDK 设置中安装并选择 LukasPAH Custom 版本"
                .to_string(),
        );
    }
    if line.contains("/lib/ld-linux.so.2: could not open") {
        return Some(
            "Proton-GDK 无法启动兼容载入器：当前 runner 试图使用缺失的 /lib/ld-linux.so.2。BMCBL 已尝试切换到 Proton-GDK 自带的 WoW64 runner；如仍出现此错误，请重新安装或更换 Proton-GDK 版本"
                .to_string(),
        );
    }
    if line.contains("FileNotFoundError:") && line.contains("files/bin/wine") {
        return Some(
            "Proton-GDK 的默认 Wine 载入器无法执行。BMCBL 已尝试切换到 Proton-GDK 自带的 WoW64 runner；如仍失败，请重新安装或更换 Proton-GDK 版本"
                .to_string(),
        );
    }
    None
}

fn recent_runner_output(output: &Arc<Mutex<VecDeque<String>>>) -> String {
    output
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
}

fn spawn_process_monitor(task_id: String, mut child: tokio::process::Child) {
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => {
                append_task_log(&task_id, format!("游戏进程已退出：{status}"));
                if status.success() {
                    finish_task(&task_id, "completed", Some("游戏已退出".to_string()));
                } else {
                    let message = format!("游戏进程异常退出：{status}");
                    warn!(task_id, %status, "compatibility runner exited with failure");
                    finish_task(&task_id, "error", Some(message));
                }
            }
            Err(error) => {
                warn!(task_id, %error, "failed to wait for compatibility runner process");
                append_task_log(&task_id, format!("等待游戏进程失败：{error}"));
                finish_task(
                    &task_id,
                    "error",
                    Some(format!("等待游戏进程失败：{error}")),
                );
            }
        };
    });
}

#[cfg(test)]
#[path = "task_linux_tests.rs"]
mod tests;
