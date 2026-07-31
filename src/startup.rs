use crate::launch::{LaunchMode, parse_launch_mode};
use anyhow::Result;
use std::path::Path;
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

    let hwnd = match unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide_window_title.as_ptr())) } {
        Ok(hwnd) => hwnd,
        Err(error) => {
            warn!(?error, window_title = %window_title, "could not find existing main window");
            return;
        }
    };

    if unsafe { IsIconic(hwnd).as_bool() } {
        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
    }
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

    let mutex_handle = match unsafe { CreateMutexW(None, true, PCWSTR(wide_name.as_ptr())) } {
        Ok(handle) => handle,
        Err(error) => {
            error!(?error, "failed to create single instance mutex");
            return None;
        }
    };

    if unsafe { GetLastError() }.0 == ERROR_ALREADY_EXISTS.0 {
        let _ = unsafe { CloseHandle(mutex_handle) };
        bring_main_window_to_foreground();
        return Some(false);
    }

    crate::utils::single_instance::set_mutex_handle(mutex_handle);
    Some(true)
}

#[cfg(windows)]
fn single_instance_guard(launch_mode: &LaunchMode) -> Option<SingleInstanceGuard> {
    if matches!(launch_mode, LaunchMode::Main) {
        match check_single_instance() {
            Some(true) => Some(SingleInstanceGuard),
            Some(false) => {
                info!(
                    "Another instance is already running. Brought main window to foreground and exiting."
                );
                process::exit(0);
            }
            None => {
                error!("Single instance check failed, continuing anyway.");
                Some(SingleInstanceGuard)
            }
        }
    } else {
        None
    }
}

pub fn run() -> Result<()> {
    let startup_started = Instant::now();
    crate::utils::memory::configure_mimalloc_optimizer();
    let runtime = crate::tasks::runtime::initialize_app_runtime()?;
    let launch_mode = parse_launch_mode();

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
            &crate::config::config::LogManagementConfig::default(),
        );
        return run_updater_mode(context);
    }

    crate::utils::file_ops::create_initial_directories();
    let config = match crate::config::config::initialize_config_cache() {
        Ok(config) => config,
        Err(error) => {
            let message = format!("读取配置失败: {error:?}\n程序将退出。");
            eprintln!("{message}");
            crate::result::show_startup_failure(
                "启动失败 - 读取配置",
                "initialize_config_cache",
                &message,
            );
            process::exit(1);
        }
    };
    crate::utils::logger::init_logging(config.launcher.debug, &config.launcher.log_management);
    debug!(
        elapsed_ms = startup_started.elapsed().as_millis(),
        debug_enabled = config.launcher.debug,
        language = %config.launcher.language,
        renderer_backend = %config.launcher.renderer_backend,
        "configuration loaded and debug logging state applied"
    );

    if launch_mode.is_main() {
        crate::core::bedrock_auth::preload_at_app_startup();
        debug!(
            "system-local Xbox probe and BMCBL managed-account restore scheduled independently"
        );
    }

    if let LaunchMode::DirectLaunch(ref direct_ctx) = launch_mode {
        let version_config = runtime
            .block_on(crate::core::version::settings::get_version_config(
                direct_ctx.version_folder.clone(),
            ))
            .unwrap_or_default();
        let is_silent = direct_ctx
            .silent_override
            .unwrap_or(version_config.shortcut_silent_launch);
        if is_silent {
            info!(
                version_folder = %direct_ctx.version_folder,
                "执行 CMD/快捷方式静默启动"
            );
            return runtime.block_on(run_silent_direct_launch(&direct_ctx.version_folder));
        }
    }

    if launch_mode.is_main() && config.launcher.stats_upload {
        if let Err(error) = crate::utils::stats::spawn_startup_ingest() {
            warn!(%error, "failed to schedule startup stats ingest");
        }
    }

    if launch_mode.is_main() {
        spawn_noncritical_startup_work();
    } else {
        info!("Import-mode preinit done");
    }

    let bootstrap = runtime.block_on(crate::app::AppBootstrap::from_config(&config, launch_mode));
    info!(
        elapsed_ms = startup_started.elapsed().as_millis(),
        "startup critical path complete; entering GPUI"
    );
    ensure_gpui_outside_tokio_runtime()?;
    crate::app::run(bootstrap)?;

    crate::config::config::flush_config_now();

    Ok(())
}

fn ensure_gpui_outside_tokio_runtime() -> Result<()> {
    anyhow::ensure!(
        tokio::runtime::Handle::try_current().is_err(),
        "GPUI event loop must not run inside a Tokio runtime context"
    );
    Ok(())
}

fn launch_working_dir(launch_mode: &LaunchMode) -> Option<std::path::PathBuf> {
    match launch_mode {
        LaunchMode::DirectLaunch(context) => std::path::Path::new(&context.version_folder)
            .parent()
            .map(std::path::Path::to_path_buf),
        _ => None,
    }
}

fn spawn_noncritical_startup_work() {
    let _ = crate::tasks::runtime::spawn_io(async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Err(error) = crate::core::sponsors::preload().await {
            debug!(?error, "sponsor preload skipped");
        }
    });
}

fn run_updater_mode(context: &crate::launch::UpdaterLaunchContext) -> Result<()> {
    crate::updater::run(context)
}

fn run_silent_direct_launch(version_folder: &str) -> Result<()> {
    crate::core::minecraft::launcher::launch_version_silent(version_folder)
}
