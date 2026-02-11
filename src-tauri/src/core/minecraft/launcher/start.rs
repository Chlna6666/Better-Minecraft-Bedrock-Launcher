use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::os::windows::process::CommandExt; // [新增] 引入 CommandExt 以支持 creation_flags
use std::ptr;
use std::time::{Duration, Instant};
use tracing::{info, warn, debug, error};
use windows::core::{HSTRING, Result as WindowsResult, HRESULT, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WIN32_ERROR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, CREATE_NO_WINDOW
};
use windows::Win32::UI::Shell::{
    ApplicationActivationManager, IApplicationActivationManager, ACTIVATEOPTIONS,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
use windows::Foundation::Uri;
use windows::System::{Launcher, LauncherOptions};
use windows::core::PWSTR;

// 检查 PID 是否属于目标包
pub fn is_process_in_package(pid: u32, target_family_name: &str) -> bool {
    unsafe {
        let Ok(h_proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false; };
        if h_proc.is_invalid() { return false; }

        let mut len: u32 = 0;
        let _ = GetPackageFamilyName(h_proc, &mut len, None);
        if len == 0 { let _ = CloseHandle(h_proc); return false; }

        let mut buffer = vec![0u16; len as usize];
        let res = GetPackageFamilyName(h_proc, &mut len, Some(PWSTR(buffer.as_mut_ptr())));
        let _ = CloseHandle(h_proc);

        if res == WIN32_ERROR(0) {
            let family = String::from_utf16_lossy(&buffer).trim_matches('\0').to_string();
            return family.eq_ignore_ascii_case(target_family_name);
        }
    }
    false
}

// 获取系统当前所有匹配名称的 PID
pub fn get_pids_by_name(exe_name: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return pids; };
        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&entry.szExeFile).trim_matches('\0').to_string();
                if name.eq_ignore_ascii_case(exe_name) { pids.push(entry.th32ProcessID); }
                if Process32NextW(snapshot, &mut entry).is_err() { break; }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    pids
}

// [修复] 纯启动命令，现在正确处理 launch_args
pub async fn launch_uwp_command_only(app_user_model_id: &str, launch_args: Option<&str>) -> WindowsResult<bool> {
    debug!("Executing launch_uwp_command_only: AUMID={}, Args={:?}", app_user_model_id, launch_args);
    unsafe { let _ = CoInitializeEx(Some(ptr::null()), COINIT_APARTMENTTHREADED); };

    if let Some(args) = launch_args {
        // 协议启动 (minecraft://)
        if args.contains("://") {
            info!("Launch strategy: Protocol Handler");
            let uri = Uri::CreateUri(&HSTRING::from(args))?;
            let pfn = app_user_model_id.split('!').next().unwrap_or(app_user_model_id);
            let options = LauncherOptions::new()?;
            options.SetTargetApplicationPackageFamilyName(&HSTRING::from(pfn))?;
            return Launcher::LaunchUriWithOptionsAsync(&uri, &options)?.await;
        }
    }

    // IApplicationActivationManager 启动
    info!("Launch strategy: IApplicationActivationManager");
    let activator: IApplicationActivationManager = unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL)? };

    // [关键修复] 将 launch_args 转换为 HSTRING 传入，而不是传入空字符串
    let args_hstring = if let Some(a) = launch_args { HSTRING::from(a) } else { HSTRING::new() };

    unsafe {
        activator.ActivateApplication(
            &HSTRING::from(app_user_model_id),
            &args_hstring,
            ACTIVATEOPTIONS(0)
        )
    }?;
    Ok(true)
}

// =================================================================================
// UWP 启动逻辑 (保留原有逻辑并修复参数)
// =================================================================================

async fn launch_uwp_with_uri(app_user_model_id: &str, uri_str: &str) -> WindowsResult<bool> {
    unsafe {
        let _ = CoInitializeEx(Some(ptr::null()), COINIT_APARTMENTTHREADED);
    };
    let uri = Uri::CreateUri(&HSTRING::from(uri_str))?;
    let pfn = app_user_model_id.split('!').next().unwrap_or(app_user_model_id);

    let options = LauncherOptions::new()?;
    options.SetTargetApplicationPackageFamilyName(&HSTRING::from(pfn))?;

    Launcher::LaunchUriWithOptionsAsync(&uri, &options)?.await
}

fn launch_uwp_winapi(app_user_model_id: &str, args: Option<&str>) -> Result<u32, windows::core::Error> {
    let hr = unsafe { CoInitializeEx(Some(ptr::null()), COINIT_APARTMENTTHREADED) };
    if !hr.is_ok() && hr != HRESULT(0x80010106u32 as i32) {}

    let activator: IApplicationActivationManager =
        unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL)? };

    let args_h = if let Some(a) = args { HSTRING::from(a) } else { HSTRING::new() };

    let result = unsafe {
        activator.ActivateApplication(
            &HSTRING::from(app_user_model_id),
            &args_h,
            ACTIVATEOPTIONS(0),
        )
    };
    result
}

// 智能启动 UWP 并返回 PID (完整流程)
pub async fn launch_uwp(edition: &str, launch_args: Option<&str>) -> io::Result<Option<u32>> {
    let app_user_model_id = match edition {
        "Microsoft.MinecraftUWP" => "Microsoft.MinecraftUWP_8wekyb3d8bbwe!App",
        "Microsoft.MinecraftWindowsBeta" => "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe!App",
        "Microsoft.MinecraftEducationEdition" => "Microsoft.MinecraftEducationEdition_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition",
        "Microsoft.MinecraftEducationPreview" => "Microsoft.MinecraftEducationPreview_8wekyb3d8bbwe!Microsoft.MinecraftEducationEdition",
        _ => return Ok(None),
    };

    let package_family_name = app_user_model_id.split('!').next().unwrap_or("");
    let target_exe_name = if edition.contains("Education") { "Minecraft.Education.exe" } else { "Minecraft.Windows.exe" };

    let pids_before = get_pids_by_name(target_exe_name);
    debug!("PIDs before launch: {:?}", pids_before);

    let mut launched_via_uri = false;

    if let Some(args) = launch_args {
        if args.contains("://") {
            info!("Launching via URI: {}", args);
            match launch_uwp_with_uri(app_user_model_id, args).await {
                Ok(true) => {
                    info!("URI launch successful");
                    launched_via_uri = true;
                },
                Ok(false) => warn!("URI launch returned false"),
                Err(e) => warn!("URI launch failed: {:?}", e),
            }
        }
    }

    if !launched_via_uri {
        info!("Launching via WinAPI ActivateApplication");
        // [修复] 传入参数
        match launch_uwp_winapi(app_user_model_id, launch_args) {
            Ok(pid) => {
                info!("WinAPI returned PID: {}", pid);
                return Ok(Some(pid));
            },
            Err(e) => {
                warn!("WinAPI launch failed: {:?}, fallback to Explorer", e);
                let _ = Command::new("explorer.exe")
                    .arg(format!("shell:appsFolder\\{}", app_user_model_id))
                    .spawn();
                launched_via_uri = true;
            }
        }
    }

    // 智能 PID 查找逻辑
    let start_time = Instant::now();
    let timeout = Duration::from_secs(8);
    info!("Waiting for process {}...", target_exe_name);

    while start_time.elapsed() < timeout {
        let pids_now = get_pids_by_name(target_exe_name);

        // 1. 检查新增进程
        let new_pids: Vec<u32> = pids_now.iter()
            .filter(|pid| !pids_before.contains(pid))
            .cloned()
            .collect();

        for pid in new_pids {
            if is_process_in_package(pid, package_family_name) {
                info!("Found new process: PID {}", pid);
                tokio::time::sleep(Duration::from_millis(500)).await;
                return Ok(Some(pid));
            }
        }

        // 2. 检查现有进程被激活 (UWP 单实例特性)
        if start_time.elapsed() > Duration::from_millis(1500) {
            for pid in pids_now {
                if is_process_in_package(pid, package_family_name) {
                    info!("Existing process activated: PID {}", pid);
                    return Ok(Some(pid));
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    warn!("Launch timeout, PID not found");
    Ok(None)
}

/// 启动 Win32/GDK 版本
/// [修改] 添加 enable_console 参数
pub fn launch_win32(package_folder: &str, launch_args: Option<&str>, enable_console: bool) -> io::Result<Option<u32>> {
    let folder = Path::new(package_folder);
    if !folder.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Package folder not found: {}", package_folder),
        ));
    }

    let try_spawn = |path: &PathBuf| -> io::Result<u32> {
        let mut cmd = Command::new(path);
        if let Some(parent) = path.parent() {
            cmd.current_dir(parent);
        } else {
            cmd.current_dir(folder);
        }

        if let Some(arg) = launch_args {
            cmd.arg(arg);
        }

        // [新增] 如果启用控制台，设置 CREATE_NEW_CONSOLE (0x10)
        if enable_console {
            cmd.creation_flags(0x00000010);
            debug!("Setting creation flags: CREATE_NEW_CONSOLE for Win32 launch");
        }

        info!("Spawning Win32 process: {:?}", cmd);
        let child = cmd.spawn()?;
        Ok(child.id())
    };
    let main_exe = folder.join("Minecraft.Windows.exe");
    if main_exe.exists() {
        match try_spawn(&main_exe) {
            Ok(pid) => return Ok(Some(pid)),
            Err(e) => error!("Failed to launch Minecraft.Windows.exe: {:?}", e),
        }
    }

    // 扫描其他 EXE
    if let Ok(rd) = std::fs::read_dir(folder) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p == main_exe { continue; }
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                if ext.eq_ignore_ascii_case("exe") {
                    if let Ok(pid) = try_spawn(&p) {
                        return Ok(Some(pid));
                    }
                }
            }
        }
    }

    Ok(None)
}