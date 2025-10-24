// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod i18n;
pub mod utils;
pub mod core;
pub mod commands;
pub mod progress;
pub mod config;
pub mod plugins;
pub mod downloads;
pub mod result;

use std::env;
use tauri::{AppHandle, Manager};
use tauri_plugin_fs::FsExt;
use tracing::{error, info};
use crate::i18n::I18n;
use crate::utils::logger::{init_logging, log};
use crate::config::config::read_config;
use crate::commands::*;
use crate::utils::appx_dependency::ensure_uwp_dependencies_or_prompt;
use crate::utils::developer_mode::ensure_developer_mode_enabled;
use crate::utils::system_info::{detect_system_encoding, get_cpu_architecture, get_system_language};
use crate::utils::{app_info, webview2_manager};
use crate::utils::AppHandle::set_global_app;

fn show_window(app: &AppHandle) {
    let windows = app.webview_windows();
    windows
        .values()
        .next()
        .expect("Sorry, no window found")
        .set_focus()
        .expect("Can't Bring Window to Focus");
}

fn debug_mode(app: &tauri::App) -> std::io::Result<()> {
    // 读取配置文件
    let config = read_config()?;

    // 获取窗口对象
    if let Some(window) = app.get_webview_window("main") {
        if config.launcher.debug {
            // 打开开发者工具
            window.open_devtools();
        } else {
            // 关闭开发者工具
            window.close_devtools();
        }
    } else {
        error!("Failed to get main window.");
    }

    Ok(())
}

pub async fn run() {
    // 准备并运行 Tauri（注意：run 会阻塞直到应用退出）
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = show_window(app);
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            log,
            get_locale,
            set_config,
            get_config,
            get_system_language,
            show_splashscreen,
            close_splashscreen,
            fetch_remote,
            download_appx,
            cancel_install,
            get_app_version,
            get_app_license,
            get_tauri_sdk_version,
            get_full_build_info,
            get_webview2_version,
            read_music_directory,
            extract_zip_appx,
            import_appx,
            launch_appx,
            load_plugin_script,
            get_plugins_list,
            get_version_list,
            delete_version,
            get_all_resource_packs,
            get_all_behavior_packs,
            list_minecraft_worlds_cmd,
            get_mod_list,
            set_mod,
            import_mods,
            delete_mods,
            open_path,
        ])
        .setup(move |app| {
            // 初始化日志（保持原有行为）
            init_logging();

            // 创建初始目录（同步）
            utils::file_ops::create_initial_directories();

            // 读取配置（同步）
            let config = match read_config() {
                Ok(c) => c,
                Err(e) => {
                    error!("读取配置失败: {}", e);
                    return Err(Box::new(e));
                }
            };

            // 处理语言选择
            let locale = match config.launcher.language.as_str() {
                "auto" => get_system_language(),
                "" => "en-US".to_string(),
                other => other.to_string(),
            };

            // 初始化 i18n（同步）
            I18n::init(&locale);

            // 检查 WebView2（同步调用，可能会做一些 IO）
            let webview2_ver = webview2_manager::ensure_webview2_or_fallback()
                .expect("WebView2 Runtime 安装检测失败，程序已退出");

            // 其它初始化（同步）
            let _ = ensure_developer_mode_enabled();
            let _ = ensure_uwp_dependencies_or_prompt();

            // 获取系统信息
            let mut sys = sysinfo::System::new_all();
            sys.refresh_all();
            let sys_name = sysinfo::System::name().unwrap_or_else(|| "未知系统".to_string());
            let kernel_version = sysinfo::System::kernel_version().unwrap_or_else(|| "未知内核版本".to_string());
            let os_version = sysinfo::System::os_version().unwrap_or_else(|| "未知OS版本".to_string());

            info!(
                "BMCBL Start! Version: {} | Tauri SDK: {} | WebView2: {} | Git Commit: {} | Built At: {}",
                app_info::get_version(),
                app_info::get_tauri_version(),
                webview2_ver,
                env!("GIT_COMMIT_HASH"),
                env!("BUILD_TIME"),
            );
            info!("App Path: {:?}", env::current_exe().unwrap());
            info!(
                "System Info: Encoding: {} | System: {} | Kernel: {} | OS Version: {} | CPU Architecture: {} | Language: {}",
                detect_system_encoding(),
                sys_name,
                kernel_version,
                os_version,
                get_cpu_architecture(),
                get_system_language()
            );

            debug_mode(app)?; // debug
            // 允许指定目录
            let scope = app.fs_scope();
            let _ = scope.allow_directory("**", true);

            set_global_app(app.handle().clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
