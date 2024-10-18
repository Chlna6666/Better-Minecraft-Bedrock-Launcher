// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::arch::asm;
use tauri::{AppHandle, Manager, Window};
use tauri_plugin_fs::FsExt;
mod utils;
use utils::config::{get_custom_style, set_custom_style};
use std::{env, fs, mem};
use std::path::PathBuf;
use winapi::um::winnls::GetACP;
use std::os::windows::ffi::OsStringExt;
use lazy_static::lazy_static;
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::SystemInformation::{PROCESSOR_ARCHITECTURE_INTEL, PROCESSOR_ARCHITECTURE_AMD64, PROCESSOR_ARCHITECTURE_ARM};
use crate::utils::logger::{clear_latest_log, log};

// 定义全局常量
const APP_VERSION: &str = "0.0.1"; // 版本号

#[tauri::command]
fn get_app_version() -> &'static str {
    APP_VERSION
}


lazy_static! {
    static ref APP_PATH: PathBuf = env::current_exe().unwrap(); // 程序路径
    static ref SYSTEM_ENCODING: u32 = detect_system_encoding(); // 系统编码
}


fn detect_system_encoding() -> u32 {
    // 获取当前活动代码页
    let codepage = unsafe { GetACP() };
    codepage
}
// 获取 CPU 架构
unsafe fn get_cpu_architecture() -> String {
    let mut sys_info: SYSTEM_INFO = unsafe { mem::zeroed() };
    unsafe {
        GetSystemInfo(&mut sys_info);
    }

    // Access the processor architecture through the nested Anonymous field
    match sys_info.Anonymous.Anonymous.wProcessorArchitecture {
        PROCESSOR_ARCHITECTURE_INTEL => "x86".to_string(),
        PROCESSOR_ARCHITECTURE_AMD64 => "x64".to_string(),
        PROCESSOR_ARCHITECTURE_ARM => "ARM".to_string(),
        _ => "Unknown".to_string(),
    }
}

#[tauri::command]
fn read_music_directory(directory: &str) -> Result<Vec<String>, String> {
    let path = PathBuf::from(directory);
    let mut files = Vec::new();

    if path.exists() && path.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();

            // 仅添加支持的音频文件格式
            if let Some(extension) = path.extension() {
                if extension == "m4a" || extension == "mp3" || extension == "wav" || extension == "flac" || extension == "ogg" || extension == "aac" {
                    files.push(path.to_string_lossy().to_string());
                }
            }
        }
    } else {
        return Err("Directory not found or not accessible".into());
    }

    Ok(files)
}

#[tauri::command]
fn read_file_content(file_path: String) -> Result<String, String> {
    // 读取文件内容，并返回 Base64 编码的字符串
    let file_content = std::fs::read(&file_path).map_err(|e| e.to_string())?;
    Ok(base64::encode(file_content))
}


#[tauri::command]
async fn close_splashscreen(window: tauri::Window) {
    // 关闭初始屏幕
    if let Some(splashscreen) = window.get_window("splashscreen") {
        splashscreen.close().unwrap();
    }
    // 显示主窗口
    window.get_window("main").unwrap().show().unwrap();
}

#[tauri::command]
async fn show_splashscreen(window: tauri::Window) {
    if let Some(splashscreen) = window.get_window("splashscreen") {
        splashscreen.show().unwrap(); // 显示启动窗口
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

fn main() {
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
        .plugin(tauri_plugin_prevent_default::init())
        .setup(|app| {
            clear_latest_log();
            info!("[APP] BMCBL Start! Version: {}", APP_VERSION);
            info!("[APP] App Path: {:?}", *APP_PATH);
            info!("[APP] System Encoding: {}", *SYSTEM_ENCODING);
            let cpu_architecture = unsafe { get_cpu_architecture() };
            info!("[APP] CPU Architecture: {}", cpu_architecture);
            utils::file_ops::create_initial_directories(); // 创建初始目录
                let window = app.get_webview_window("main").unwrap();
                window.open_devtools();
                window.close_devtools();
            // 允许指定目录
            let scope = app.fs_scope();
            scope.allow_directory("*", true);
            debug!("{:?}", scope.allowed());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            close_splashscreen,
            show_splashscreen,
            get_app_version,
            get_custom_style,
            set_custom_style,
            read_music_directory,
            read_file_content,
            log
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
