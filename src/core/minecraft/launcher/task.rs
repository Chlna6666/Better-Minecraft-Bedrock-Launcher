use crate::config::config::read_config;
use crate::core::inject::inject::{
    grant_all_application_packages_access, inject_existing_process, launch_win32_with_injection,
};
use crate::core::inject::pe::{
    ensure_backup, inject_dll_import, is_file_patched, restore_original_pe,
};
use crate::core::minecraft::appx::register::register_appx_package_async;
use crate::core::minecraft::appx::remove::remove_package;
use crate::core::minecraft::appx::utils::{get_manifest_identity, get_package_info};
use crate::core::minecraft::launcher::start::{launch_uwp_command_only, wait_for_uwp_pid};
use crate::core::minecraft::mod_manager::load_mods_config;
use crate::core::minecraft::mouse_lock::start_window_monitor;
use crate::core::minecraft::uwp_minimize_fix::enable_debugging_for_package;
use crate::core::version::settings::get_version_config;
use crate::tasks::task_manager::{
    TaskControl, append_task_log, create_task_with_details, finish_task, is_cancelled,
    register_task_abort_handle, set_task_labels, set_task_message, set_total, task_control,
    update_progress,
};
use pelite::pe64::{Pe, PeFile};
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

use crate::utils::file_ops;

const INJECTOR_BYTES: &[u8] = include_bytes!("../../../../assets/bin/BLoader.dll");
const LAUNCH_TOTAL_STEPS: u64 = 5;
const BLOADER_DEFAULT_REDIRECTION_ROOT: &str = "Minecraft Bedrock";
const LAUNCHER_TASK_STAGE_LABELS: [(&str, &str); 5] = [
    ("parsing", "解析中"),
    ("preparing_files", "准备安装"),
    ("patching", "处理中"),
    ("initializing", "初始化中"),
    ("launching", "启动游戏"),
];

fn register_launcher_task_stage_labels() {
    crate::tasks::task_manager::register_task_stage_labels(LAUNCHER_TASK_STAGE_LABELS);
}

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

pub fn start_launch_task(request: LaunchRequest) -> String {
    register_launcher_task_stage_labels();
    let title = format!("启动 {}", request.display_name);
    let detail = Some(request.version.to_string());
    let task_id = create_task_with_details(
        None,
        title,
        detail,
        "starting",
        Some(LAUNCH_TOTAL_STEPS),
        false,
    );
    let _ = set_total(&task_id, Some(LAUNCH_TOTAL_STEPS));
    append_log(&task_id, format!("准备启动 {}", request.display_name));
    info!(
        task_id = %task_id,
        display_name = %request.display_name,
        version = %request.version,
        package_folder = %request.package_folder,
        auto_start = request.auto_start,
        has_launch_args = request.launch_args.is_some(),
        "已创建游戏启动任务"
    );

    let task_id_for_task = task_id.clone();
    let join_handle = tokio::spawn(async move {
        info!(
            task_id = %task_id_for_task,
            display_name = %request.display_name,
            version = %request.version,
            "游戏启动任务开始执行"
        );
        let result = launch_game(&request, &task_id_for_task).await;
        match result {
            Ok(Some(pid)) => {
                info!(
                    task_id = %task_id_for_task,
                    pid,
                    "游戏启动任务执行完成，已获得进程 PID"
                );
                append_log(&task_id_for_task, format!("游戏已启动，PID {pid}"));
                finish_task(&task_id_for_task, "completed", Some("启动完成".to_string()));
            }
            Ok(None) => {
                info!(
                    task_id = %task_id_for_task,
                    "游戏启动任务执行完成，当前流程未实际拉起游戏进程"
                );
                append_log(&task_id_for_task, "启动流程完成".to_string());
                finish_task(&task_id_for_task, "completed", Some("启动完成".to_string()));
            }
            Err(error) => {
                if is_cancelled(&task_id_for_task) {
                    append_log(&task_id_for_task, "启动已取消".to_string());
                    finish_task(
                        &task_id_for_task,
                        "cancelled",
                        Some("启动已取消".to_string()),
                    );
                    return;
                }

                error!("launch task failed: {error}");
                append_log(&task_id_for_task, format!("启动失败: {error}"));
                finish_task(&task_id_for_task, "error", Some(error));
            }
        }
    });
    register_abort_handle(&task_id, join_handle);
    task_id
}

fn register_abort_handle(task_id: &str, join_handle: JoinHandle<()>) {
    register_task_abort_handle(task_id.to_string(), join_handle.abort_handle());
}

fn append_log(task_id: &str, line: impl Into<String>) {
    let line = line.into();
    let _ = append_task_log(task_id, line.clone());
    let _ = set_task_message(task_id, Some(line));
}

fn advance_step(task_id: &str, stage: &str, message: impl Into<String>) {
    let message = message.into();
    append_log(task_id, message);
    update_progress(task_id, 1, Some(LAUNCH_TOTAL_STEPS), Some(stage));
}

fn check_cancelled(task_id: &str) -> Result<(), String> {
    if is_cancelled(task_id) {
        Err("启动已取消".to_string())
    } else {
        Ok(())
    }
}

fn check_cancelled_control(control: Option<&TaskControl>) -> Result<(), String> {
    if control.is_some_and(crate::tasks::task_manager::is_cancelled_fast) {
        Err("启动已取消".to_string())
    } else {
        Ok(())
    }
}

fn remove_readonly(path: &Path) {
    if let Ok(metadata) = fs::metadata(path) {
        let mut perms = metadata.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            if let Err(error) = fs::set_permissions(path, perms) {
                warn!("尝试移除只读属性失败: {} ({error})", path.display());
            }
        }
    }
}

fn ensure_file_in_dir(dir: &Path, filename: &str, content: &[u8]) -> Result<PathBuf, String> {
    let target = dir.join(filename);
    if target.exists() {
        remove_readonly(&target);
    }
    fs::write(&target, content).map_err(|error| format!("写入 {filename} 失败: {error}"))?;
    let _ = grant_all_application_packages_access(&target);
    Ok(target)
}

fn write_bloader_config(
    dir: &Path,
    disable_mod_loading: bool,
    enable_redirection: bool,
    file_redirections: Value,
    mods: Value,
) -> Result<PathBuf, String> {
    let config_path = dir.join("config.json");
    let mut config = fs::read_to_string(&config_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    config.insert(
        "disable_mod_loading".to_string(),
        json!(disable_mod_loading),
    );
    config.insert("enable_redirection".to_string(), json!(enable_redirection));
    config.insert(
        "redirection_root".to_string(),
        json!(BLOADER_DEFAULT_REDIRECTION_ROOT),
    );
    config.insert("file_redirections".to_string(), file_redirections);
    config.insert("mods".to_string(), mods);

    let config_content = serde_json::to_string_pretty(&Value::Object(config))
        .map_err(|error| format!("写入 BLoader 配置失败: {error}"))?;
    ensure_file_in_dir(dir, "config.json", config_content.as_bytes())
}

fn remove_legacy_preloader_config(dir: &Path) {
    let legacy_path = dir.join("preloader.json");
    if !legacy_path.exists() {
        return;
    }

    remove_readonly(&legacy_path);
    if let Err(error) = fs::remove_file(&legacy_path) {
        warn!(
            "无法删除旧 BLoader preloader.json 配置 {}: {error}",
            legacy_path.display()
        );
    }
}

fn remove_appx_signature_if_present(package_folder: &str) -> Result<bool, String> {
    let signature_path = Path::new(package_folder).join("AppxSignature.p7x");
    if !signature_path.exists() {
        return Ok(false);
    }

    remove_readonly(&signature_path);
    fs::remove_file(&signature_path)
        .map_err(|error| format!("删除 AppxSignature.p7x 失败: {error}"))?;
    Ok(true)
}

fn identity_to_aumid(identity: &str) -> String {
    match identity {
        "Microsoft.MinecraftWindowsBeta" => "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe!App",
        "Microsoft.MinecraftEducationEdition" => {
            "Microsoft.MinecraftEducationEdition_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition"
        }
        "Microsoft.MinecraftEducationPreview" => {
            "Microsoft.MinecraftEducationPreview_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition"
        }
        _ => "Microsoft.MinecraftUWP_8wekyb3d8bbwe!App",
    }
    .to_string()
}

fn find_game_executable(package_folder: &str, identity_name: &str) -> Option<PathBuf> {
    let folder = Path::new(package_folder);
    let common_names = [
        "Minecraft.Windows.exe",
        "Minecraft.Education.exe",
        &format!("{identity_name}.exe"),
    ];
    for name in common_names {
        let path = folder.join(name);
        if path.exists() {
            return Some(path);
        }
    }

    let entries = fs::read_dir(folder).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension() else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("exe") {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.contains("CrashSender") && !file_name.contains("Report") {
            return Some(path);
        }
    }

    None
}

fn get_registered_path(family_name: &str) -> Option<PathBuf> {
    let pm = PackageManager::new().ok()?;
    let packages = pm
        .FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family_name))
        .ok()?;
    for pkg in packages {
        if let Ok(location) = pkg.InstalledLocation()
            && let Ok(path) = location.Path()
        {
            return Some(PathBuf::from(path.to_string_lossy().to_string()));
        }
    }
    None
}

fn parse_version_to_vec_simple(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|segment| segment.parse::<u64>().unwrap_or(0))
        .collect()
}

fn compare_versions(left: &str, right: &str) -> Ordering {
    parse_version_to_vec_simple(left).cmp(&parse_version_to_vec_simple(right))
}

fn build_editor_deeplink(is_win32: bool, is_preview: bool) -> String {
    let scheme = if is_preview {
        "minecraft-preview"
    } else {
        "minecraft"
    };
    if is_win32 {
        format!("{scheme}://creator/?Editor=true")
    } else {
        format!("{scheme}://?Editor=true")
    }
}

fn is_win32_version(version: &str) -> bool {
    if version.starts_with("1.22.") || version.starts_with("1.23.") {
        return true;
    }
    let parsed = parse_version_to_vec_simple(version);
    if parsed.len() >= 3 && parsed[0] == 1 && parsed[1] == 21 && parsed[2] >= 12201 {
        return true;
    }
    compare_versions(version, "1.21.12000.21") != Ordering::Less
}

fn get_embedded_dll_version(bytes: &[u8]) -> Option<Vec<u64>> {
    let file = PeFile::from_bytes(bytes).ok()?;
    let resources = file.resources().ok()?;
    let version_info = resources.version_info().ok()?;
    let fixed = version_info.fixed()?;
    // SAFETY: `fixed()` returns a valid VS_FIXEDFILEINFO-compatible blob owned by pelite.
    let info = unsafe { &*(fixed as *const _ as *const VS_FIXEDFILEINFO_WIN32) };
    Some(vec![
        ((info.dwFileVersionMS >> 16) & 0xFFFF) as u64,
        (info.dwFileVersionMS & 0xFFFF) as u64,
        ((info.dwFileVersionLS >> 16) & 0xFFFF) as u64,
        (info.dwFileVersionLS & 0xFFFF) as u64,
    ])
}

pub fn embedded_dll_version_string() -> Option<String> {
    get_embedded_dll_version(INJECTOR_BYTES).map(|parts| {
        parts
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(".")
    })
}

async fn launch_game(request: &LaunchRequest, task_id: &str) -> Result<Option<u32>, String> {
    let control = task_control(task_id);
    check_cancelled(task_id)?;
    check_cancelled_control(control.as_deref())?;

    let package_folder = request.package_folder.as_ref();
    let config = read_config().map_err(|error| error.to_string())?;
    let game_cfg = &config.game;
    let mods_dir = Path::new(package_folder).join("mods");
    let folder_name = request.folder_name.to_string();
    let version_config = get_version_config(folder_name.clone())
        .await
        .unwrap_or_default();

    let _ = set_task_labels(
        task_id,
        format!("启动 {}", request.display_name),
        Some(request.version.to_string()),
    );
    info!(
        task_id = %task_id,
        display_name = %request.display_name,
        version = %request.version,
        package_folder,
        "进入游戏启动主流程"
    );

    append_log(task_id, format!("版本目录: {package_folder}"));
    let injector_version = embedded_dll_version_string().unwrap_or_else(|| "unknown".to_string());
    append_log(task_id, format!("预加载器版本: {injector_version}"));

    let (identity_name, identity_version) = get_manifest_identity(package_folder)
        .await
        .map_err(|error| format!("Manifest 解析失败: {error}"))?;
    let is_win32 = is_win32_version(&identity_version);
    info!(
        task_id = %task_id,
        identity_name = %identity_name,
        identity_version = %identity_version,
        is_win32,
        "游戏包 Manifest 解析完成"
    );
    advance_step(
        task_id,
        "parsing",
        format!("版本信息已解析: {identity_version} ({identity_name})"),
    );

    let mut final_launch_args = request.launch_args.as_ref().map(ToString::to_string);
    if version_config.editor_mode
        && compare_versions(&identity_version, "1.19.80.20") != Ordering::Less
    {
        let is_preview = identity_name.contains("Beta") || identity_name.contains("Preview");
        final_launch_args = Some(build_editor_deeplink(is_win32, is_preview));
        append_log(
            task_id,
            "检测到编辑器模式，已生成 Deeplink 参数".to_string(),
        );
    }

    check_cancelled(task_id)?;
    let mut startup_mods_relative_paths = Vec::new();
    let mut delayed_mods = Vec::new();
    if request.auto_start
        && !version_config.disable_mod_loading
        && let Ok(mods) = load_mods_config(&mods_dir).await
    {
        for (path_buf, delay) in mods {
            let Some(path_string) = path_buf.to_str().map(ToString::to_string) else {
                continue;
            };
            if !is_win32 {
                let _ = grant_all_application_packages_access(&path_buf);
            }

            if delay == 0 {
                if let Some(file_name) = path_buf.file_name().and_then(|name| name.to_str()) {
                    startup_mods_relative_paths.push(format!("mods/{file_name}"));
                }
            } else {
                delayed_mods.push((path_string, delay));
            }
        }
    }
    debug!(
        task_id = %task_id,
        startup_mods = startup_mods_relative_paths.len(),
        delayed_mods = delayed_mods.len(),
        disable_mod_loading = version_config.disable_mod_loading,
        "模组注入计划已生成"
    );
    advance_step(
        task_id,
        "preparing_files",
        format!(
            "已准备模组加载信息，立即注入 {} 个，延迟注入 {} 个",
            startup_mods_relative_paths.len(),
            delayed_mods.len()
        ),
    );

    check_cancelled(task_id)?;
    if request.auto_start
        && let Some(exe_path) = find_game_executable(package_folder, &identity_name)
    {
        debug!(
            task_id = %task_id,
            exe_path = %exe_path.display(),
            "已定位游戏可执行文件"
        );
        let exe_dir = exe_path.parent().ok_or("无效的游戏目录".to_string())?;
        let local_data_root = exe_dir.join(BLOADER_DEFAULT_REDIRECTION_ROOT);
        if !local_data_root.exists() {
            fs::create_dir_all(&local_data_root)
                .map_err(|error| format!("创建重定向目录失败: {error}"))?;
        }
        let _ = grant_all_application_packages_access(&local_data_root);

        let injector_name = "BLoader.dll";
        let injector_target_path = exe_dir.join(injector_name);
        let mut need_update = true;
        if injector_target_path.exists() {
            remove_readonly(&injector_target_path);
            if let Ok(disk_bytes) = fs::read(&injector_target_path) {
                need_update = disk_bytes != INJECTOR_BYTES;
            }
        }

        if need_update {
            ensure_file_in_dir(exe_dir, injector_name, INJECTOR_BYTES)?;
        }

        let file_redirections =
            version_config.effective_file_redirections(Path::new(package_folder));
        if !file_redirections.is_empty() {
            append_log(
                task_id,
                format!("已配置 {} 条文件重定向", file_redirections.len()),
            );
        }

        let _ = write_bloader_config(
            exe_dir,
            version_config.disable_mod_loading,
            version_config.enable_redirection,
            json!(file_redirections),
            json!(startup_mods_relative_paths),
        )?;
        remove_legacy_preloader_config(exe_dir);

        if let Err(error) = ensure_backup(&exe_path) {
            warn!("无法创建 EXE 备份，将继续使用自标记还原机制: {error}");
        }

        if is_file_patched(&exe_path) {
            append_log(task_id, "检测到 PE 已包含补丁标记，跳过修补".to_string());
        } else {
            let _ = restore_original_pe(&exe_path);
            remove_readonly(&exe_path);
            inject_dll_import(&exe_path, injector_name, None)
                .map_err(|error| format!("PE 修改失败: {error}"))?;
            append_log(task_id, "静态注入环境已部署".to_string());
        }
    }
    advance_step(task_id, "patching", "启动环境准备完成".to_string());

    check_cancelled(task_id)?;
    if !is_win32 {
        if remove_appx_signature_if_present(package_folder)? {
            append_log(task_id, "检测到 AppxSignature.p7x，已删除".to_string());
        }

        let aumid = identity_to_aumid(&identity_name);
        let family_name = aumid.split('!').next().unwrap_or("");
        let mut need_remove = false;
        let mut need_register = true;

        if let Ok(Some((installed_version, _, _))) = get_package_info(&aumid) {
            let is_path_diff = if let Some(registered_path) = get_registered_path(family_name) {
                let registered_path =
                    fs::canonicalize(&registered_path).unwrap_or(registered_path.clone());
                let target_path = fs::canonicalize(Path::new(package_folder))
                    .unwrap_or_else(|_| PathBuf::from(package_folder));
                registered_path != target_path
            } else {
                true
            };
            let compare_result = compare_versions(&installed_version, &identity_version);
            if is_path_diff || compare_result == Ordering::Greater {
                need_remove = true;
            } else if installed_version == identity_version {
                need_register = false;
            }
        }

        if need_remove {
            info!(
                task_id = %task_id,
                family_name,
                "检测到旧注册信息，准备移除已注册包"
            );
            remove_package(family_name)
                .await
                .map_err(|error| format!("卸载旧包失败 ({family_name}): {error:?}"))?;
            sleep(Duration::from_millis(500)).await;
        }
        if need_register {
            info!(task_id = %task_id, package_folder, "准备注册 APPX 包");
            register_appx_package_async(package_folder)
                .await
                .map_err(|error| format!("注册 APPX 失败 ({package_folder}): {error:?}"))?;
        }
        advance_step(task_id, "initializing", "APPX 注册状态已就绪".to_string());
    } else {
        advance_step(
            task_id,
            "initializing",
            "Win32 版本无需重新注册".to_string(),
        );
    }

    check_cancelled(task_id)?;
    if !request.auto_start {
        info!(task_id = %task_id, "本次仅执行准备流程，不实际启动游戏");
        advance_step(task_id, "launching", "已完成准备，未执行启动".to_string());
        return Ok(None);
    }

    if !is_win32 && game_cfg.uwp_minimize_fix {
        if let Ok(Some((_, _, package_name))) = get_package_info(&identity_to_aumid(&identity_name))
        {
            let _ = enable_debugging_for_package(&package_name);
        }
    }

    let pid = if is_win32 {
        let exe_path = find_game_executable(package_folder, &identity_name)
            .ok_or("未找到游戏 EXE".to_string())?;
        let exe_path = exe_path
            .to_str()
            .ok_or("游戏 EXE 路径包含无效字符".to_string())?;
        let log_task_id = task_id.to_string();
        let log_callback = Arc::new(move |message: String| {
            append_log(&log_task_id, message);
        });
        info!(task_id = %task_id, exe_path, "准备启动 Win32 版本");
        let pid = launch_win32_with_injection(
            exe_path,
            final_launch_args.as_deref(),
            Vec::new(),
            false,
            Some(log_callback.clone()),
        )
        .await
        .map_err(|error| format!("启动失败: {error:?}"))?;
        if !version_config.disable_mod_loading {
            handle_delayed_injection(pid, delayed_mods, log_callback, false);
        }
        info!(task_id = %task_id, pid, "Win32 版本启动成功");
        pid
    } else {
        let aumid = identity_to_aumid(&identity_name);
        info!(
            task_id = %task_id,
            aumid = %aumid,
            launch_args = ?final_launch_args.as_deref(),
            "准备启动 UWP 版本"
        );
        let activated_pid =
            launch_uwp_command_only(&aumid, final_launch_args.as_deref().or(Some("")))
                .await
                .map_err(|error| format!("启动请求失败: {error:?}"))?;
        let target_exe = if identity_name.contains("Education") {
            "Minecraft.Education.exe"
        } else {
            "Minecraft.Windows.exe"
        };
        let pfn = aumid.split('!').next().unwrap_or("").to_string();
        let pid = match activated_pid {
            Some(pid) if pid > 0 => pid,
            _ => wait_for_uwp_pid(target_exe, &pfn)
                .await
                .ok_or("启动超时".to_string())?,
        };
        if !version_config.disable_mod_loading {
            let log_task_id = task_id.to_string();
            handle_delayed_injection(
                pid,
                delayed_mods,
                Arc::new(move |message: String| {
                    append_log(&log_task_id, message);
                }),
                false,
            );
        }
        info!(task_id = %task_id, pid, "UWP 版本启动成功");
        pid
    };

    if version_config.lock_mouse_on_launch {
        start_window_monitor(
            "Minecraft",
            &version_config.unlock_mouse_hotkey,
            version_config.reduce_pixels,
        );
    }

    advance_step(task_id, "launching", format!("游戏已成功拉起，PID {pid}"));
    info!(task_id = %task_id, pid, "游戏启动流程已完成");
    Ok(Some(pid))
}

fn handle_delayed_injection(
    pid: u32,
    mods: Vec<(String, u64)>,
    log_callback: Arc<dyn Fn(String) + Send + Sync>,
    show_console: bool,
) {
    if mods.is_empty() {
        return;
    }

    tokio::spawn(async move {
        for (path, delay) in mods {
            sleep(Duration::from_millis(delay)).await;
            let _ =
                inject_existing_process(pid, path, Some(log_callback.clone()), true, show_console)
                    .await;
        }
    });
}
pub fn build_package_folder(folder_name: &str) -> PathBuf {
    file_ops::bmcbl_subdir("versions").join(folder_name)
}
