use super::bloader;
use crate::core::linux_runtime::{
    RunnerKind, resolve_runner, runner_runtime_root, validate_proton_game_runtime,
};
use crate::tasks::task_manager::{
    append_task_log, create_task_with_details, finish_task, register_task_abort_handle, set_total,
    update_progress,
};
use std::collections::VecDeque;
use std::env;
use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

const LAUNCH_TOTAL_STEPS: u64 = 3;
const EARLY_EXIT_GRACE_PERIOD: Duration = Duration::from_secs(8);
const GAME_INPUT_INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const GAME_INPUT_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);
const PROTON_PREFIX_INITIALIZATION_TIMEOUT: Duration = Duration::from_secs(120);
const ROUNDMCDEV_PREFIX_READY_TIMEOUT: Duration = Duration::from_secs(30);
const ROUNDMCDEV_PREFIX_READY_POLL_INTERVAL: Duration = Duration::from_secs(1);
const RECENT_RUNNER_OUTPUT_LIMIT: usize = 32;
const GDK_GNUTLS_PRIORITY: &[u8] = b"[priorities]\nSYSTEM = NORMAL:-VERS-TLS1.3:%COMPAT\n";

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

fn set_runner_ld_library_path(command: &mut Command, runner: &crate::core::linux_runtime::Runner) {
    let Some(proton_root) = runner_runtime_root(runner) else {
        return;
    };
    let mut lib_paths = [
        proton_root.join("files/lib64"),
        proton_root.join("files/lib"),
    ]
    .into_iter()
    .filter(|path| path.is_dir())
    .map(|path| path.to_string_lossy().into_owned())
    .collect::<Vec<_>>();
    if let Some(inherited) = env::var_os("LD_LIBRARY_PATH").filter(|value| !value.is_empty()) {
        lib_paths.push(inherited.to_string_lossy().into_owned());
    }
    if !lib_paths.is_empty() {
        command.env("LD_LIBRARY_PATH", lib_paths.join(":"));
    }
}

fn proton_wine_prefix_path(compatibility_path: &Path) -> PathBuf {
    compatibility_path.join("pfx")
}

fn runner_supports_winegdk_login(kind: RunnerKind) -> bool {
    kind == RunnerKind::Umu
}

fn proton_steam_client_path(
    runner: &crate::core::linux_runtime::Runner,
) -> Result<PathBuf, String> {
    if runner.kind == RunnerKind::Umu {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".steam/steam"))
            .ok_or_else(|| {
                "无法确定 HOME，不能设置 RoundMCDev 的 STEAM_COMPAT_CLIENT_INSTALL_PATH".to_string()
            });
    }
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

async fn configure_proton_command(
    command: &mut Command,
    runner: &crate::core::linux_runtime::Runner,
    prefix_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    let proton_root =
        runner_runtime_root(runner).ok_or_else(|| "无法确定 Proton-GDK 安装目录".to_string())?;
    let bundle_root = (runner.kind == RunnerKind::Umu)
        .then(|| crate::core::linux_runtime::roundmcdev_bundle_root(runner))
        .flatten();
    let steam_client_path = proton_steam_client_path(runner)?;
    let proton_log_directory = crate::utils::file_ops::logs_dir().join("proton");
    let gnutls_priority_file = if runner.kind == RunnerKind::Umu {
        bundle_root
            .as_ref()
            .ok_or_else(|| "无法确定 RoundMCDev UMU 资源包目录".to_string())?
            .join("etc/gnutls-no-tls13.cfg")
    } else {
        crate::utils::file_ops::config_dir().join("compat/gnutls-no-tls13.cfg")
    };
    tokio::fs::create_dir_all(&steam_client_path)
        .await
        .map_err(|error| {
            format!(
                "创建 Proton Steam 兼容目录 {} 失败：{error}",
                steam_client_path.display()
            )
        })?;
    tokio::fs::create_dir_all(&proton_log_directory)
        .await
        .map_err(|error| {
            format!(
                "创建 Proton 日志目录 {} 失败：{error}",
                proton_log_directory.display()
            )
        })?;
    if let Some(parent) = gnutls_priority_file.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("创建 GDK TLS 配置目录失败：{error}"))?;
    }
    let priority_is_current = tokio::fs::read(&gnutls_priority_file)
        .await
        .is_ok_and(|contents| contents == GDK_GNUTLS_PRIORITY);
    if !priority_is_current {
        tokio::fs::write(&gnutls_priority_file, GDK_GNUTLS_PRIORITY)
            .await
            .map_err(|error| format!("写入 GDK TLS 兼容配置失败：{error}"))?;
    }

    command
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam_client_path)
        .env("UMU_ID", "bmcbl-minecraft-bedrock")
        .env("STORE", "none")
        .env("GNUTLS_SYSTEM_PRIORITY_FILE", &gnutls_priority_file)
        .env("GNUTLS_SYSTEM_PRIORITY_FAIL_ON_INVALID", "0")
        .env("WINEDLLOVERRIDES", "dxgi,d3d11,d3d10core,d3d9,advapi32=b")
        .env("VKD3D_CONFIG", "force_raw_va_cbv")
        .env(
            "MICROSOFT_WINDOWSAPPRUNTIME_BOOTSTRAP_INITIALIZE_SHOWUI",
            "0",
        )
        .env(
            "MICROSOFT_WINDOWSAPPRUNTIME_BOOTSTRAP_INITIALIZE_FAILFAST",
            "0",
        )
        .env(
            "MICROSOFT_WINDOWSAPPRUNTIME_DEPLOYMENT_INITIALIZE_ONERRORSHOWUI",
            "0",
        );
    if runner.kind == RunnerKind::Umu {
        let bundle_root =
            bundle_root.ok_or_else(|| "无法确定 RoundMCDev UMU 资源包目录".to_string())?;
        let wine_prefix = proton_wine_prefix_path(prefix_path);
        command
            .env("PROTONPATH", &proton_root)
            .env("PROTON_VERB", "run")
            .env("WINEPREFIX", wine_prefix)
            .env("UMU_FOLDERS_PATH", bundle_root)
            .env("UMU_RUNTIME_UPDATE", "0")
            .env("GAMEID", "umu-default")
            .env("WINEDEBUG", "-all");
    } else {
        command
            .env("STEAM_COMPAT_DATA_PATH", prefix_path)
            .env("PROTON_LOG", "1")
            .env("PROTON_LOG_DIR", &proton_log_directory);
    }
    set_runner_ld_library_path(command, runner);
    append_task_log(
        task_id,
        format!("Proton 日志目录：{}", proton_log_directory.display()),
    );
    Ok(())
}

fn proton_prefix_has_metadata(prefix_path: &Path) -> bool {
    ["version", "tracked_files", "config_info"]
        .into_iter()
        .any(|name| prefix_path.join(name).is_file())
}

fn proton_prefix_has_registry(prefix_path: &Path) -> bool {
    let prefix = prefix_path.join("pfx");
    prefix.join("system.reg").is_file() || prefix.join("user.reg").is_file()
}

fn incompatible_proton_prefix_needs_backup(prefix_path: &Path) -> bool {
    proton_prefix_has_registry(prefix_path) && !proton_prefix_has_metadata(prefix_path)
}

async fn backup_incompatible_proton_prefix(
    prefix_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    if !incompatible_proton_prefix_needs_backup(prefix_path) {
        return Ok(());
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("生成旧 Prefix 备份时间戳失败：{error}"))?
        .as_secs();
    let current_prefix = prefix_path.join("pfx");
    let backup_prefix = prefix_path.join(format!("pfx.bmcbl-wine-backup-{timestamp}"));
    tokio::fs::rename(&current_prefix, &backup_prefix)
        .await
        .map_err(|error| {
            format!(
                "备份不兼容 Prefix {} 到 {} 失败：{error}",
                current_prefix.display(),
                backup_prefix.display()
            )
        })?;
    let marker = prefix_path.join(".bmcbl-proton-gameinput-installed");
    if let Err(error) = tokio::fs::remove_file(&marker).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!(
            "清理旧 GameInput 状态 {} 失败：{error}",
            marker.display()
        ));
    }
    append_task_log(
        task_id,
        format!(
            "检测到由裸 Wine 创建的旧 Prefix，已保留备份：{}",
            backup_prefix.display()
        ),
    );
    Ok(())
}

async fn initialize_proton_prefix(
    runner: &crate::core::linux_runtime::Runner,
    prefix_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    if runner.kind == RunnerKind::Umu {
        return initialize_roundmcdev_prefix(runner, prefix_path, task_id).await;
    }
    backup_incompatible_proton_prefix(prefix_path, task_id).await?;
    let dosdevices = prefix_path.join("pfx/dosdevices");
    tokio::fs::create_dir_all(&dosdevices)
        .await
        .map_err(|error| format!("创建 Proton dosdevices 目录失败：{error}"))?;
    let mut command = Command::new(&runner.executable);
    configure_proton_command(&mut command, runner, prefix_path, task_id).await?;
    if runner.kind == RunnerKind::Proton {
        command.arg("run");
    }
    command
        .arg(r"C:\windows\system32\cmd.exe")
        .arg("/c")
        .arg("exit")
        .arg("0")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    debug!(task_id, command = ?command, "prepared Proton prefix initialization command");
    append_task_log(
        task_id,
        if runner.kind == RunnerKind::Umu {
            "正在通过 UMU/Proton-GDK 初始化 Prefix"
        } else {
            "正在通过 Proton wrapper 初始化 Prefix"
        },
    );

    let output = tokio::time::timeout(PROTON_PREFIX_INITIALIZATION_TIMEOUT, command.output())
        .await
        .map_err(|_| "Proton Prefix 初始化超时".to_string())?
        .map_err(|error| format!("无法启动 Proton Prefix 初始化：{error}"))?;
    append_command_output(task_id, &output.stdout, false);
    append_command_output(task_id, &output.stderr, true);
    if !output.status.success() {
        return Err(format!(
            "Proton Prefix 初始化失败，退出代码 {}",
            output.status.code().unwrap_or(-1)
        ));
    }
    let prefix = prefix_path.join("pfx");
    if !prefix.join("system.reg").is_file()
        || !prefix.join("user.reg").is_file()
        || !prefix.join("drive_c/windows/system32").is_dir()
    {
        return Err(format!(
            "Proton wrapper 已退出，但 Prefix 不完整：{}",
            prefix.display()
        ));
    }
    append_task_log(task_id, "Proton Prefix 初始化完成");
    Ok(())
}

async fn initialize_roundmcdev_prefix(
    runner: &crate::core::linux_runtime::Runner,
    compatibility_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    let wine_prefix = proton_wine_prefix_path(compatibility_path);
    if roundmcdev_prefix_is_ready(&wine_prefix) {
        append_task_log(
            task_id,
            format!("RoundMCDev Wine Prefix 已就绪：{}", wine_prefix.display()),
        );
        return Ok(());
    }
    tokio::fs::create_dir_all(&wine_prefix)
        .await
        .map_err(|error| format!("创建 RoundMCDev Wine Prefix 失败：{error}"))?;
    let proton_root = crate::core::linux_runtime::runner_runtime_root(runner)
        .ok_or_else(|| "无法确定 RoundMCDev GDK-Proton 目录".to_string())?;
    let proton = proton_root.join("proton");
    if !proton.is_file() {
        return Err(format!(
            "RoundMCDev GDK-Proton 缺少 proton wrapper：{}",
            proton.display()
        ));
    }

    let steam_client_path = proton_steam_client_path(runner)?;
    tokio::fs::create_dir_all(&steam_client_path)
        .await
        .map_err(|error| format!("创建 Proton Steam 兼容目录失败：{error}"))?;
    let mut command = Command::new(&proton);
    command
        .arg("run")
        .arg("wineboot")
        .arg("-u")
        .env("WINEPREFIX", &wine_prefix)
        .env("STEAM_COMPAT_DATA_PATH", compatibility_path)
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &steam_client_path)
        .env("WINEDEBUG", "-all")
        .env("WINEDLLOVERRIDES", "dxgi,d3d11,d3d10core,d3d9,advapi32=b")
        .env("SDL_VIDEODRIVER", "dummy");
    set_runner_ld_library_path(&mut command, runner);
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    append_task_log(task_id, "正在通过 GDK-Proton wrapper 执行 wineboot -u");
    let output = tokio::time::timeout(PROTON_PREFIX_INITIALIZATION_TIMEOUT, command.output())
        .await
        .map_err(|_| "RoundMCDev Wine Prefix 初始化超时".to_string())?
        .map_err(|error| format!("无法启动 RoundMCDev wineboot：{error}"))?;
    append_command_output(task_id, &output.stdout, false);
    append_command_output(task_id, &output.stderr, true);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            String::new()
        } else {
            format!("：{stderr}")
        };
        return Err(format!(
            "RoundMCDev Wine Prefix 初始化失败，退出代码 {}{detail}",
            output.status.code().unwrap_or(-1),
        ));
    }
    stop_lingering_proton_processes(runner, compatibility_path, task_id).await?;
    if !wait_for_roundmcdev_prefix_ready(&wine_prefix).await {
        return Err(format!(
            "GDK-Proton wineboot 已退出，但等待 {} 秒后 Prefix 仍不完整：{}",
            ROUNDMCDEV_PREFIX_READY_TIMEOUT.as_secs(),
            wine_prefix.display()
        ));
    }
    append_task_log(task_id, "RoundMCDev Wine Prefix 初始化完成");
    Ok(())
}

fn roundmcdev_prefix_is_ready(wine_prefix: &Path) -> bool {
    wine_registry_has_valid_header(&wine_prefix.join("system.reg"))
        && wine_registry_has_valid_header(&wine_prefix.join("user.reg"))
        && wine_prefix.join("drive_c/windows/system32").is_dir()
}

fn wine_registry_has_valid_header(path: &Path) -> bool {
    let Ok(mut registry) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0_u8; 64];
    let Ok(read) = registry.read(&mut header) else {
        return false;
    };
    let header = &header[..read];
    let first_non_null = header
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(header.len());
    header
        .get(first_non_null..)
        .is_some_and(|header| header.starts_with(b"WINE REGISTRY Version "))
}

async fn wait_for_roundmcdev_prefix_ready(wine_prefix: &Path) -> bool {
    let deadline = std::time::Instant::now() + ROUNDMCDEV_PREFIX_READY_TIMEOUT;
    loop {
        if roundmcdev_prefix_is_ready(wine_prefix) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(ROUNDMCDEV_PREFIX_READY_POLL_INTERVAL).await;
    }
}

fn request_uses_preview_data(request: &LaunchRequest) -> bool {
    [
        request.folder_name.as_ref(),
        request.display_name.as_ref(),
        request.package_folder.as_ref(),
    ]
    .into_iter()
    .any(|value| {
        let value = value.to_ascii_lowercase();
        value.contains("preview") || value.contains("beta") || value.contains("预览")
    })
}

async fn ensure_gdk_data_directories(
    wine_prefix: &Path,
    request: &LaunchRequest,
    task_id: &str,
) -> Result<(), String> {
    let edition_folder = if request_uses_preview_data(request) {
        "Minecraft Bedrock Preview"
    } else {
        "Minecraft Bedrock"
    };
    let com_mojang = wine_prefix
        .join("drive_c/users/steamuser/AppData/Roaming")
        .join(edition_folder)
        .join("Users/Shared/games/com.mojang");
    for directory in [
        "minecraftWorlds",
        "resource_packs",
        "behavior_packs",
        "skin_packs",
        "world_templates",
        "minecraftpe",
        "Screenshots",
        "development_resource_packs",
        "development_behavior_packs",
        "development_skin_packs",
    ] {
        let path = com_mojang.join(directory);
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|error| format!("创建 GDK 数据目录 {} 失败：{error}", path.display()))?;
    }
    append_task_log(
        task_id,
        format!("GDK 用户数据目录已就绪：{}", com_mojang.display()),
    );
    Ok(())
}

pub fn start_launch_task(request: LaunchRequest) -> String {
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
    let join_handle = match crate::tasks::runtime::spawn_io(async move {
        match launch_game(&request, &task_id_for_task).await {
            Ok(Some(process_id)) => {
                append_task_log(
                    &task_id_for_task,
                    format!("游戏进程已启动，PID {process_id}"),
                );
                finish_task(
                    &task_id_for_task,
                    "completed",
                    Some(format!("游戏已启动，PID {process_id}")),
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
    }) {
        Ok(join_handle) => join_handle,
        Err(error) => {
            append_task_log(&task_id, format!("无法调度启动任务：{error}"));
            finish_task(&task_id, "error", Some(error));
            return task_id;
        }
    };
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
        bloader::version_string(&disk_bytes).as_deref() != Some(bloader::embedded_version_string())
    } else {
        true
    };
    if need_update {
        let injector_bytes = bloader::bytes()?;
        tokio::fs::write(&injector_path, injector_bytes)
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
        crate::tasks::runtime::run_io_blocking(move || {
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
    let runner = crate::tasks::runtime::run_io_blocking(resolve_runner)
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
    let game_executable = crate::tasks::runtime::run_io_blocking(move || {
        resolve_game_executable(&package_path_for_probe)
    })
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

    crate::tasks::runtime::run_io_blocking({
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
        RunnerKind::Proton | RunnerKind::Umu => {
            initialize_proton_prefix(&runner, &prefix_path, task_id).await?;
            let wine_prefix = proton_wine_prefix_path(&prefix_path);
            ensure_gdk_data_directories(&wine_prefix, request, task_id).await?;
            if runner.kind == RunnerKind::Umu {
                apply_roundmcdev_winegdk_prerequisites(&runner, &wine_prefix, task_id).await?;
                install_roundmcdev_cryptbase(&runner, &wine_prefix, task_id).await?;
                apply_roundmcdev_proton_patches(&runner, &package_path, &game_executable, task_id)
                    .await?;
            }
            install_proton_game_input(&runner, &prefix_path, &package_path, task_id).await?;
            stop_lingering_proton_processes(&runner, &prefix_path, task_id).await?;
        }
        RunnerKind::Wine => {
            initialize_wine_prefix(&runner.executable, &prefix_path, task_id).await?;
            install_wine_game_input(&runner.executable, &prefix_path, &package_path, task_id)
                .await?;
        }
    }

    apply_roundmcdev_game_fixes(&runner, &game_executable, task_id).await?;

    update_progress(task_id, 1, Some(LAUNCH_TOTAL_STEPS), Some("launching"));

    if !request.auto_start {
        append_task_log(task_id, "已完成环境准备，未请求启动游戏");
        return Ok(None);
    }

    let launch_auth = if runner_supports_winegdk_login(runner.kind) {
        let wine_prefix = proton_wine_prefix_path(&prefix_path);
        let launch_auth = crate::core::bedrock_auth::prepare_launch(&wine_prefix).await?;
        append_task_log(
            task_id,
            if launch_auth.is_some() {
                "已准备 RoundMCDev WineGDK 登录会话"
            } else {
                "未检测到 WineGDK 登录凭证，将以未登录状态启动"
            },
        );
        launch_auth
    } else {
        append_task_log(
            task_id,
            "当前运行器不提供 RoundMCDev WineGDK 登录，以游戏原生未登录状态启动",
        );
        None
    };

    let mut command = match runner.kind {
        RunnerKind::Proton => {
            let mut command = Command::new(&runner.executable);
            configure_proton_command(&mut command, &runner, &prefix_path, task_id).await?;
            let windows_game_executable = wine_z_path(&game_executable)?;
            append_task_log(
                task_id,
                format!(
                    "GDK 游戏路径：{}",
                    windows_game_executable.to_string_lossy()
                ),
            );
            command
                .arg("run")
                .arg(&windows_game_executable)
                .current_dir(game_executable.parent().unwrap_or(&package_path));
            command
        }
        RunnerKind::Umu => {
            let mut command = Command::new(&runner.executable);
            configure_proton_command(&mut command, &runner, &prefix_path, task_id).await?;
            append_task_log(
                task_id,
                format!("UMU GDK 游戏路径：{}", game_executable.display()),
            );
            command
                .arg(&game_executable)
                .current_dir(game_executable.parent().unwrap_or(&package_path));
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
    if let Some(auth) = &launch_auth {
        auth.apply_to_command(&mut command)?;
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
    let publish_startup_output = Arc::new(AtomicBool::new(true));
    let mut stdout_pump = child
        .stdout
        .take()
        .map(|stdout| {
            spawn_output_pump(
                task_id.to_string(),
                stdout,
                false,
                recent_output.clone(),
                None,
                Some(publish_startup_output.clone()),
            )
        })
        .transpose()?;
    let mut stderr_pump = child
        .stderr
        .take()
        .map(|stderr| {
            spawn_output_pump(
                task_id.to_string(),
                stderr,
                true,
                recent_output.clone(),
                None,
                Some(publish_startup_output.clone()),
            )
        })
        .transpose()?;

    tokio::time::sleep(EARLY_EXIT_GRACE_PERIOD).await;
    match child.try_wait() {
        Ok(Some(status)) => {
            finish_output_pumps(task_id, stdout_pump.take(), stderr_pump.take()).await;
            let output = recent_runner_output(&recent_output);
            let detail = if output.is_empty() {
                "\nProton wrapper 没有输出；请查看任务中记录的 Proton 日志目录".to_string()
            } else {
                format!("\n{output}")
            };
            return Err(format!("兼容运行器在启动检测期内退出（{status}）{detail}"));
        }
        Ok(None) => {}
        Err(error) => {
            return Err(format!("检查兼容运行器进程状态失败：{error}"));
        }
    }
    publish_startup_output.store(false, Ordering::Release);
    let session = crate::core::version::game_info::GameSession::start(
        PathBuf::from(request.package_folder.as_ref()),
        process_id,
    )
    .await
    .map_err(|error| {
        append_task_log(task_id, format!("记录游戏启动统计失败：{error}"));
        error
    })
    .ok()
    .flatten();
    spawn_process_monitor(
        task_id.to_string(),
        child,
        stdout_pump,
        stderr_pump,
        recent_output,
        launch_auth,
        session,
    );
    update_progress(task_id, 1, Some(LAUNCH_TOTAL_STEPS), Some("launching"));
    update_progress(task_id, 0, Some(LAUNCH_TOTAL_STEPS), Some("running_game"));
    Ok(Some(process_id))
}

async fn apply_roundmcdev_winegdk_prerequisites(
    runner: &crate::core::linux_runtime::Runner,
    wine_prefix: &Path,
    task_id: &str,
) -> Result<(), String> {
    let proton_root = crate::core::linux_runtime::runner_runtime_root(runner)
        .ok_or_else(|| "无法确定 RoundMCDev GDK-Proton 目录".to_string())?;
    let wine = proton_root.join("files/bin/wine");
    if !wine.is_file() {
        return Err(format!("RoundMCDev 缺少 wine：{}", wine.display()));
    }
    let registry_values = [
        (
            "HKLM\\Software\\Microsoft\\Windows NT\\CurrentVersion\\OEM",
            "ConsoleMode",
            "REG_DWORD",
            "8",
        ),
        (
            "HKLM\\Software\\Microsoft\\WindowsRuntime\\ActivatableClassId\\Microsoft.Windows.Storage.Pickers.FileOpenPicker",
            "DllPath",
            "REG_SZ",
            r"C:\windows\system32\windows.storage.dll",
        ),
        (
            "HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\\WinHttp",
            "DefaultSecureProtocols",
            "REG_DWORD",
            "2560",
        ),
        (
            "HKLM\\Software\\Microsoft\\SchannelTLS\\Protocols\\TLS 1.3\\Client",
            "DisabledByDefault",
            "REG_DWORD",
            "1",
        ),
        (
            "HKCU\\Environment",
            "MICROSOFT_WINDOWSAPPRUNTIME_BOOTSTRAP_INITIALIZE_SHOWUI",
            "REG_SZ",
            "0",
        ),
        (
            "HKCU\\Environment",
            "MICROSOFT_WINDOWSAPPRUNTIME_BOOTSTRAP_INITIALIZE_FAILFAST",
            "REG_SZ",
            "0",
        ),
        (
            "HKCU\\Environment",
            "MICROSOFT_WINDOWSAPPRUNTIME_DEPLOYMENT_INITIALIZE_ONERRORSHOWUI",
            "REG_SZ",
            "0",
        ),
    ];
    for (key, value_name, value_type, value) in registry_values {
        let mut command = Command::new(&wine);
        command
            .args([
                "reg", "add", key, "/v", value_name, "/t", value_type, "/d", value, "/f",
            ])
            .env("WINEPREFIX", wine_prefix)
            .env("WINEDEBUG", "-all");
        set_runner_ld_library_path(&mut command, runner);
        let output = command
            .output()
            .await
            .map_err(|error| format!("执行 RoundMCDev WineGDK 注册表配置失败：{error}"))?;
        if !output.status.success() {
            return Err(format!(
                "RoundMCDev WineGDK 注册表配置失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    append_task_log(task_id, "已应用 BedrockBoot WineGDK 注册表前置项");
    Ok(())
}

async fn install_roundmcdev_cryptbase(
    runner: &crate::core::linux_runtime::Runner,
    wine_prefix: &Path,
    task_id: &str,
) -> Result<(), String> {
    let Some(proton_root) = crate::core::linux_runtime::runner_runtime_root(runner) else {
        return Err("无法确定 RoundMCDev GDK-Proton 目录".to_string());
    };
    let source = proton_root.join("files/lib/wine/x86_64-windows/cryptbase.dll");
    let destination = wine_prefix.join("drive_c/windows/system32/cryptbase.dll");
    let copied = crate::tasks::runtime::run_io_blocking(move || {
        if !source.is_file() || destination.is_file() {
            return Ok(false);
        }
        let parent = destination
            .parent()
            .ok_or_else(|| "cryptbase.dll 目标路径没有父目录".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 cryptbase.dll 目录失败：{error}"))?;
        std::fs::copy(&source, &destination)
            .map_err(|error| format!("复制 cryptbase.dll 失败：{error}"))?;
        Ok::<bool, String>(true)
    })
    .await
    .map_err(|error| format!("安装 cryptbase.dll 任务失败：{error}"))??;
    if copied {
        append_task_log(task_id, "已将 Proton-GDK cryptbase.dll 写入 Prefix");
    }
    Ok(())
}

async fn apply_roundmcdev_proton_patches(
    runner: &crate::core::linux_runtime::Runner,
    package_path: &Path,
    game_executable: &Path,
    task_id: &str,
) -> Result<(), String> {
    let proton_root = crate::core::linux_runtime::runner_runtime_root(runner)
        .ok_or_else(|| "无法确定 RoundMCDev GDK-Proton 目录".to_string())?;
    let proton_root_for_task = proton_root.clone();
    let package_path = package_path.to_path_buf();
    let game_executable = game_executable.to_path_buf();
    let report = crate::tasks::runtime::run_io_blocking(move || {
        let mut changes = Vec::new();
        let wine_directory = proton_root_for_task.join("files/lib/wine/x86_64-windows");
        if patch_roundmcdev_combase(&wine_directory.join("combase.dll"))? {
            changes.push("combase.RoOriginateErrorW");
        }
        if patch_roundmcdev_ntdll(&wine_directory.join("ntdll.dll"))? {
            changes.push("ntdll exception stubs");
        }
        let http_client = package_path.join("libHttpClient.GDK.dll");
        if patch_roundmcdev_lhc_xcurl_gate(&http_client)? {
            changes.push("libHttpClient XCurl gate");
        }
        if patch_roundmcdev_stack_reserve(&game_executable)? {
            changes.push("game stack reserve");
        }
        Ok::<Vec<&'static str>, String>(changes)
    })
    .await
    .map_err(|error| format!("执行 RoundMCDev patch 任务失败：{error}"))??;
    if report.is_empty() {
        append_task_log(task_id, "BedrockBoot patch 已存在或当前版本无需修改");
    } else {
        append_task_log(
            task_id,
            format!("已应用 BedrockBoot patch：{}", report.join("、")),
        );
    }
    Ok(())
}

fn patch_roundmcdev_combase(path: &Path) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut data =
        std::fs::read(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let Some(offset) = pe_export_file_offset(&data, "RoOriginateErrorW") else {
        return Ok(false);
    };
    let patch = [0x31, 0xC0, 0xC3, 0x90];
    if data.get(offset..offset + patch.len()) == Some(patch.as_slice()) {
        return Ok(false);
    }
    let end = offset
        .checked_add(patch.len())
        .ok_or_else(|| "combase patch 偏移溢出".to_string())?;
    if end > data.len() {
        return Err("combase patch 偏移超出文件范围".to_string());
    }
    data[offset..end].copy_from_slice(&patch);
    std::fs::write(path, data).map_err(|error| format!("写入 {} 失败：{error}", path.display()))?;
    Ok(true)
}

fn patch_roundmcdev_ntdll(path: &Path) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut data =
        std::fs::read(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let signature = [
        0x55, 0x53, 0x48, 0x81, 0xEC, 0xC8, 0x00, 0x00, 0x00, 0x48, 0x8D, 0xAC, 0x24, 0xC0, 0x00,
        0x00, 0x00,
    ];
    let new_stub = [0xB8, 0x02, 0x00, 0x00, 0xC0, 0xC3, 0x90, 0x90];
    let mut changed = false;
    let mut cursor = 0;
    while let Some(offset) = find_bytes(&data, &signature, cursor) {
        let call_offset = offset + signature.len();
        let matches_funnel = data.get(call_offset..call_offset + 9).is_some_and(|bytes| {
            bytes.starts_with(&[0x48, 0x89, 0xD9, 0xE8]) && bytes.get(7..9) == Some(&[0xEB, 0xF6])
        });
        if matches_funnel {
            let end = offset + new_stub.len();
            if end > data.len() {
                return Err("ntdll patch 偏移超出文件范围".to_string());
            }
            if data[offset..end] != new_stub {
                data[offset..end].copy_from_slice(&new_stub);
                changed = true;
            }
        }
        cursor = offset + signature.len();
    }
    if changed {
        std::fs::write(path, data)
            .map_err(|error| format!("写入 {} 失败：{error}", path.display()))?;
    }
    Ok(changed)
}

fn patch_roundmcdev_lhc_xcurl_gate(path: &Path) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut data =
        std::fs::read(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let gate = [
        0x83, 0xC0, 0xFE, 0xBA, 0x04, 0x00, 0x00, 0x00, 0x48, 0x8D, 0x0D,
    ];
    let compare = [0x83, 0xF8, 0x06];
    let Some(gate_offset) = find_bytes(&data, &gate, 0) else {
        return Ok(false);
    };
    let Some(compare_offset) = find_bytes(&data, &compare, gate_offset + gate.len()) else {
        return Ok(false);
    };
    let patch_offset = compare_offset + compare.len();
    let end = patch_offset + 6;
    if end > data.len() {
        return Err("libHttpClient patch 偏移超出文件范围".to_string());
    }
    if data[patch_offset..end] == [0x90; 6] {
        return Ok(false);
    }
    data[patch_offset..end].fill(0x90);
    std::fs::write(path, data).map_err(|error| format!("写入 {} 失败：{error}", path.display()))?;
    Ok(true)
}

fn patch_roundmcdev_stack_reserve(path: &Path) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut data =
        std::fs::read(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let Some(nt_offset) = pe_nt_offset(&data) else {
        return Ok(false);
    };
    let optional_offset = nt_offset + 4 + 20;
    if read_u16(&data, optional_offset) != Some(0x20B) {
        return Ok(false);
    }
    let field_offset = optional_offset + 72;
    let Some(current) = read_u64(&data, field_offset) else {
        return Ok(false);
    };
    const TARGET_STACK_RESERVE: u64 = 0x1000000;
    if current >= TARGET_STACK_RESERVE {
        return Ok(false);
    }
    let end = field_offset + 8;
    if end > data.len() {
        return Err("游戏 stack reserve 偏移超出文件范围".to_string());
    }
    data[field_offset..end].copy_from_slice(&TARGET_STACK_RESERVE.to_le_bytes());
    std::fs::write(path, data).map_err(|error| format!("写入 {} 失败：{error}", path.display()))?;
    Ok(true)
}

fn find_bytes(data: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= data.len() || needle.len() > data.len() {
        return None;
    }
    data[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn pe_nt_offset(data: &[u8]) -> Option<usize> {
    let nt_offset = usize::try_from(read_u32(data, 0x3C)?).ok()?;
    (read_u32(data, nt_offset) == Some(0x0000_4550)).then_some(nt_offset)
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    data.get(offset..offset + 8).map(|bytes| {
        u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    })
}

fn pe_rva_to_file_offset(data: &[u8], rva: u32) -> Option<usize> {
    let nt_offset = pe_nt_offset(data)?;
    let section_count = usize::from(read_u16(data, nt_offset + 6)?);
    let optional_size = usize::from(read_u16(data, nt_offset + 20)?);
    let section_table = nt_offset.checked_add(24)?.checked_add(optional_size)?;
    for index in 0..section_count {
        let offset = section_table.checked_add(index.checked_mul(40)?)?;
        let virtual_size = read_u32(data, offset + 8)?;
        let virtual_address = read_u32(data, offset + 12)?;
        let raw_size = read_u32(data, offset + 16)?;
        let raw_pointer = read_u32(data, offset + 20)?;
        let section_size = virtual_size.max(raw_size);
        if rva >= virtual_address && rva - virtual_address < section_size {
            let file_offset =
                usize::try_from(raw_pointer.checked_add(rva - virtual_address)?).ok()?;
            return (file_offset < data.len()).then_some(file_offset);
        }
    }
    None
}

fn pe_export_file_offset(data: &[u8], export_name: &str) -> Option<usize> {
    let nt_offset = pe_nt_offset(data)?;
    let optional_offset = nt_offset + 24;
    let export_rva = read_u32(data, optional_offset + 96)?;
    let export_offset = pe_rva_to_file_offset(data, export_rva)?;
    let name_count = usize::try_from(read_u32(data, export_offset + 24)?).ok()?;
    let functions_rva = read_u32(data, export_offset + 28)?;
    let names_rva = read_u32(data, export_offset + 32)?;
    let ordinals_rva = read_u32(data, export_offset + 36)?;
    let names_offset = pe_rva_to_file_offset(data, names_rva)?;
    let ordinals_offset = pe_rva_to_file_offset(data, ordinals_rva)?;
    let functions_offset = pe_rva_to_file_offset(data, functions_rva)?;
    for index in 0..name_count {
        let name_rva = read_u32(data, names_offset + index * 4)?;
        let name_offset = pe_rva_to_file_offset(data, name_rva)?;
        let end = data
            .get(name_offset..)?
            .iter()
            .position(|byte| *byte == 0)
            .map(|length| name_offset + length)?;
        if data.get(name_offset..end)? != export_name.as_bytes() {
            continue;
        }
        let ordinal = usize::from(read_u16(data, ordinals_offset + index * 2)?);
        let function_rva = read_u32(data, functions_offset + ordinal * 4)?;
        return pe_rva_to_file_offset(data, function_rva);
    }
    None
}

async fn apply_roundmcdev_game_fixes(
    runner: &crate::core::linux_runtime::Runner,
    game_executable: &Path,
    task_id: &str,
) -> Result<(), String> {
    let Some(bundle_root) = crate::core::linux_runtime::roundmcdev_bundle_root(runner) else {
        return Ok(());
    };

    let game_fix = bundle_root.join("gameFix");
    let game_patch = bundle_root.join("GamePatch/gdk/mcpatcher_core.dll");
    let game_directory = game_executable
        .parent()
        .ok_or_else(|| "游戏 EXE 没有有效的父目录".to_string())?
        .to_path_buf();
    let game_directory_for_log = game_directory.clone();
    let bundle_root_for_log = bundle_root.clone();
    let stdio_workaround_enabled = crate::tasks::runtime::run_io_blocking(move || {
        copy_directory_contents(&game_fix, &game_directory)
            .map_err(|error| format!("复制 RoundMCDev gameFix 失败：{error}"))?;
        install_roundmcdev_bloader_mod(
            &game_directory,
            &game_patch,
            ROUNDMCDEV_BLOADER_MOD_VERSION,
        )?;
        remove_legacy_roundmcdev_preload(&game_directory)?;
        configure_bloader_linux_stdio_workaround(
            &game_directory,
            bloader::embedded_version_string(),
        )
    })
    .await
    .map_err(|error| format!("应用 RoundMCDev 游戏修复任务失败：{error}"))??;
    if stdio_workaround_enabled {
        append_task_log(
            task_id,
            "已启用 BLoader 0.2.11 Linux 无窗口日志递归兼容模式",
        );
    }
    append_task_log(
        task_id,
        format!(
            "已应用 RoundMCDev 游戏修复到 {}：{}",
            game_directory_for_log.display(),
            bundle_root_for_log.display()
        ),
    );
    Ok(())
}

const ROUNDMCDEV_BLOADER_MOD_DIRECTORY: &str = "roundmcdev-game-patch";
const ROUNDMCDEV_BLOADER_MOD_ENTRY: &str = "mcpatcher_core.dll";
const ROUNDMCDEV_BLOADER_MOD_VERSION: &str = "Release10-32";
const BLOADER_STDIO_RECURSION_VERSION_PREFIX: &str = "0.2.11";
const BLOADER_LEGACY_STDIO_WORKAROUND_KEY: &str = "_bmcbl_linux_stdio_workaround";
const BLOADER_LEGACY_ORIGINAL_DEBUG_CONSOLE_KEY: &str = "_bmcbl_original_enable_debug_console";
const BLOADER_PROCESS_CAPTURE_DIRECTORY: &str = "logs/captured-stdio";
const BLOADER_PROCESS_STDOUT_CAPTURE_NAME: &str = "process-stdout.raw.log";
const BLOADER_PROCESS_CAPTURE_BLOCKER_MARKER: &str = ".bmcbl-disable-recursive-capture";

fn install_roundmcdev_bloader_mod(
    game_directory: &Path,
    source_dll: &Path,
    release_tag: &str,
) -> Result<(), String> {
    if !source_dll.is_file() {
        return Err(format!(
            "RoundMCDev GamePatch 不存在：{}",
            source_dll.display()
        ));
    }
    let mod_directory = game_directory
        .join("mods")
        .join(ROUNDMCDEV_BLOADER_MOD_DIRECTORY);
    std::fs::create_dir_all(&mod_directory)
        .map_err(|error| format!("创建 BLoader 原生 Mod 目录失败：{error}"))?;
    let target_dll = mod_directory.join(ROUNDMCDEV_BLOADER_MOD_ENTRY);
    std::fs::copy(source_dll, &target_dll)
        .map_err(|error| format!("部署 BLoader GamePatch DLL 失败：{error}"))?;

    let manifest = serde_json::json!({
        "id": "roundmcdev.game-patch",
        "name": "RoundMCDev GamePatch",
        "entry": ROUNDMCDEV_BLOADER_MOD_ENTRY,
        "type": "native",
        "version": release_tag,
        "author": "RoundMCDev",
        "description": "Minecraft Bedrock GDK 网络兼容补丁",
        "required": true,
        "notify_success": false,
    });
    let manifest_contents = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("生成 BLoader GamePatch 清单失败：{error}"))?;
    std::fs::write(mod_directory.join("manifest.json"), manifest_contents)
        .map_err(|error| format!("写入 BLoader GamePatch 清单失败：{error}"))?;
    Ok(())
}

fn remove_legacy_roundmcdev_preload(game_directory: &Path) -> Result<(), String> {
    let legacy_path = game_directory.join("preload/mcpatcher_core.dll");
    if !legacy_path.is_file() {
        return Ok(());
    }
    std::fs::remove_file(&legacy_path)
        .map_err(|error| format!("清理错误启动链路遗留的 preload GamePatch 失败：{error}"))
}

fn configure_bloader_linux_stdio_workaround(
    game_directory: &Path,
    bloader_version: &str,
) -> Result<bool, String> {
    let config_path = game_directory.join("config.json");
    let debug_console_enabled = restore_legacy_bloader_stdio_workaround(&config_path)?;
    let vulnerable = bloader_version.starts_with(BLOADER_STDIO_RECURSION_VERSION_PREFIX);
    let capture_path = game_directory
        .join(BLOADER_PROCESS_CAPTURE_DIRECTORY)
        .join(BLOADER_PROCESS_STDOUT_CAPTURE_NAME);

    if !vulnerable || debug_console_enabled {
        remove_bloader_process_capture_blocker(&capture_path)?;
        return Ok(false);
    }

    install_bloader_process_capture_blocker(&capture_path)?;
    Ok(true)
}

fn restore_legacy_bloader_stdio_workaround(config_path: &Path) -> Result<bool, String> {
    if !config_path.is_file() {
        return Ok(false);
    }
    let contents =
        std::fs::read(config_path).map_err(|error| format!("读取 BLoader 配置失败：{error}"))?;
    let mut config = serde_json::from_slice::<serde_json::Value>(&contents)
        .map_err(|error| format!("解析 BLoader 配置失败：{error}"))?;
    let config = config
        .as_object_mut()
        .ok_or_else(|| "BLoader config.json 根节点不是对象".to_string())?;
    let legacy_workaround = config.remove(BLOADER_LEGACY_STDIO_WORKAROUND_KEY);
    let legacy_original_debug_console = config.remove(BLOADER_LEGACY_ORIGINAL_DEBUG_CONSOLE_KEY);
    let legacy_config_changed =
        legacy_workaround.is_some() || legacy_original_debug_console.is_some();
    let legacy_workaround_enabled = legacy_workaround
        .as_ref()
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if legacy_workaround_enabled {
        let original = legacy_original_debug_console
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        config.insert(
            "enable_debug_console".to_string(),
            serde_json::Value::Bool(original),
        );
    }
    let debug_console_enabled = config
        .get("enable_debug_console")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !legacy_config_changed {
        return Ok(debug_console_enabled);
    }

    let contents = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("生成 BLoader 配置失败：{error}"))?;
    let config_directory = config_path
        .parent()
        .ok_or_else(|| "BLoader 配置没有有效的父目录".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(config_directory)
        .map_err(|error| format!("创建 BLoader 临时配置失败：{error}"))?;
    use std::io::Write as _;
    temporary
        .write_all(&contents)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("写入 BLoader 临时配置失败：{error}"))?;
    temporary
        .persist(config_path)
        .map_err(|error| format!("保存 BLoader 配置失败：{}", error.error))?;
    Ok(debug_console_enabled)
}

fn install_bloader_process_capture_blocker(capture_path: &Path) -> Result<(), String> {
    let capture_directory = capture_path
        .parent()
        .ok_or_else(|| "BLoader stdout 捕获路径没有有效的父目录".to_string())?;
    std::fs::create_dir_all(capture_directory)
        .map_err(|error| format!("创建 BLoader stdout 捕获目录失败：{error}"))?;

    match std::fs::symlink_metadata(capture_path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => std::fs::remove_file(capture_path)
            .map_err(|error| format!("清理 BLoader 递归 stdout 日志失败：{error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("检查 BLoader stdout 捕获路径失败：{error}")),
    }
    std::fs::create_dir_all(capture_path)
        .map_err(|error| format!("创建 BLoader stdout 捕获阻断目录失败：{error}"))?;
    std::fs::write(
        capture_path.join(BLOADER_PROCESS_CAPTURE_BLOCKER_MARKER),
        b"BMCBL blocks BLoader 0.2.11 recursive process stdout capture on Linux.\n",
    )
    .map_err(|error| format!("写入 BLoader stdout 捕获阻断标记失败：{error}"))
}

fn remove_bloader_process_capture_blocker(capture_path: &Path) -> Result<(), String> {
    let marker_path = capture_path.join(BLOADER_PROCESS_CAPTURE_BLOCKER_MARKER);
    if !marker_path.is_file() {
        return Ok(());
    }
    std::fs::remove_file(&marker_path)
        .map_err(|error| format!("移除 BLoader stdout 捕获阻断标记失败：{error}"))?;
    std::fs::remove_dir(capture_path)
        .map_err(|error| format!("移除 BLoader stdout 捕获阻断目录失败：{error}"))
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!("源目录不存在：{}", source.display()));
    }
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("创建目标目录 {} 失败：{error}", destination.display()))?;
    for entry in std::fs::read_dir(source)
        .map_err(|error| format!("读取源目录 {} 失败：{error}", source.display()))?
    {
        let entry = entry.map_err(|error| format!("读取修复文件条目失败：{error}"))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory_contents(&source_path, &destination_path)?;
        } else {
            std::fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "复制 {} 到 {} 失败：{error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

async fn stop_lingering_proton_processes(
    runner: &crate::core::linux_runtime::Runner,
    prefix_path: &Path,
    task_id: &str,
) -> Result<(), String> {
    let proton_root =
        runner_runtime_root(runner).ok_or_else(|| "无法确定 Proton-GDK 安装目录".to_string())?;
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
    let mut command = Command::new(&wineserver);
    set_runner_ld_library_path(&mut command, runner);
    command
        .arg("-k")
        .arg("-w")
        .env("WINEPREFIX", proton_wine_prefix_path(prefix_path))
        .stdin(Stdio::null());
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
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
    let wine_prefix = proton_wine_prefix_path(prefix_path);
    if proton_game_input_is_ready_at(&wine_prefix).await? {
        tokio::fs::write(&marker, b"installed\n")
            .await
            .map_err(|error| format!("写入 Proton-GDK GameInput 状态失败：{error}"))?;
        append_task_log(
            task_id,
            "已在 Proton prefix 中检测到 GameInput，跳过重复安装",
        );
        return Ok(());
    }
    if marker.is_file() {
        tokio::fs::remove_file(&marker)
            .await
            .map_err(|error| format!("清理失效的 Proton-GDK GameInput 状态失败：{error}"))?;
        append_task_log(task_id, "检测到失效的 GameInput 状态，正在重新安装");
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
    let mut command = if runner.kind == RunnerKind::Umu {
        let proton_root = runner_runtime_root(runner)
            .ok_or_else(|| "无法确定 RoundMCDev Proton-GDK 目录".to_string())?;
        let wine = proton_root.join("files/bin/wine");
        if !wine.is_file() {
            return Err(format!("Proton-GDK 中没有找到 wine：{}", wine.display()));
        }
        let mut command = Command::new(wine);
        command
            .env("WINEPREFIX", &wine_prefix)
            .env("WINEDEBUG", "-all")
            .env("WINEDLLOVERRIDES", "advapi32=n,b");
        set_runner_ld_library_path(&mut command, runner);
        command
    } else {
        let mut command = Command::new(&runner.executable);
        configure_proton_command(&mut command, runner, prefix_path, task_id).await?;
        command.arg("runinprefix");
        command
    };
    let windows_installer = wine_z_path(normalized_installer)?;
    command
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
    let mut stdout_pump = child
        .stdout
        .take()
        .map(|stdout| {
            spawn_output_pump(
                task_id.to_string(),
                stdout,
                false,
                recent_output.clone(),
                Some(failure_sender.clone()),
                None,
            )
        })
        .transpose()?;
    let mut stderr_pump = child
        .stderr
        .take()
        .map(|stderr| {
            spawn_output_pump(
                task_id.to_string(),
                stderr,
                true,
                recent_output.clone(),
                Some(failure_sender),
                None,
            )
        })
        .transpose()?;

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
            finish_output_pumps(task_id, stdout_pump.take(), stderr_pump.take()).await;
            return Err(failure);
        }
        InstallOutcome::TimedOut => {
            if let Err(error) = child.kill().await {
                append_task_log(task_id, format!("终止超时的 GameInput 安装器失败：{error}"));
            }
            finish_output_pumps(task_id, stdout_pump.take(), stderr_pump.take()).await;
            return Err(
                "GameInput 安装超时；已停止启动，避免使用缺少原生 GameInput 的 Prefix".to_string(),
            );
        }
    };
    finish_output_pumps(task_id, stdout_pump.take(), stderr_pump.take()).await;
    if !status.success() {
        let detail = recent_runner_output(&recent_output);
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!("\n{detail}")
        };
        return Err(format!(
            "GameInput 安装失败，退出代码 {}{detail}",
            status.code().unwrap_or(-1)
        ));
    }

    append_task_log(task_id, "等待 Proton 写入 GameInput 文件与注册状态");
    if !wait_for_proton_game_input_ready_at(&wine_prefix, GAME_INPUT_REGISTRATION_TIMEOUT).await? {
        return Err(format!(
            "GameInput 安装器已退出，但未检测到原生 GameInputRedist.dll、服务程序或注册表；Prefix：{}",
            wine_prefix.display()
        ));
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

async fn proton_game_input_is_ready(prefix_path: &Path) -> Result<bool, String> {
    proton_game_input_is_ready_at(&prefix_path.join("pfx")).await
}

async fn proton_game_input_is_ready_at(prefix: &Path) -> Result<bool, String> {
    let game_input_directory = prefix.join("drive_c/Program Files/Microsoft GameInput/x64");
    if !game_input_directory.join("GameInputRedist.dll").is_file()
        || !game_input_directory
            .join("GameInputRedistService.exe")
            .is_file()
    {
        return Ok(false);
    }

    for registry_path in [prefix.join("system.reg"), prefix.join("user.reg")] {
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

async fn wait_for_proton_game_input_ready(
    prefix_path: &Path,
    timeout: Duration,
) -> Result<bool, String> {
    wait_for_proton_game_input_ready_at(&prefix_path.join("pfx"), timeout).await
}

async fn wait_for_proton_game_input_ready_at(
    prefix: &Path,
    timeout: Duration,
) -> Result<bool, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if proton_game_input_is_ready_at(prefix).await? {
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
    Ok(crate::core::minecraft::paths::compatibility_prefix_dir(
        folder_name,
    ))
}

fn sanitize_instance_folder_name(folder_name: &str) -> String {
    crate::core::minecraft::paths::sanitize_compatibility_prefix_name(folder_name)
}

fn spawn_output_pump<R>(
    task_id: String,
    reader: R,
    is_error: bool,
    recent_output: Arc<Mutex<VecDeque<String>>>,
    failure_sender: Option<mpsc::UnboundedSender<String>>,
    publish_to_task: Option<Arc<AtomicBool>>,
) -> Result<tokio::task::JoinHandle<()>, String>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    crate::tasks::runtime::spawn_io(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let prefix = if is_error { "runner stderr" } else { "runner" };
                    let displayed_line = normalize_runner_output_line(&line);
                    let log_line = format!("{prefix}: {displayed_line}");
                    if publish_to_task
                        .as_ref()
                        .is_none_or(|publish| publish.load(Ordering::Acquire))
                    {
                        append_task_log(&task_id, log_line.clone());
                    } else if is_error {
                        debug!(task_id, line = %displayed_line, "compatibility runner stderr");
                    }
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
            "当前 Proton-GDK 的 combase.dll 没有实现 RoOriginateErrorW，无法启动该 Minecraft 版本。请在 Proton-GDK 设置中安装并选择支持登录的 RoundMCDev 版本"
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

fn spawn_process_monitor(
    task_id: String,
    mut child: tokio::process::Child,
    stdout_pump: Option<tokio::task::JoinHandle<()>>,
    stderr_pump: Option<tokio::task::JoinHandle<()>>,
    recent_output: Arc<Mutex<VecDeque<String>>>,
    _launch_auth: Option<crate::core::bedrock_auth::PreparedLaunchAuth>,
    mut session: Option<crate::core::version::game_info::GameSession>,
) {
    let task_id_for_monitor = task_id.clone();
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        let task_id = task_id_for_monitor;
        let mut checkpoint = tokio::time::interval(Duration::from_secs(5 * 60));
        checkpoint.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        checkpoint.tick().await;
        let process_status = loop {
            tokio::select! {
                status = child.wait() => break status,
                _ = checkpoint.tick() => {
                    if let Some(session) = session.as_mut()
                        && let Err(error) = session.checkpoint().await
                    {
                        append_task_log(&task_id, format!("定时保存游戏时间失败：{error}"));
                    }
                }
            }
        };
        match process_status {
            Ok(status) => {
                finish_output_pumps(&task_id, stdout_pump, stderr_pump).await;
                if !status.success() {
                    let output = recent_runner_output(&recent_output);
                    warn!(
                        task_id,
                        %status,
                        recent_output = %output,
                        "compatibility runner exited with failure after successful launch"
                    );
                } else {
                    info!(task_id, %status, "compatibility runner exited");
                }
            }
            Err(error) => {
                warn!(task_id, %error, "failed to wait for compatibility runner process");
            }
        };
        if let Some(session) = session
            && let Err(error) = session.finish().await
        {
            append_task_log(&task_id, format!("保存游戏时间失败：{error}"));
        }
    }) {
        warn!(task_id, %error, "failed to schedule compatibility process monitor");
    }
}

#[cfg(test)]
#[path = "task_linux_tests.rs"]
mod tests;
