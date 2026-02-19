// src/commands/minecraft.rs

use crate::commands::{close_launcher_window, minimize_launcher_window};
use crate::config::config::read_config;
use crate::core::inject::inject::{
    launch_win32_with_injection, inject_existing_process, grant_all_application_packages_access
};
use crate::core::inject::pe::{ensure_backup, restore_original_pe, inject_dll_import, is_file_patched};
use crate::core::minecraft::mod_manager::load_mods_config;
use crate::core::minecraft::appx::register::register_appx_package_async;
use crate::core::minecraft::appx::remove::remove_package;
use crate::core::minecraft::appx::utils::{get_manifest_identity, get_package_info};
use crate::core::minecraft::mouse_lock::start_window_monitor;
use crate::core::minecraft::uwp_minimize_fix::enable_debugging_for_package;
use crate::core::version::settings::get_version_config;
use crate::core::minecraft::launcher::start::{get_pids_by_name, is_process_in_package, launch_uwp_command_only};

use tauri::{AppHandle, Emitter};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::sync::Arc;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use serde_json::json;
use std::fs;
use std::cmp::Ordering;

use windows::core::HSTRING;
use windows::Management::Deployment::PackageManager;

use pelite::pe64::{Pe, PeFile};

const INJECTOR_BYTES: &[u8] = include_bytes!("../../assets/BLoader.dll");

#[repr(C)]
#[allow(non_snake_case)]
struct VS_FIXEDFILEINFO_WIN32 {
    pub dwSignature: u32,
    pub dwStrucVersion: u32,
    pub dwFileVersionMS: u32,
    pub dwFileVersionLS: u32,
    pub dwProductVersionMS: u32,
    pub dwProductVersionLS: u32,
    pub dwFileFlagsMask: u32,
    pub dwFileFlags: u32,
    pub dwFileOS: u32,
    pub dwFileType: u32,
    pub dwFileSubtype: u32,
    pub dwFileDateMS: u32,
    pub dwFileDateLS: u32,
}

// --- [辅助函数] ---

fn emit_launch(app: &AppHandle, stage: &str, status: &str, message: Option<String>, code: Option<String>) {
    let msg_str = message.clone().unwrap_or_default();
    debug!("[LaunchEvent] Stage: {}, Status: {}, Msg: {}", stage, status, msg_str);
    let _ = app.emit("launch-progress", json!({
        "stage": stage,
        "status": status,
        "message": message,
        "code": code
    }));
}

fn remove_readonly(path: &Path) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut perms = metadata.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            if let Err(e) = fs::set_permissions(path, perms) {
                warn!("尝试移除只读属性失败: {} ({})", path.display(), e);
            } else {
                debug!("已移除只读属性: {}", path.display());
            }
        }
    }
}

fn ensure_file_in_dir(dir: &Path, filename: &str, content: &[u8]) -> Result<PathBuf, String> {
    let target = dir.join(filename);
    if target.exists() {
        remove_readonly(&target);
    }
    fs::write(&target, content).map_err(|e| format!("写入 {} 失败: {}", filename, e))?;
    let _ = grant_all_application_packages_access(&target);
    Ok(target)
}

fn identity_to_aumid(identity: &str) -> String {
    match identity {
        "Microsoft.MinecraftWindowsBeta" => "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe!App",
        "Microsoft.MinecraftEducationEdition" => "Microsoft.MinecraftEducationEdition_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition",
        "Microsoft.MinecraftEducationPreview" => "Microsoft.MinecraftEducationPreview_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition",
        _ => "Microsoft.MinecraftUWP_8wekyb3d8bbwe!App",
    }.to_string()
}

fn find_game_executable(package_folder: &str, identity_name: &str) -> Option<PathBuf> {
    let folder = Path::new(package_folder);
    let common_names = [
        "Minecraft.Windows.exe",
        "Minecraft.Education.exe",
        &format!("{}.exe", identity_name)
    ];
    for name in common_names {
        let p = folder.join(name);
        if p.exists() { return Some(p); }
    }
    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext.eq_ignore_ascii_case("exe") {
                    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                    if !file_name.contains("CrashSender") && !file_name.contains("Report") {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

fn get_registered_path(family_name: &str) -> Option<PathBuf> {
    let pm = PackageManager::new().ok()?;
    let packages = pm.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family_name)).ok()?;
    for pkg in packages {
        if let Ok(loc) = pkg.InstalledLocation() {
            if let Ok(path) = loc.Path() {
                return Some(PathBuf::from(path.to_string_lossy().to_string()));
            }
        }
    }
    None
}

fn parse_version_to_vec_simple(v: &str) -> Vec<u64> {
    v.split('.').map(|s| s.parse::<u64>().unwrap_or(0)).collect()
}

fn compare_versions(v1: &str, v2: &str) -> Ordering {
    let vec1 = parse_version_to_vec_simple(v1);
    let vec2 = parse_version_to_vec_simple(v2);
    vec1.cmp(&vec2)
}

fn is_win32_version(version: &str) -> bool {
    if version.starts_with("1.22.") || version.starts_with("1.23.") {
        return true;
    }
    let v = parse_version_to_vec_simple(version);
    if v.len() >= 3 {
        if v[0] == 1 && v[1] == 21 && v[2] >= 12201 {
            return true;
        }
    }
    const THRESHOLD: &str = "1.21.12000.21";
    compare_versions(version, THRESHOLD) != Ordering::Less
}

fn format_version(v: &[u64]) -> String {
    if v.len() >= 4 { format!("{}.{}.{}.{}", v[0], v[1], v[2], v[3]) } else { "0.0.0.0".to_string() }
}

fn get_embedded_dll_version(bytes: &[u8]) -> Option<Vec<u64>> {
    let file = match PeFile::from_bytes(bytes) {
        Ok(f) => f,
        Err(_) => return None,
    };
    let resources = file.resources().ok()?;
    let version_info = resources.version_info().ok()?;
    if let Some(fixed) = version_info.fixed() {
        unsafe {
            let ptr = fixed as *const _ as *const VS_FIXEDFILEINFO_WIN32;
            let info = &*ptr;
            return Some(vec![
                ((info.dwFileVersionMS >> 16) & 0xFFFF) as u64,
                (info.dwFileVersionMS & 0xFFFF) as u64,
                ((info.dwFileVersionLS >> 16) & 0xFFFF) as u64,
                (info.dwFileVersionLS & 0xFFFF) as u64,
            ]);
        }
    }
    None
}

// --- [核心逻辑] ---

pub async fn register_and_start(
    package_folder: &str,
    auto_start: bool,
    app: &AppHandle,
    launch_args: Option<&str>,
    _enable_console: bool,
) -> Result<Option<u32>, String> {
    info!("Starting launch sequence for: {}", package_folder);
    let config = read_config().map_err(|e| e.to_string())?;
    let game_cfg = &config.game;
    let mods_dir = Path::new(package_folder).join("mods");

    let folder_name_str = Path::new(package_folder).file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let ver_config = get_version_config(folder_name_str).await.unwrap_or_default();

    let app_clone = app.clone();
    let log_cb = Arc::new(move |msg: String| {
        let _ = app_clone.emit("launch-progress", json!({
            "stage": "inject",
            "status": "info",
            "message": msg,
            "code": null
        }));
    });

    // 1. 解析 Manifest
    emit_launch(app, "start", "info", Some("正在解析版本信息...".into()), None);
    let (identity_name, identity_version) = get_manifest_identity(package_folder).await.map_err(|e| format!("Manifest 解析失败: {}", e))?;

    let is_win32 = is_win32_version(&identity_version);
    info!("Version Info: Name={}, Ver={}, Win32Mode={}", identity_name, identity_version, is_win32);
    emit_launch(app, "manifest", "ok", Some(format!("版本: {} (Win32={})", identity_version, is_win32)), None);

    let final_console = false;

    let mut final_launch_args = launch_args.map(|s| s.to_string());

    if ver_config.editor_mode {
        if compare_versions(&identity_version, "1.19.80.20") != Ordering::Less {
            final_launch_args = Some("minecraft://?Editor=true".to_string());
        }
    }

    // 2. Mod 列表准备
    let mut startup_mods_relative_paths = Vec::new();
    let mut delayed_mods = Vec::new();

    if auto_start && !ver_config.disable_mod_loading {
        if let Ok(list) = load_mods_config(&mods_dir).await {
            for (path_buf, delay) in list {
                if let Some(path_str) = path_buf.to_str() {
                    let p = path_str.to_string();
                    if !is_win32 { let _ = grant_all_application_packages_access(&path_buf); }

                    if delay == 0 {
                        if let Some(fname) = path_buf.file_name().and_then(|n| n.to_str()) {
                            startup_mods_relative_paths.push(format!("mods/{}", fname));
                        }
                    } else {
                        delayed_mods.push((p, delay));
                    }
                }
            }
        }
    }

    // [PE Preloader 部署逻辑]
    if auto_start {
        if let Some(exe_path) = find_game_executable(package_folder, &identity_name) {
            let exe_dir = exe_path.parent().ok_or("Invalid exe directory")?;
            // ==================== [新增修复代码 START] ====================
            // 核心修复：在 DLL 注入前，由启动器代为创建 "Minecraft Bedrock" 文件夹并授予沙盒权限
            // 这解决了 DLL 在 UWP 容器内无法创建根目录的问题
            let local_data_root = exe_dir.join("Minecraft Bedrock");

            // 1. 确保目录存在
            if !local_data_root.exists() {
                if let Err(e) = fs::create_dir_all(&local_data_root) {
                    warn!("[Launcher] 预创建重定向根目录失败: {}", e);
                } else {
                    info!("[Launcher] 已预创建重定向根目录: {}", local_data_root.display());
                }
            }

            // 2. 强制赋予 ALL APPLICATION PACKAGES 完全控制权限 (S-1-15-2-1)
            // 这会让 UWP 游戏进程有权在里面写入文件
            if local_data_root.exists() {
                if let Err(e) = grant_all_application_packages_access(&local_data_root) {
                    warn!("[Launcher] 授予重定向目录 UWP 权限失败: {:?}", e);
                } else {
                    debug!("[Launcher] 已刷新重定向目录权限 (ALL APPLICATION PACKAGES)");
                }
            }
            let injector_name = "BLoader.dll";
            let injector_target_path = exe_dir.join(injector_name);

            let mut need_update = true;
            if injector_target_path.exists() {
                remove_readonly(&injector_target_path);
                match fs::read(&injector_target_path) {
                    Ok(disk_bytes) => {
                        if disk_bytes == INJECTOR_BYTES {
                            need_update = false;
                        }
                    }
                    Err(_) => {}
                }
            }

            if need_update {
                if let Err(e) = ensure_file_in_dir(exe_dir, injector_name, INJECTOR_BYTES) {
                    warn!("尝试更新 BLoader.dll 失败 (可能被占用): {}", e);
                }
            }

            let config_json = json!({
                "disable_mod_loading": ver_config.disable_mod_loading,
                "mods": startup_mods_relative_paths
            });
            let config_content = serde_json::to_string_pretty(&config_json).unwrap_or_default();
            let _ = ensure_file_in_dir(exe_dir, "preloader.json", config_content.as_bytes());

            if let Err(e) = ensure_backup(&exe_path) {
                warn!("无法创建 EXE 备份 (将继续使用自标记还原机制): {}", e);
            }

            if is_file_patched(&exe_path) {
                info!("检测到 PE 已包含补丁标记，跳过修改: {}", exe_path.display());
                emit_launch(app, "inject", "ok", Some("PE 已就绪 (跳过修补)".into()), None);
            } else {
                let _ = restore_original_pe(&exe_path);
                remove_readonly(&exe_path);

                info!("正在修改 PE 导入表: {}", exe_path.display());
                match inject_dll_import(&exe_path, injector_name, None) {
                    Ok(_) => emit_launch(app, "inject", "ok", Some("静态注入环境部署成功".into()), None),
                    Err(e) => {
                        error!("PE 注入失败: {}", e);
                        emit_launch(app, "inject", "error", Some(format!("PE 修改失败: {}", e)), None);
                        let _ = restore_original_pe(&exe_path);
                    }
                }
            }
        }
    }

    // 3. UWP 注册逻辑
    if !is_win32 {
        let aumid = identity_to_aumid(&identity_name);
        let family_name = aumid.split('!').next().unwrap_or("");
        let mut need_remove = false;
        let mut need_register = true;

        match get_package_info(&aumid) {
            Ok(Some((installed_ver, _, _))) => {
                let is_path_diff = if let Some(reg_path) = get_registered_path(family_name) {
                    let reg = fs::canonicalize(&reg_path).unwrap_or(reg_path.clone());
                    let tgt = fs::canonicalize(Path::new(package_folder)).unwrap_or(PathBuf::from(package_folder));
                    reg != tgt
                } else { true };
                let cmp_res = compare_versions(&installed_ver, &identity_version);
                if is_path_diff || cmp_res == Ordering::Greater { need_remove = true; }
                else if installed_ver == identity_version { need_register = false; }
            },
            _ => {}
        }
        if need_remove { let _ = remove_package(family_name).await; sleep(Duration::from_millis(500)).await; }
        if need_register { let _ = register_appx_package_async(package_folder).await; }
    }

    // 4. 启动与注入
    if auto_start {
        if !is_win32 && game_cfg.uwp_minimize_fix {
            if let Ok(Some((_, _, name))) = get_package_info(&identity_to_aumid(&identity_name)) {
                let _ = enable_debugging_for_package(&name);
            }
        }

        if is_win32 {
            // Win32 启动逻辑
            let exe_path = find_game_executable(package_folder, &identity_name).ok_or("EXE Not Found")?;

            match launch_win32_with_injection(exe_path.to_str().unwrap(), final_launch_args.as_deref(), Vec::new(), final_console, Some(log_cb.clone())).await {
                Ok(pid) => {
                    emit_launch(app, "launch", "ok", Some(format!("启动成功 PID: {}", pid)), None);
                    if !ver_config.disable_mod_loading {
                        handle_delayed_injection(pid, delayed_mods, log_cb.clone(), final_console);
                    }
                    return Ok(Some(pid));
                },
                Err(e) => return Err(format!("启动失败: {:?}", e)),
            }

        } else {
            // UWP 启动逻辑
            let aumid = identity_to_aumid(&identity_name);
            if let Err(e) = launch_uwp_command_only(&aumid, final_launch_args.as_deref().or(Some(""))).await {
                return Err(format!("启动请求失败: {:?}", e));
            }
            let target_exe = if identity_name.contains("Education") { "Minecraft.Education.exe" } else { "Minecraft.Windows.exe" };
            let pfn = aumid.split('!').next().unwrap_or("").to_string();
            if let Some(pid) = wait_for_uwp_pid(target_exe, &pfn).await {
                if !ver_config.disable_mod_loading {
                    handle_delayed_injection(pid, delayed_mods, log_cb.clone(), final_console);
                }
                return Ok(Some(pid));
            }
            return Err("启动超时".to_string());
        }
    }
    Ok(None)
}

fn handle_delayed_injection(pid: u32, mods: Vec<(String, u64)>, log_cb: Arc<dyn Fn(String) + Send + Sync>, show_console: bool) {
    if mods.is_empty() { return; }
    tokio::spawn(async move {
        for (path, delay) in mods {
            sleep(Duration::from_millis(delay)).await;
            let _ = inject_existing_process(pid, path, Some(log_cb.clone()), true, show_console).await;
        }
    });
}

// [核心修复] wait_for_uwp_pid 增加对现有进程的检测
async fn wait_for_uwp_pid(target_exe: &str, pfn: &str) -> Option<u32> {
    let start = Instant::now();
    let pids_before = get_pids_by_name(target_exe);

    while start.elapsed() < Duration::from_secs(15) {
        let current_pids = get_pids_by_name(target_exe);

        // 1. 优先检查新增进程 (Restart Scenario)
        for pid in &current_pids {
            if !pids_before.contains(pid) && is_process_in_package(*pid, pfn) {
                return Some(*pid);
            }
        }

        // 2. 检查现有进程被激活 (UWP Resume Scenario)
        // 如果 1.5 秒后还没有新 PID，就假设是旧进程被唤醒
        if start.elapsed() > Duration::from_millis(1500) {
            for pid in &current_pids {
                // 如果 PID 属于目标包，就认为是它
                if is_process_in_package(*pid, pfn) {
                    return Some(*pid);
                }
            }
        }

        sleep(Duration::from_millis(100)).await;
    }
    None
}

#[tauri::command]
pub async fn launch_appx(
    app: AppHandle,
    file_name: String,
    auto_start: bool,
    launch_args: Option<String>,
    enable_console: Option<bool>,
) -> Result<(), String> {
    info!("Command: launch_appx, file={}, auto={}, console={:?}", file_name, auto_start, enable_console);
    let versions_root = Path::new("./BMCBL/versions");
    let package_folder = versions_root.join(&file_name);
    if !package_folder.exists() { return Err(format!("版本不存在: {}", package_folder.display())); }

    let pid_opt = register_and_start(
        package_folder.to_str().unwrap(),
        auto_start,
        &app,
        launch_args.as_deref(),
        enable_console.unwrap_or(false)
    ).await?;

    if let Some(_pid) = pid_opt {
        if let Ok(config) = read_config() {
            // Per-version mouse lock settings
            let ver_cfg = get_version_config(file_name.clone()).await.unwrap_or_default();
            if ver_cfg.lock_mouse_on_launch {
                start_window_monitor("Minecraft", &ver_cfg.unlock_mouse_hotkey, ver_cfg.reduce_pixels);
                sleep(Duration::from_secs(2)).await;
            }

            match config.game.launcher_visibility.as_str() {
                "minimize" => minimize_launcher_window(&app),
                "close" => close_launcher_window(&app),
                _ => {}
            }
        }
    }
    emit_launch(&app, "done", "ok", Some("完成".into()), None);
    Ok(())
}
