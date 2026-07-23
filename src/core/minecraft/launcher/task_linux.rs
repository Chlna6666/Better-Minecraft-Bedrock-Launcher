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

const INJECTOR_BYTES: &[u8] = include_bytes!("../../../../assets/bin/BLoader.dll");

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

fn resolve_wine64(runner: &crate::core::linux_runtime::Runner) -> Result<PathBuf, String> {
    let proton_root = runner
        .executable
        .parent()
        .ok_or_else(|| "无法确定 Proton-GDK 安装目录".to_string())?;
    let wine64 = proton_root.join("files/bin/wine64");
    if !wine64.is_file() {
        return Err(format!(
            "Proton-GDK 中没有找到 wine64：{}",
            wine64.display()
        ));
    }
    Ok(wine64)
}

fn set_runner_ld_library_path(command: &mut Command, runner: &crate::core::linux_runtime::Runner) {
    let Some(proton_root) = runner.executable.parent() else {
        return;
    };
    let lib_paths = [
        proton_root.join("files/lib64"),
        proton_root.join("files/lib"),
    ]
    .into_iter()
    .map(|path| path.to_string_lossy().into_owned())
    .collect::<Vec<_>>();
    command.env("LD_LIBRARY_PATH", lib_paths.join(":"));
}

async fn copy_runner_dlls(
    runner: &crate::core::linux_runtime::Runner,
    prefix_path: &Path,
) -> Result<(), String> {
    let proton_root = runner
        .executable
        .parent()
        .ok_or_else(|| "无法确定 Proton-GDK 安装目录".to_string())?;
    let default_pfx = proton_root.join("files/share/default_pfx/drive_c/windows/system32");
    if !default_pfx.is_dir() {
        return Ok(());
    }
    let target = prefix_path.join("pfx/drive_c/windows/system32");
    tokio::fs::create_dir_all(&target)
        .await
        .map_err(|error| format!("创建 Wine system32 目录失败：{error}"))?;
    let mut entries = tokio::fs::read_dir(&default_pfx)
        .await
        .map_err(|error| format!("读取 default_pfx 目录失败：{error}"))?;
    let mut copied = 0usize;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let file_name = entry.file_name();
        let source = entry.path();
        if !source.is_file() {
            continue;
        }
        let dest = target.join(&file_name);
        if dest.exists() {
            continue;
        }
        tokio::fs::copy(&source, &dest).await.map_err(|error| {
            format!(
                "复制 {} 到 {} 失败：{error}",
                source.display(),
                dest.display()
            )
        })?;
        copied += 1;
    }
    debug!(copied, "copied Proton runtime DLLs to Wine prefix");
    Ok(())
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

async fn inject_bloader(exe_path: &Path, task_id: &str) -> Result<(), String> {
    let exe_dir = exe_path
        .parent()
        .ok_or_else(|| "无法确定游戏可执行文件目录".to_string())?;
    let injector_path = exe_dir.join("BLoader.dll");
    let need_update = if injector_path.is_file() {
        let disk_bytes = tokio::fs::read(&injector_path)
            .await
            .map_err(|error| format!("读取现有 BLoader.dll 失败：{error}"))?;
        disk_bytes != INJECTOR_BYTES
    } else {
        true
    };
    if need_update {
        tokio::fs::write(&injector_path, INJECTOR_BYTES)
            .await
            .map_err(|error| format!("写入 BLoader.dll 失败：{error}"))?;
        append_task_log(
            task_id,
            format!("部署 BLoader.dll：{}", injector_path.display()),
        );
    }
    if crate::core::inject::pe::is_file_patched(exe_path) {
        append_task_log(task_id, "游戏 EXE 已包含补丁标记，跳过注入".to_string());
    } else {
        let exe_path = exe_path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            crate::core::inject::pe::ensure_backup(&exe_path)
                .map_err(|error| format!("创建 EXE 备份失败：{error}"))?;
            crate::core::inject::pe::restore_original_pe(&exe_path)
                .map_err(|error| format!("还原 PE 失败：{error}"))?;
            crate::core::inject::pe::inject_dll_import(&exe_path, "BLoader.dll", None)
                .map_err(|error| format!("PE 注入失败：{error}"))
        })
        .await
        .map_err(|error| format!("BLoader 注入任务失败：{error}"))??;
        append_task_log(task_id, "BLoader.dll 已注入游戏 EXE".to_string());
    }
    Ok(())
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

    // Inject BLoader.dll into the game EXE, matching the Windows launcher.
    // Without this, the Windows App Runtime bootstrapper fails to find
    // runtime 1.8 and the game exits with code 3.
    inject_bloader(&game_executable, task_id).await?;

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
            // Ensure the dosdevices directory exists before any Wine
            // operations. Copy the full Proton default_pfx content so
            // both msiexec (GameInput) and the game (BLoader.dll) find
            // their DLL dependencies.
            let dosdevices = prefix_path.join("pfx").join("dosdevices");
            tokio::fs::create_dir_all(&dosdevices)
                .await
                .map_err(|error| format!("创建 dosdevices 目录失败：{error}"))?;
            copy_runner_dlls(&runner, &prefix_path).await?;
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

    let mut command = match runner.kind {
        RunnerKind::Proton => {
            let wine64 = resolve_wine64(&runner)?;
            let mut command = Command::new(&wine64);
            set_runner_ld_library_path(&mut command, &runner);
            let windows_game_executable = wine_z_path(&game_executable)?;
            append_task_log(
                task_id,
                format!(
                    "GDK 游戏路径：{}",
                    windows_game_executable.to_string_lossy()
                ),
            );
            command
                .env("WINEPREFIX", prefix_path.join("pfx"))
                .env("WINEARCH", "win64")
                .env("WINEDLLOVERRIDES", "dxgi,d3d11,d3d10core,d3d9=b")
                .arg(&windows_game_executable);
            command
        }
        RunnerKind::Wine => {
            let mut command = Command::new(&runner.executable);
            command
                .env("WINEPREFIX", &prefix_path)
                .env("WINEARCH", "win64")
                .env("WINEDLLOVERRIDES", "dxgi,d3d11,d3d10core,d3d9=b")
                .arg(&game_executable);
            if let Some(working_directory) = game_executable.parent() {
                command.current_dir(working_directory);
            }
            command
        }
    };
    if let Some(argument) = request
        .launch_args
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        command.arg(argument);
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

    // Do not perform an early-exit check — let the game run as long as
    // it needs via WaitForExitAsync.
    // When Proton dispatches through start.exe /unix the wrapper process
    // can exit before the game, and the game itself may produce no output
    // until the window appears. Treat any early exit as a transition to
    // "running_game" so the process monitor can observe the real outcome.
    tokio::time::sleep(EARLY_EXIT_GRACE_PERIOD).await;
    match child.try_wait() {
        Ok(Some(status)) => {
            finish_output_pumps(task_id, stdout_pump, stderr_pump).await;
            if !status.success() {
                let output = recent_runner_output(&recent_output);
                let detail = if output.is_empty() {
                    String::new()
                } else {
                    format!("\n{output}")
                };
                append_task_log(
                    task_id,
                    format!("兼容环境在启动检测期内退出（{status}），继续监控游戏进程{detail}"),
                );
            }
        }
        Ok(None) => {}
        Err(error) => {
            append_task_log(task_id, format!("检查兼容环境进程状态失败：{error}"));
        }
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
        // `wineserver -k` returns exit code 1 when no Wine process is running
        // for this prefix — the normal case on a fresh or cleanly-closed
        // prefix. The wineserver cleanup is not strictly necessary, so
        // treat a non-zero exit as a benign no-op instead of aborting the
        // launch. Only spawn failures and timeouts remain fatal (handled
        // above via `?`).
        let code = output.status.code().unwrap_or(-1);
        warn!(
            task_id,
            code, "wineserver -k exited non-zero; treating as no-op when no process is running"
        );
        append_task_log(
            task_id,
            format!("没有正在运行的 Wine 进程（wineserver 退出代码 {code}），继续启动"),
        );
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
    let wine64 = resolve_wine64(runner)?;
    let mut command = Command::new(&wine64);
    set_runner_ld_library_path(&mut command, runner);
    let windows_installer = wine_z_path(normalized_installer)?;
    command
        .env("WINEPREFIX", prefix_path.join("pfx"))
        .env("WINEARCH", "win64")
        .arg("msiexec")
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
            append_task_log(task_id, "GameInput 安装超时，跳过（不影响基础游戏启动）");
            return Ok(());
        }
    };
    if !status.success() {
        let detail = recent_runner_output(&recent_output);
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!("\n{detail}")
        };
        append_task_log(
            task_id,
            format!(
                "GameInput 安装未完成（退出代码 {}）{detail}，继续尝试启动游戏",
                status.code().unwrap_or(-1)
            ),
        );
        return Ok(());
    }

    append_task_log(task_id, "等待 Proton 写入 GameInput 注册状态");
    if !wait_for_proton_game_input_registration(prefix_path, GAME_INPUT_REGISTRATION_TIMEOUT)
        .await?
    {
        // Proton may return before wineserver flushes system.reg. Treat
        // a successful installer exit as completion; retain the extra
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
