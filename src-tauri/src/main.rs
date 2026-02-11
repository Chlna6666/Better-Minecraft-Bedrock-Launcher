// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;
use std::{env, process};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};

use app_lib::i18n::I18n;
use app_lib::{run, show_windows_error, utils};
use app_lib::config::config::{read_config, Config};
use app_lib::utils::logger::init_logging;
use app_lib::utils::system_info::{detect_system_encoding, get_cpu_architecture, get_system_language};
use app_lib::utils::{updater, webview2_manager};
use app_lib::utils::developer_mode::ensure_developer_mode_enabled;
use app_lib::utils::updater::clean_old_versions;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // [Bug 2 修复] 强制将工作目录设置为 EXE 所在目录
    // 解决双击文件启动时，相对路径 "./BMCBL" 指向错误位置的问题
    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let _ = env::set_current_dir(exe_dir);
        }
    }

    init_logging();

    // 创建初始目录（同步）
    utils::file_ops::create_initial_directories();

    // 处理 --run-updater 子进程逻辑（保持原样）
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--run-updater") {
        let src = args.get(2).cloned().unwrap_or_default();
        let dst = args.get(3).cloned().unwrap_or_default();
        let timeout_secs = args
            .get(4)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);

        info!(src = %src, dst = %dst, timeout_secs, "updater-child start");
        debug!(args = ?args, "full arg list for updater-child");

        let start = std::time::Instant::now();

        match updater::run_updater_child(
            Path::new(&src),
            Path::new(&dst),
            Duration::from_secs(timeout_secs),
        ) {
            Ok(_) => {
                let elapsed = start.elapsed();
                info!(src = %src, dst = %dst, elapsed_ms = %elapsed.as_millis(), "updater-child success");
                process::exit(0);
            }
            Err(err) => {
                error!(src = %src, dst = %dst, error = ?err, "updater-child failed");
                process::exit(2);
            }
        }
    }

    clean_old_versions();

    utils::registry::register_file_associations();

    // 读取配置文件
    let config: Config = match read_config() {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("读取配置失败: {:?}\n程序将退出。", e);
            error!("{}", msg);
            show_windows_error("启动失败 - 读取配置", &msg);
            process::exit(1);
        }
    };

    // WebView2 GPU 加速开关：通过 Additional Browser Arguments 控制
    // 注意：该设置需要重启应用才能生效（WebView2 环境只在启动时创建）
    #[cfg(target_os = "windows")]
    if !config.launcher.gpu_acceleration {
        const DISABLE_GPU_ARGS: &str = "--disable-gpu --disable-software-rasterizer";
        let existing = env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        let new_val = if existing.trim().is_empty() {
            DISABLE_GPU_ARGS.to_string()
        } else if existing.contains("--disable-gpu") {
            existing
        } else {
            format!("{} {}", existing.trim_end(), DISABLE_GPU_ARGS)
        };
        env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", new_val);
        info!("WebView2 GPU acceleration disabled by config.");
    }

    let locale = match config.launcher.language.as_str() {
        "auto" => get_system_language(),
        "" => "en-US".to_string(),
        other => other.replace('_', "-"),
    };

    I18n::init(&locale);

    let webview2_ver = match webview2_manager::ensure_webview2_or_fallback() {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("WebView2 运行时检测失败: {:?}\n请确保 WebView2 已安装或允许程序继续。", e);
            error!("{}", msg);
            show_windows_error("启动失败 - WebView2 检测", &msg);
            process::exit(1);
        }
    };

    let _ = ensure_developer_mode_enabled();

    let mut sys = sysinfo::System::new_all();
    sys.refresh_all();
    let sys_name = sysinfo::System::name().unwrap_or_else(|| "未知系统".to_string());
    let kernel_version = sysinfo::System::kernel_version().unwrap_or_else(|| "未知内核版本".to_string());
    let os_version = sysinfo::System::os_version().unwrap_or_else(|| "未知OS版本".to_string());

    info!(
        "Preinit Done. App Path: {:?}",
        env::current_exe().unwrap_or_else(|_| Path::new(".").to_path_buf())
    );
    info!(
        "System Info: Encoding: {} | System: {} | Kernel: {} | OS Version: {} | CPU Architecture: {} | Language: {}",
        detect_system_encoding(),
        sys_name,
        kernel_version,
        os_version,
        get_cpu_architecture(),
        get_system_language()
    );

    let preinit = Arc::new(app_lib::PreInit {
        config,
        locale,
        webview2_ver,
    });

    match run(preinit).await {
        Ok(_) => {
            info!("Program exited normally.");
            process::exit(0);
        }
        Err(e) => {
            let err_msg = format!("程序运行失败: {:?}", e);
            error!("{}", err_msg);
            show_windows_error("程序运行失败", &err_msg);
            process::exit(1);
        }
    }
}
