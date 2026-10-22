#![expect(
    unsafe_code,
    reason = "startup singleton and foreground handling use native Windows handles"
)]

use crate::cli::CliAction;
use crate::core::version::api::get_version_list;
use crate::core::version::launch_versions::{LaunchVersionEntry, select_launch_version};
use crate::launch::{
    DirectLaunchContext, ImportLaunchContext, LaunchMode, LaunchOptions, LaunchTarget,
    LaunchUiMode, UpdaterLaunchContext,
};
use crate::startup_args::{StartupAction, TerminalHostCommand};
use crate::tasks::runtime::AppRuntime;
use anyhow::{Context as _, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{env, process};
use tracing::{debug, error, info, warn};

#[cfg(windows)]
const SINGLE_INSTANCE_MUTEX_NAME: &str = "Global\\com.bmcbl.app.single_instance";

#[cfg(windows)]
fn bring_main_window_to_foreground() {
    use std::ffi::OsStr;
    #[cfg(target_os = "windows")]
    use std::os::windows::ffi::OsStrExt;
    use tracing::warn;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
    };
    use windows::core::PCWSTR;

    let window_title = crate::utils::app_info::runtime_app_name();
    let wide_window_title: Vec<u16> = OsStr::new(&window_title)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: wide_window_title is NUL-terminated and remains alive for the call.
    let hwnd = match unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide_window_title.as_ptr())) } {
        Ok(hwnd) => hwnd,
        Err(error) => {
            warn!(?error, window_title = %window_title, "could not find existing main window");
            return;
        }
    };

    // SAFETY: hwnd came from FindWindowW and is only queried for its iconic state.
    if unsafe { IsIconic(hwnd).as_bool() } {
        // SAFETY: hwnd came from FindWindowW; restoring it is best-effort.
        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
    }

    // SAFETY: hwnd came from FindWindowW; foreground activation is best-effort.
    let _ = unsafe { SetForegroundWindow(hwnd) };
    info!(window_title = %window_title, "Brought existing main window to foreground");
}

#[cfg(windows)]
struct SingleInstanceGuard;

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        crate::utils::single_instance::release_mutex();
    }
}

#[cfg(windows)]
fn check_single_instance() -> Option<bool> {
    use std::ffi::OsStr;
    #[cfg(target_os = "windows")]
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;

    let wide_name: Vec<u16> = OsStr::new(SINGLE_INSTANCE_MUTEX_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: wide_name is NUL-terminated and remains alive for the call.
    let mutex_handle = match unsafe { CreateMutexW(None, true, PCWSTR(wide_name.as_ptr())) } {
        Ok(handle) => handle,
        Err(error) => {
            error!(?error, "failed to create single instance mutex");
            return None;
        }
    };

    // SAFETY: GetLastError reads the thread-local Win32 error after CreateMutexW.
    if unsafe { GetLastError() }.0 == ERROR_ALREADY_EXISTS.0 {
        // SAFETY: mutex_handle is the valid handle returned by CreateMutexW.
        let _ = unsafe { CloseHandle(mutex_handle) };
        bring_main_window_to_foreground();
        return Some(false);
    }

    crate::utils::single_instance::set_mutex_handle(mutex_handle);
    Some(true)
}

#[cfg(windows)]
fn single_instance_guard(launch_mode: &LaunchMode) -> Option<SingleInstanceGuard> {
    let opens_main_window = matches!(launch_mode, LaunchMode::Main);
    let opens_gui_launch = launch_mode
        .direct_launch()
        .is_some_and(|context| !context.silent);
    if !opens_main_window && !opens_gui_launch {
        return None;
    }

    match check_single_instance() {
        Some(true) => Some(SingleInstanceGuard),
        Some(false) if opens_main_window => {
            info!(
                "Another instance is already running. Brought main window to foreground and exiting."
            );
            process::exit(0);
        }
        Some(false) => {
            let message = "已有 BMCBL 实例正在运行，无法在 GUI 模式启动新实例。请先关闭现有实例，或使用 --silent 以静默方式启动。";
            error!("{message}");
            eprintln!("{message}");
            crate::result::show_startup_failure(
                "启动失败 - 已有实例",
                "single_instance_guard",
                message,
            );
            process::exit(1);
        }
        None => {
            error!("Single instance check failed, continuing anyway.");
            Some(SingleInstanceGuard)
        }
    }
}

pub(crate) enum Invocation {
    Startup(StartupAction),
    Cli(CliAction),
}

pub(crate) fn run(invocation: Invocation) -> Result<()> {
    let invocation = match invocation {
        Invocation::Startup(StartupAction::TerminalHost(command)) => {
            return run_terminal_host(command);
        }
        Invocation::Cli(CliAction::Info { json }) => return run_info(json),
        invocation => invocation,
    };

    let startup_started = Instant::now();
    crate::utils::memory::configure_mimalloc_optimizer();
    let runtime = crate::tasks::runtime::initialize_app_runtime()?;

    let launch_mode = match invocation {
        Invocation::Cli(CliAction::List { json }) => {
            crate::utils::file_ops::create_initial_directories();
            crate::utils::logger::init_logging(
                false,
                false,
                &crate::config::config::LogManagementConfig::default(),
                false,
            );
            return run_list(runtime, json);
        }
        Invocation::Startup(action) => resolve_startup_mode(runtime, action)?,
        Invocation::Cli(action) => resolve_cli_mode(runtime, action)?,
    };

    if let Some(working_dir) = launch_working_dir(&launch_mode)
        && let Err(error) = env::set_current_dir(&working_dir)
    {
        eprintln!(
            "Failed to set working directory to {}: {error}",
            working_dir.display()
        );
    }

    #[cfg(windows)]
    let _single_instance_guard = single_instance_guard(&launch_mode);

    if let LaunchMode::Updater(context) = &launch_mode {
        crate::utils::file_ops::create_initial_directories();
        crate::utils::logger::init_logging(
            false,
            false,
            &crate::config::config::LogManagementConfig::default(),
            false,
        );
        return run_updater_mode(context);
    }

    #[cfg(target_os = "linux")]
    refuse_root_launch()?;

    let is_headless_launch = launch_mode
        .direct_launch()
        .is_some_and(|context| context.silent);
    crate::utils::file_ops::create_initial_directories();
    let config = match crate::config::config::initialize_config_cache() {
        Ok(config) => config,
        Err(error) => {
            let message = format!("读取配置失败: {error:?}\n程序将退出。");
            eprintln!("{message}");
            if !is_headless_launch {
                crate::result::show_startup_failure(
                    "启动失败 - 读取配置",
                    "initialize_config_cache",
                    &message,
                );
            }
            process::exit(1);
        }
    };
    crate::utils::logger::init_logging(
        config.launcher.debug,
        config.launcher.gpui_trace,
        &config.launcher.log_management,
        !is_headless_launch,
    );
    debug!(
        elapsed_ms = startup_started.elapsed().as_millis(),
        debug_enabled = config.launcher.debug,
        gpui_trace_enabled = config.launcher.gpui_trace,
        language = %config.launcher.language,
        renderer_backend = %config.launcher.renderer_backend,
        "configuration loaded and debug logging state applied"
    );

    if launch_mode.is_main() {
        crate::core::bedrock_auth::preload_at_app_startup();
        debug!("system-local Xbox probe and BMCBL managed-account restore scheduled independently");
    }

    if let Some(context) = launch_mode.direct_launch()
        && context.silent
    {
        return run_headless_launch(runtime, context);
    }

    if launch_mode.is_main() && config.launcher.stats_upload {
        if let Err(error) = crate::utils::telemetry::spawn_startup_telemetry(&config) {
            warn!(%error, "failed to schedule startup telemetry");
        }
    }

    if launch_mode.is_main() {
        spawn_noncritical_startup_work();
    } else {
        info!("Import-mode preinit done");
    }

    let bootstrap = runtime.block_on(crate::app::AppBootstrap::from_config(config, launch_mode));
    info!(
        elapsed_ms = startup_started.elapsed().as_millis(),
        "startup critical path complete; entering GPUI"
    );
    ensure_gpui_outside_tokio_runtime()?;
    // Tokio work crosses into GPUI through the runtime bridge; the event loop owns
    // the main thread and must never inherit a Tokio task's cooperative budget.
    crate::app::run(bootstrap)?;

    // 优雅退出遥测客户端，排空在途遥测数据
    crate::utils::telemetry::shutdown();

    // 配置写盘为 ~500ms 合并延迟，退出前把未落盘的改动立即写入磁盘。
    crate::config::config::flush_config_now();

    // 异步日志线程在退出前排空队列，避免丢失退出前的诊断信息。
    crate::utils::log_manager::flush_async_logs_now();

    Ok(())
}

fn resolve_startup_mode(runtime: &AppRuntime, action: StartupAction) -> Result<LaunchMode> {
    match action {
        StartupAction::Gui => Ok(LaunchMode::Main),
        StartupAction::Launch(options) => Ok(LaunchMode::DirectLaunch(resolve_direct_launch(
            runtime, options,
        )?)),
        StartupAction::Import(file_path) => Ok(LaunchMode::Import(ImportLaunchContext {
            file_path: absolute_path(file_path),
        })),
        StartupAction::Updater(command) => Ok(LaunchMode::Updater(UpdaterLaunchContext {
            source_path: absolute_path(command.source_path),
            destination_path: absolute_path(command.destination_path),
            timeout_secs: command.timeout_secs,
        })),
        StartupAction::TerminalHost(_) => {
            Err(anyhow::anyhow!("terminal-host 命令已在启动流程前处理"))
        }
    }
}

fn resolve_cli_mode(runtime: &AppRuntime, action: CliAction) -> Result<LaunchMode> {
    match action {
        CliAction::Launch(options) => Ok(LaunchMode::DirectLaunch(resolve_direct_launch(
            runtime, options,
        )?)),
        CliAction::Import(file_path) => Ok(LaunchMode::Import(ImportLaunchContext {
            file_path: absolute_path(file_path),
        })),
        CliAction::List { .. } | CliAction::Info { .. } => {
            Err(anyhow::anyhow!("该命令已在启动流程前处理"))
        }
    }
}

fn resolve_direct_launch(
    runtime: &AppRuntime,
    options: LaunchOptions,
) -> Result<DirectLaunchContext> {
    // The version directory is created on first run; scanning before it
    // exists would report an IO error instead of "no version installed".
    crate::utils::file_ops::create_initial_directories();
    let versions = runtime
        .block_on(get_version_list())
        .context("读取本地版本列表失败")?;
    let entry =
        select_launch_version(&versions, options.version.as_deref()).map_err(anyhow::Error::msg)?;
    let target = launch_target_from_entry(entry);
    let silent = match options.ui_mode {
        LaunchUiMode::Silent => true,
        LaunchUiMode::Gui => false,
        LaunchUiMode::Auto => {
            runtime
                .block_on(crate::core::version::settings::get_version_config(
                    target.folder.clone(),
                ))
                .unwrap_or_else(|error| {
                    warn!(
                        version_folder = %target.folder,
                        %error,
                        "failed to read instance launch mode; using the default"
                    );
                    crate::core::version::settings::VersionConfig::default()
                })
                .shortcut_silent_launch
        }
    };
    info!(
        version_folder = %target.folder,
        version = %target.version,
        silent,
        has_game_args = options.game_args.is_some(),
        "resolved launch target"
    );
    Ok(DirectLaunchContext {
        target,
        silent,
        game_args: options.game_args,
        json: options.json,
    })
}

/// Resolves user-provided paths before startup changes the process working
/// directory to the executable directory.
fn absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    env::current_dir()
        .map(|current_dir| current_dir.join(&path))
        .unwrap_or(path)
}

fn launch_target_from_entry(entry: &LaunchVersionEntry) -> LaunchTarget {
    LaunchTarget {
        folder: entry.folder.to_string(),
        name: entry.name.to_string(),
        version: entry.version.to_string(),
        kind: entry.kind.to_string(),
        package_folder: entry.path.to_string(),
    }
}

fn run_headless_launch(runtime: &AppRuntime, context: &DirectLaunchContext) -> Result<()> {
    let target = &context.target;
    info!(
        version_folder = %target.folder,
        version = %target.version,
        "执行命令行静默启动"
    );

    let request = crate::core::minecraft::launcher::LaunchRequest {
        launch_args: context
            .game_args
            .as_ref()
            .map(|value| Arc::<str>::from(value.as_str())),
        ..crate::core::minecraft::launcher::LaunchRequest::new(
            target.folder.clone(),
            target.name.clone(),
            target.version.clone(),
            target.package_folder.clone(),
        )
    };
    let launch = crate::core::minecraft::launcher::start_launch_task_observed(request);
    let task_id = launch.task_id().to_string();
    let outcome = runtime.block_on(launch.wait());

    let result = match outcome {
        Ok(Some(pid)) if pid != 0 => {
            if context.json {
                println!(
                    "{}",
                    json!({
                        "ok": true,
                        "taskId": task_id,
                        "folder": target.folder,
                        "name": target.name,
                        "version": target.version,
                        "pid": pid,
                    })
                );
            } else {
                println!("已启动 {}（{}，PID {pid}）", target.name, target.version);
            }
            Ok(())
        }
        Ok(_) => {
            if context.json {
                println!(
                    "{}",
                    json!({
                        "ok": true,
                        "taskId": task_id,
                        "folder": target.folder,
                        "name": target.name,
                        "version": target.version,
                        "pid": serde_json::Value::Null,
                    })
                );
            } else {
                println!("启动流程已完成，但未返回游戏进程 PID");
            }
            Ok(())
        }
        Err(error) => {
            let cancelled = crate::tasks::task_manager::get_snapshot_arc(&task_id)
                .is_some_and(|snapshot| snapshot.status.as_ref() == "cancelled");
            let message = if cancelled {
                "启动已取消".to_string()
            } else {
                format!("启动失败：{error}")
            };
            error!(
                task_id = %task_id,
                %message,
                "命令行静默启动失败"
            );
            let logs = crate::tasks::task_manager::task_logs(&task_id);
            for line in logs.iter().rev().take(8).rev() {
                eprintln!("  {line}");
            }
            Err(anyhow::anyhow!(message))
        }
    };

    crate::config::config::flush_config_now();
    crate::utils::log_manager::flush_async_logs_now();
    result
}

fn run_list(runtime: &AppRuntime, json_output: bool) -> Result<()> {
    let versions = runtime
        .block_on(get_version_list())
        .context("读取本地版本列表失败")?;
    if json_output {
        let entries = versions
            .iter()
            .map(|version| {
                json!({
                    "folder": version.folder.as_ref(),
                    "name": version.name.as_ref(),
                    "version": version.version.as_ref(),
                    "kind": version.kind.as_ref(),
                    "path": version.path.as_ref(),
                    "sessions": version.game_info.total_sessions,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if versions.is_empty() {
        println!("未找到已安装的游戏版本");
    } else {
        for version in &versions {
            println!(
                "{}\t{}\t{}\t{}",
                version.folder, version.name, version.version, version.kind
            );
        }
    }
    crate::utils::log_manager::flush_async_logs_now();
    Ok(())
}

fn run_info(json_output: bool) -> Result<()> {
    let executable = env::current_exe().ok();
    let data_dir = crate::utils::file_ops::bmcbl_dir();
    let versions_dir = crate::utils::file_ops::bmcbl_subdir("versions");
    if json_output {
        println!(
            "{}",
            json!({
                "version": crate::utils::app_info::get_version(),
                "channel": crate::utils::app_info::build_channel().as_str(),
                "buildInfo": crate::utils::app_info::get_build_info(),
                "executable": executable.as_ref().map(|path| path.to_string_lossy()),
                "dataDir": data_dir.to_string_lossy(),
                "versionsDir": versions_dir.to_string_lossy(),
            })
        );
    } else {
        println!("BMCBL {}", crate::utils::app_info::get_version());
        println!("{}", crate::utils::app_info::get_build_info());
        if let Some(executable) = executable {
            println!("可执行文件: {}", executable.display());
        }
        println!("数据目录: {}", data_dir.display());
        println!("版本目录: {}", versions_dir.display());
    }
    Ok(())
}

fn run_terminal_host(command: TerminalHostCommand) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        crate::core::windows_terminal::run_host(&command.command_path, &command.response_path)
            .map_err(anyhow::Error::msg)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = command;
        anyhow::bail!("terminal-host 是 Windows 专用内部命令")
    }
}

#[cfg(target_os = "linux")]
fn refuse_root_launch() -> Result<()> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let effective_user_id = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|uids| uids.split_whitespace().nth(1))
        .and_then(|uid| uid.parse::<u32>().ok());

    if effective_user_id == Some(0) {
        anyhow::bail!(
            "请不要以 root 身份运行 BMCBL。请使用普通桌面用户启动；需要安装系统依赖时，BMCBL 将单独请求授权。"
        );
    }

    Ok(())
}

fn ensure_gpui_outside_tokio_runtime() -> Result<()> {
    anyhow::ensure!(
        tokio::runtime::Handle::try_current().is_err(),
        "GPUI event loop must not run inside a Tokio runtime context"
    );
    Ok(())
}

#[cfg(test)]
mod runtime_boundary_tests {
    use super::*;

    #[test]
    fn gpui_event_loop_is_allowed_on_a_plain_thread() {
        assert!(ensure_gpui_outside_tokio_runtime().is_ok());
    }

    #[test]
    fn relative_paths_are_resolved_before_the_working_directory_changes() {
        let resolved = absolute_path(PathBuf::from("pack.mcpack"));
        assert!(resolved.is_absolute());
        assert_eq!(
            resolved,
            std::env::current_dir()
                .expect("current directory should exist")
                .join("pack.mcpack")
        );
    }

    #[test]
    fn gpui_event_loop_is_rejected_inside_a_tokio_task() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test Tokio runtime should build");

        runtime.block_on(async {
            let error = ensure_gpui_outside_tokio_runtime()
                .expect_err("GPUI event loop must not run as a Tokio task");
            assert!(error.to_string().contains("must not run inside"));
        });
    }
}

fn spawn_noncritical_startup_work() {
    let result = crate::tasks::runtime::spawn_io(async {
        if let Err(error) = crate::tasks::runtime::run_io_blocking(|| {
            if let Err(error) = crate::utils::diagnostics::prepare_previous_run_reports() {
                error!(?error, "failed to prepare previous run diagnostics");
            }
            if let Err(error) = crate::utils::diagnostics::mark_session_started() {
                error!(?error, "failed to mark diagnostics session as started");
            }
            crate::utils::updater_child::clean_old_versions();
            #[cfg(target_os = "windows")]
            crate::utils::registry::register_file_associations();
            log_system_info();
        })
        .await
        {
            error!(%error, "noncritical startup work failed");
        }
    });
    if let Err(error) = result {
        error!(?error, "failed to start noncritical startup work");
    } else {
        debug!("noncritical startup work scheduled");
    }
}

fn launch_working_dir(launch_mode: &LaunchMode) -> Option<std::path::PathBuf> {
    match launch_mode {
        LaunchMode::Updater(context) => context
            .destination_path
            .parent()
            .map(std::path::Path::to_path_buf),
        LaunchMode::Main | LaunchMode::Import(_) | LaunchMode::DirectLaunch(_) => {
            env::current_exe()
                .ok()
                .and_then(|exe_path| exe_path.parent().map(std::path::Path::to_path_buf))
        }
    }
}

fn run_updater_mode(context: &crate::launch::UpdaterLaunchContext) -> Result<()> {
    let src = context.source_path.display().to_string();
    let dst = context.destination_path.display().to_string();
    let timeout_secs = context.timeout_secs;

    info!(src = %src, dst = %dst, timeout_secs, "updater-child start");

    let start = std::time::Instant::now();
    match crate::utils::updater_child::run_updater_child(
        Path::new(&src),
        Path::new(&dst),
        Duration::from_secs(timeout_secs),
    ) {
        Ok(()) => {
            let elapsed = start.elapsed();
            info!(
                src = %src,
                dst = %dst,
                elapsed_ms = %elapsed.as_millis(),
                "updater-child success"
            );
            crate::utils::log_manager::flush_async_logs_now();
            process::exit(0);
        }
        Err(error) => {
            error!(src = %src, dst = %dst, error = ?error, "updater-child failed");
            crate::utils::log_manager::flush_async_logs_now();
            process::exit(2);
        }
    }
}

fn log_system_info() {
    let sys_name = sysinfo::System::name().unwrap_or_else(|| "Unknown".to_string());
    let kernel_version = sysinfo::System::kernel_version().unwrap_or_else(|| "Unknown".to_string());
    let os_version = sysinfo::System::os_version().unwrap_or_else(|| "Unknown".to_string());

    info!(
        "Preinit Done. App Path: {:?}",
        env::current_exe().unwrap_or_else(|_| Path::new(".").to_path_buf())
    );
    info!(
        "System Info: Encoding: {} | System: {} | Kernel: {} | OS Version: {} | CPU Architecture: {} | Language: {}",
        crate::utils::system_info::detect_system_encoding(),
        sys_name,
        kernel_version,
        os_version,
        crate::utils::system_info::get_cpu_architecture(),
        crate::utils::system_info::get_system_language()
    );
}
