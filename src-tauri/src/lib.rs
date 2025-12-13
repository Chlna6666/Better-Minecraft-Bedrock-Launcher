// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
pub mod archive;
pub mod commands;
pub mod config;
pub mod core;
pub mod downloads;
pub mod http;
pub mod i18n;
pub mod plugins;
pub mod result;
pub mod tasks;
pub mod utils;

use ::core::task;
use crate::archive::commands::{extract_zip_appx, import_appx};
use crate::commands::*;
use crate::config::config::Config;
use crate::core::minecraft::assets::delete_game_asset;
use crate::core::minecraft::map::list_minecraft_worlds_for_user;
use crate::core::version::gdk_users::get_gdk_users;
use crate::downloads::commands::{download_appx, download_resource};
use crate::http::commands::fetch_remote;
use crate::tasks::commands::{cancel_task, get_task_status};
use crate::utils::app_handle::set_global_app;
use crate::utils::app_info;
use crate::utils::logger::log;
use crate::utils::system_info::get_system_language;
use crate::utils::updater::{check_updates, download_and_apply_update, quit_app};
use crate::utils::utils::to_wstr;
use anyhow::{anyhow, Result};
use std::env;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_fs::FsExt;
use tracing::{error, info};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

#[derive(Clone)]
pub struct PreInit {
    pub config: Config,
    pub locale: String,
    pub webview2_ver: String,
}

/// Windows 原生错误弹窗（使用 windows crate）
pub fn show_windows_error(title: &str, msg: &str) {
    let wide_title = to_wstr(title);
    let wide_msg = to_wstr(msg);
    // 保证 wide_title/wide_msg 在调用期间存活（它们在当前作用域）
    unsafe {
        // HWND(0) 表示没有 owner 窗口（也可以传 HWND(handle)）
        MessageBoxW(
            Option::from(HWND(std::ptr::null_mut())),
            PCWSTR(wide_msg.as_ptr()),
            PCWSTR(wide_title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn show_window(app: &AppHandle) {
    let windows = app.webview_windows();
    windows
        .values()
        .next()
        .expect("Sorry, no window found")
        .set_focus()
        .expect("Can't Bring Window to Focus");
}

/// debug_mode 接受已读取的 Config（避免在这里重复读取配置文件）
fn debug_mode(app: &tauri::App, config: &Config) -> std::io::Result<()> {
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

/// run 接收 Arc<PreInit> 并返回 anyhow::Result
pub async fn run(preinit: Arc<PreInit>) -> Result<()> {
    // clone Arc 以便在 setup 闭包中捕获（Arc 轻量）
    let pre_clone = preinit.clone();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = show_window(app);
        }))
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            quit_app,
            check_updates,
            download_and_apply_update,
            log,
            get_locale,
            set_config,
            get_config,
            get_system_language,
            show_splashscreen,
            close_splashscreen,
            fetch_remote,
            download_appx,
            download_resource,
            get_app_version,
            get_app_license,
            get_tauri_sdk_version,
            get_full_build_info,
            get_webview2_version,
            read_music_directory,
            cancel_task,
            get_task_status,
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
            list_minecraft_worlds_for_user,
            get_mod_list,
            set_mod,
            import_mods,
            delete_mods,
            delete_game_asset,
            open_path,
            get_gdk_users
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            tasks::task_manager::init_task_manager(handle);
            let config_ref: &Config = &pre_clone.config;
            let locale = pre_clone.locale.clone();
            let webview2_ver = pre_clone.webview2_ver.clone();

            // 记录信息（main 已经做过日志初始化）
            info!(
                "BMCBL Start! Version: {} | Tauri SDK: {} | WebView2: {} | Git Commit: {} | Built At: {}",
                app_info::get_version(),
                app_info::get_tauri_version(),
                webview2_ver,
                env!("GIT_COMMIT_HASH"),
                env!("BUILD_TIME"),
            );

            // 使用提前读取到的 config 控制 devtools
            debug_mode(app, config_ref)?; // debug

            // 允许指定目录
            let scope = app.fs_scope();
            let _ = scope.allow_directory("**", true);

            set_global_app(app.handle().clone());

            Ok(())
        });

    // run 并将错误包装为 anyhow::Error 返回给 caller
    let ctx = tauri::generate_context!();
    builder
        .run(ctx)
        .map_err(|e| anyhow!("Tauri run failed: {:?}", e))?;

    Ok(())
}
