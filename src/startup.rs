use crate::launch::{LaunchMode, parse_launch_mode};
use anyhow::Result;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;
use std::{env, process};
use tracing::{debug, error, info};

#[cfg(windows)]
const SINGLE_INSTANCE_MUTEX_NAME: &str = "Global\\com.bmcbl.app.single_instance";

#[cfg(windows)]
fn bring_main_window_to_foreground() {
    use std::ffi::OsStr;
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

    // SAFETY: `wide_window_title` is NUL-terminated and remains alive for the duration of the call.
    let hwnd = match unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide_window_title.as_ptr())) } {
        Ok(hwnd) => hwnd,
        Err(error) => {
            warn!(?error, window_title = %window_title, "could not find existing main window");
            return;
        }
    };

    // SAFETY: `hwnd` came from `FindWindowW` and is only queried for its iconic state.
    if unsafe { IsIconic(hwnd).as_bool() } {
        // SAFETY: `hwnd` came from `FindWindowW`; restoring it is a best-effort foreground action.
        let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
    }

    // SAFETY: `hwnd` came from `FindWindowW`; foreground activation is best-effort.
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
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;

    let wide_name: Vec<u16> = OsStr::new(SINGLE_INSTANCE_MUTEX_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `wide_name` is NUL-terminated and remains alive for the duration of the call.
    let mutex_handle = match unsafe { CreateMutexW(None, true, PCWSTR(wide_name.as_ptr())) } {
        Ok(handle) => handle,
        Err(error) => {
            error!(?error, "failed to create single instance mutex");
            return None;
        }
    };

    // SAFETY: `GetLastError` reads the thread-local Win32 error after `CreateMutexW`.
    if unsafe { GetLastError() }.0 == ERROR_ALREADY_EXISTS.0 {
        // SAFETY: `mutex_handle` is the valid handle returned by `CreateMutexW`.
        let _ = unsafe { CloseHandle(mutex_handle) };
        bring_main_window_to_foreground();
        return Some(false);
    }

    crate::utils::single_instance::set_mutex_handle(mutex_handle);
    Some(true)
}

#[cfg(windows)]
fn single_instance_guard(launch_mode: &LaunchMode) -> Option<SingleInstanceGuard> {
    match launch_mode {
        LaunchMode::Main => match check_single_instance() {
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
        },
        LaunchMode::Import(_) | LaunchMode::Updater(_) => None,
    }
}

pub fn run() -> Result<()> {
    crate::utils::memory::configure_mimalloc_optimizer();
    build_launcher_runtime()?.block_on(async_main())
}

fn build_launcher_runtime() -> Result<tokio::runtime::Runtime> {
    let available_threads = std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(2);
    let worker_threads = available_threads.clamp(2, 4);
    let blocking_threads = available_threads.saturating_add(2).clamp(4, 8);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .max_blocking_threads(blocking_threads)
        .thread_stack_size(1024 * 1024)
        .thread_name("bmcbl-runtime")
        .build()
        .map_err(Into::into)
}

async fn async_main() -> Result<()> {
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
        crate::utils::logger::init_logging(false);
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
    crate::utils::logger::init_logging(config.launcher.debug);
    debug!(
        debug_enabled = config.launcher.debug,
        language = %config.launcher.language,
        renderer_backend = %config.launcher.renderer_backend,
        "configuration loaded and debug logging state applied"
    );

    if launch_mode.is_main()
        && let Err(error) = crate::utils::diagnostics::prepare_previous_run_reports()
    {
        error!(?error, "failed to prepare previous run diagnostics");
    }
    if launch_mode.is_main()
        && let Err(error) = crate::utils::diagnostics::mark_session_started()
    {
        error!(?error, "failed to mark diagnostics session as started");
    }

    if launch_mode.is_main() {
        crate::utils::updater_child::clean_old_versions();
        crate::utils::registry::register_file_associations();
    }

    if launch_mode.is_main() && config.launcher.stats_upload {
        crate::utils::stats::spawn_startup_ingest();
    }

    if launch_mode.is_main() {
        log_system_info();
    } else {
        info!("Import-mode preinit done");
    }

    let bootstrap = crate::app::AppBootstrap::from_config(&config, launch_mode).await;
    crate::app::run(bootstrap)?;

    Ok(())
}

fn launch_working_dir(launch_mode: &LaunchMode) -> Option<std::path::PathBuf> {
    match launch_mode {
        LaunchMode::Updater(context) => context
            .destination_path
            .parent()
            .map(std::path::Path::to_path_buf),
        LaunchMode::Main | LaunchMode::Import(_) => env::current_exe()
            .ok()
            .and_then(|exe_path| exe_path.parent().map(std::path::Path::to_path_buf)),
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
            process::exit(0);
        }
        Err(error) => {
            error!(src = %src, dst = %dst, error = ?error, "updater-child failed");
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
