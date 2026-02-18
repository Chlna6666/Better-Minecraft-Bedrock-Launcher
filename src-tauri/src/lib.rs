// src/lib.rs
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
pub mod curseforge;

use crate::archive::commands::{extract_zip_appx, import_appx};
use crate::commands::*;
use crate::config::config::Config;
use crate::core::minecraft::assets::{delete_game_asset, import_assets,check_import_conflict,inspect_import_file};
use crate::core::minecraft::gdk::commands::unpack_gdk;
use crate::core::version::gdk_users::get_gdk_users;
use crate::downloads::commands::{download_appx, download_resource, download_resource_to_cache};
use crate::http::commands::fetch_remote;
use crate::plugins::manager::scan_plugins;
use crate::tasks::commands::{cancel_task, get_task_status};
use crate::utils::app_handle::set_global_app;
use crate::utils::app_info;
use crate::utils::logger::log;
use crate::utils::system_info::get_system_language;
use crate::utils::updater::{check_updates, download_and_apply_update, quit_app, restart_app};
use crate::utils::utils::to_wstr;
use anyhow::{anyhow, Result};
use std::env;
use std::sync::{Arc, Mutex};
// [新增] 引入 AtomicBool 用于状态标记
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, Emitter, WebviewWindowBuilder, WebviewUrl, WebviewWindow, App};
use tauri_plugin_fs::FsExt;
use tracing::{info, error};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
use crate::core::minecraft::commands::{backup_map_cmd, export_map_cmd, get_behavior_packs, get_minecraft_worlds, get_resource_packs, read_level_dat_cmd, write_level_dat_cmd};
use crate::core::version::settings::{get_version_config, save_version_config};
use crate::curseforge::{get_curseforge_categories, get_curseforge_mod, get_curseforge_mod_description, get_curseforge_mod_files, get_minecraft_versions, search_curseforge_mods};
use crate::plugins::commands::{get_plugins_list, load_plugin_script};
use crate::utils::network::{probe_gdk_asset_cdns, test_network_connectivity};

#[derive(Clone)]
pub struct PreInit {
    pub config: Config,
    pub locale: String,
    pub webview2_ver: String,
}

pub struct StartupImportState(pub Arc<Mutex<Option<String>>>);

// [新增] 标记核心功能（Tasks, Plugins）是否已初始化
static HAS_INIT_CORE: AtomicBool = AtomicBool::new(false);

pub fn show_windows_error(title: &str, msg: &str) {
    let wide_title = to_wstr(title);
    let wide_msg = to_wstr(msg);
    unsafe {
        MessageBoxW(
            Option::from(HWND(std::ptr::null_mut())),
            PCWSTR(wide_msg.as_ptr()),
            PCWSTR(wide_title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn find_mc_file(args: &[String]) -> Option<String> {
    for arg in args.iter().skip(1) {
        let lower = arg.to_lowercase();
        if lower.ends_with(".mcpack") || lower.ends_with(".mcworld") || lower.ends_with(".mcaddon") || lower.ends_with(".mctemplate") {
            return Some(arg.replace("\"", ""));
        }
    }
    None
}

// 获取或动态创建导入窗口
fn get_or_create_import_window(app: &AppHandle) -> Option<WebviewWindow> {
    if let Some(w) = app.get_webview_window("import") {
        return Some(w);
    }
    info!("Creating Import Window dynamically...");
    match WebviewWindowBuilder::new(
        app,
        "import",
        WebviewUrl::App("import.html".into())
    )
        .title("资源导入")
        .inner_size(600.0, 500.0)
        .min_inner_size(400.0, 300.0)
        .center()
        .resizable(true)
        .decorations(true)
        .visible(false)
        .skip_taskbar(false)
        .build() {
        Ok(w) => Some(w),
        Err(e) => {
            error!("Failed to create import window: {:?}", e);
            None
        }
    }
}

fn handle_import_request(app: &AppHandle, file_path: String) {
    if let Some(state) = app.try_state::<StartupImportState>() {
        *state.0.lock().unwrap() = Some(file_path.clone());
    }
    if let Some(import_win) = get_or_create_import_window(app) {
        let _ = import_win.show();
        let _ = import_win.set_focus();
        let _ = import_win.unminimize();
        let _ = import_win.emit("import-file-requested", file_path);
    }
}

// [Bug 1 修复] 确保主环境就绪（窗口+后台任务）
// 供单例模式唤醒时调用
fn ensure_main_environment(app: &AppHandle) {
    // 1. 如果之前是极速导入模式，核心可能未初始化，这里进行补救
    if !HAS_INIT_CORE.load(Ordering::Relaxed) {
        info!("Hot-switching to Normal Mode: Initializing core systems...");
        tasks::task_manager::init_task_manager(app.clone());
        let _plugins = scan_plugins(app);
        info!("Plugin System Initialized (Lazy).");
        HAS_INIT_CORE.store(true, Ordering::Relaxed);
    }

    // 2. 检查主窗口是否存在
    if app.get_webview_window("main").is_none() {
        info!("Main window missing (destroyed in import mode), recreating...");
        // 动态重建主窗口 (参数需与 tauri.conf.json 保持一致或符合 UI 预期)
        let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
            .title("Better Minecraft Bedrock Launcher")
            .inner_size(1000.0, 650.0)
            .min_inner_size(800.0, 600.0)
            .center()
            .resizable(true)
            .decorations(false) // 自定义标题栏
            .visible(true)      // 直接显示
            .build();

        if let Err(e) = win {
            error!("Failed to recreate main window: {:?}", e);
        }
    } else {
        // 如果窗口存在（可能被隐藏），显示它
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
            let _ = main.set_focus();
            let _ = main.unminimize();
        }
    }
}

fn show_main_or_splash_initial(app: &mut App) {
    if let Some(splash) = app.get_webview_window("splashscreen") {
        let _ = splash.show();
        let _ = splash.set_focus();
    } else if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
}

fn debug_mode(app: &tauri::App, config: &Config) -> std::io::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        if config.launcher.debug {
            window.open_devtools();
        } else {
            window.close_devtools();
        }
    }
    if let Some(window) = app.get_webview_window("import") {
        if config.launcher.debug {
            window.open_devtools();
        }
    }
    Ok(())
}

pub async fn run(preinit: Arc<PreInit>) -> Result<()> {
    let pre_clone = preinit.clone();

    let import_state = Arc::new(Mutex::new(None));
    let import_state_clone = import_state.clone();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(move |app, args, _cwd| {
            info!("Single instance triggered with args: {:?}", args);
            if let Some(file_path) = find_mc_file(&args) {
                // 热启动：文件导入
                handle_import_request(app, file_path);
            } else {
                // 热启动：正常打开 -> [修复] 确保环境和窗口恢复
                ensure_main_environment(app);
            }
        }))
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(curseforge::CurseForgeClient::new())
        .manage(OnlineState::default())
        .manage(StartupImportState(import_state))
        .invoke_handler(tauri::generate_handler![
            quit_app,
            check_updates,
            download_and_apply_update,
            log,
            get_locale,
            utils::mc_dependency::get_mc_deps_prompt,
            utils::mc_dependency::start_mc_deps_install,
            utils::mc_dependency::open_ms_store_for_pfn,
            // Backwards-compatible command names (deprecated): keep for older frontend builds.
            utils::mc_dependency::get_appx_deps_prompt,
            utils::mc_dependency::start_appx_deps_install,
            restart_app,
            set_config,
            get_config,
            get_system_language,
            show_splashscreen,
            close_splashscreen,
            fetch_remote,
            download_appx,
            download_resource,
            download_resource_to_cache,
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
            get_resource_packs,
            get_behavior_packs,
            get_minecraft_worlds,
            get_mod_list,
            set_mod,
            set_mod_inject_delay,
            set_mod_type,
            import_mods,
            import_assets,
            check_import_conflict,
            inspect_import_file,
            delete_mods,
            delete_game_asset,
            open_path,
            get_version_config,
            save_version_config,
            get_gdk_users,
            unpack_gdk,
            read_level_dat_cmd,
            write_level_dat_cmd,
            export_map_cmd,
            backup_map_cmd,
            get_curseforge_mod,
            get_curseforge_mod_description,
            get_curseforge_mod_files,
            get_curseforge_categories,
            get_minecraft_versions,
            search_curseforge_mods,
            test_network_connectivity,
            probe_gdk_asset_cdns,
            get_startup_import_file,
            paperconnect_generate_room,
            paperconnect_parse_room_code,
            easytier_start,
            easytier_restart_with_port_forwards,
            easytier_stop,
            easytier_cli_peers,
            easytier_embedded_status,
            easytier_embedded_nat_types,
            easytier_embedded_peers,
            paperconnect_find_center,
            paperconnect_tcp_request,
            paperconnect_default_client_id,
            paperconnect_pick_listen_port,
            paperconnect_server_start,
            paperconnect_server_stop,
            paperconnect_server_state,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let config_ref: &Config = &pre_clone.config;

            info!(
                "BMCBL Start! Version: {} | Args: {:?}",
                app_info::get_version(),
                env::args().collect::<Vec<_>>()
            );

            debug_mode(app, config_ref)?;

            let scope = app.fs_scope();
            let _ = scope.allow_directory("**", true);

            set_global_app(app.handle().clone());
            utils::stats::spawn_startup_ingest();

            let args: Vec<String> = env::args().collect();

            if let Some(file_path) = find_mc_file(&args) {
                // [分支 A] 导入模式 (极速启动)
                info!("Fast Launch for Import: {}", file_path);

                *import_state_clone.lock().unwrap() = Some(file_path.clone());

                // 1. 动态创建导入窗口
                handle_import_request(&handle, file_path);

                // 2. 销毁未使用的窗口以节省内存
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.close();
                }
                if let Some(splash) = app.get_webview_window("splashscreen") {
                    let _ = splash.close();
                }

                // 标记未初始化核心
                HAS_INIT_CORE.store(false, Ordering::Relaxed);

            } else {
                // [分支 B] 正常模式 (完整加载)
                // 如果缺少 UWP 依赖，优先弹出现代化 Webview 窗口处理（与主前端/导入窗口一致的 UI 风格）。
                match utils::mc_dependency::maybe_open_mc_deps_window(&handle) {
                    Ok(true) => {
                        if let Some(main) = app.get_webview_window("main") {
                            let _ = main.close();
                        }
                        if let Some(splash) = app.get_webview_window("splashscreen") {
                            let _ = splash.close();
                        }
                        HAS_INIT_CORE.store(false, Ordering::Relaxed);
                        return Ok(());
                    }
                    Ok(false) => {}
                    Err(e) => {
                        error!("Failed to open mc deps window: {:?}", e);
                    }
                }

                HAS_INIT_CORE.store(true, Ordering::Relaxed); // 标记已初始化
                tasks::task_manager::init_task_manager(handle.clone());
                let _plugins = scan_plugins(&handle);
                info!("Plugin System Initialized. Loaded {} plugins.", _plugins.len());

                show_main_or_splash_initial(app);
            }

            Ok(())
        });

    let ctx = tauri::generate_context!();
    builder
        .run(ctx)
        .map_err(|e| anyhow!("Tauri run failed: {:?}", e))?;

    Ok(())
}
