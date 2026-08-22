#![cfg(target_os = "windows")]

use anyhow::{Result, anyhow};
use std::ffi::{OsStr, c_void};
use std::fs;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use windows::Win32::Foundation::{
    APPMODEL_ERROR_NO_PACKAGE, CloseHandle, ERROR_INSUFFICIENT_BUFFER, GetLastError, HANDLE,
    WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenProcess, PROCESS_CREATE_THREAD,
    PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
    WaitForSingleObject,
};
use windows::core::PCSTR;

pub use super::inject_legacy::{
    InjectProgressCb, launch_win32_with_injection, register_xuser_launch_payload,
    terminal_host_launch_metadata,
};

const ALL_APPLICATION_PACKAGES_SID: &str = "*S-1-15-2-1";
const INJECT_THREAD_TIMEOUT_MS: u32 = 30_000;

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: this wrapper owns handles returned by OpenProcess/CreateRemoteThread.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct RemoteAllocation {
    process: HANDLE,
    address: *mut c_void,
}

impl RemoteAllocation {
    fn new(process: HANDLE, address: *mut c_void) -> Self {
        Self { process, address }
    }
}

impl Drop for RemoteAllocation {
    fn drop(&mut self) {
        if !self.address.is_null() {
            // SAFETY: address was allocated in process by VirtualAllocEx with MEM_RESERVE|MEM_COMMIT.
            unsafe {
                let _ = VirtualFreeEx(self.process, self.address, 0, MEM_RELEASE);
            }
        }
    }
}

fn win32_error_hint(code: u32) -> &'static str {
    match code {
        5 => "访问被拒绝；检查目标进程权限、AppContainer 限制以及 DLL ACL",
        6 => "目标进程句柄已失效；Minecraft 可能已经退出或正在终止",
        8 | 14 => "系统内存不足，无法为远程注入分配资源",
        87 => "参数无效；PID 可能已经复用或目标进程状态发生变化",
        126 => "模块或其依赖项未找到",
        127 => "目标函数未找到",
        193 | 216 => "可执行映像架构不兼容；检查 DLL 与 Minecraft 的 x64/ARM64 架构",
        299 => "只能完成部分内存访问；常见于跨架构注入或目标进程正在退出",
        487 | 998 => "目标进程内存不可访问；目标可能正在退出或受到额外保护",
        _ => "请结合注入阶段、Win32 错误码和目标进程状态继续定位",
    }
}

fn classify_windows_error(stage: &str, pid: u32, error: windows::core::Error) -> anyhow::Error {
    if let Some(code) = WIN32_ERROR::from_error(&error) {
        anyhow!(
            "{stage} 失败 (PID {pid}, Win32={}): {error}; {}",
            code.0,
            win32_error_hint(code.0)
        )
    } else {
        anyhow!("{stage} 失败 (PID {pid}): {error}")
    }
}

fn classify_last_error(stage: &str, pid: u32, code: u32) -> anyhow::Error {
    anyhow!(
        "{stage} 失败 (PID {pid}, Win32={code}): {}",
        win32_error_hint(code)
    )
}

fn is_packaged_process(process: HANDLE, pid: u32) -> Result<bool> {
    let mut family_name_len = 0u32;
    // A null output buffer intentionally probes whether the process owns package identity.
    // Packaged processes return ERROR_INSUFFICIENT_BUFFER with the required length; ordinary
    // Win32 processes return APPMODEL_ERROR_NO_PACKAGE.
    let status = unsafe { GetPackageFamilyName(process, &mut family_name_len, None) };
    if status == ERROR_INSUFFICIENT_BUFFER {
        return Ok(true);
    }
    if status == APPMODEL_ERROR_NO_PACKAGE {
        return Ok(false);
    }
    Err(anyhow!(
        "GetPackageFamilyName 失败 (PID {pid}, Win32={}): 无法判定目标是否为 AppContainer/打包进程",
        status.0
    ))
}

/// Grants only the access required by packaged/UWP processes.
///
/// Files (DLL/config) receive read+execute access. Directories receive modify access with
/// inheritance so BLoader redirection roots can still create/update data beneath them.
/// The previous blanket BUILTIN\\Users full-control grant is deliberately not reproduced.
pub fn grant_all_application_packages_access(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)
        .map_err(|error| anyhow!("读取 ACL 目标元数据失败 {}: {error}", path.display()))?;

    let permission = if metadata.is_dir() {
        format!("{ALL_APPLICATION_PACKAGES_SID}:(OI)(CI)M")
    } else {
        format!("{ALL_APPLICATION_PACKAGES_SID}:RX")
    };

    let mut command = Command::new("icacls");
    command.arg(path).arg("/grant:r").arg(permission);
    if metadata.is_dir() {
        command.arg("/T");
    }
    command.arg("/Q");

    let output = command
        .output()
        .map_err(|error| anyhow!("执行 icacls 失败 {}: {error}", path.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(anyhow!(
            "设置 AppContainer ACL 失败 {} (status={}): {}{}{}",
            path.display(),
            output.status,
            stderr,
            if !stderr.is_empty() && !stdout.is_empty() { " | " } else { "" },
            stdout
        ));
    }

    Ok(())
}

unsafe fn configure_remote_console(
    process: HANDLE,
    pid: u32,
    log: &dyn Fn(&str),
) -> Result<()> {
    let kernel = unsafe { GetModuleHandleW(windows::core::w!("kernel32.dll")) }
        .map_err(|error| classify_windows_error("GetModuleHandleW(kernel32)", pid, error))?;

    if let Some(free_console) = unsafe { GetProcAddress(kernel, PCSTR(b"FreeConsole\0".as_ptr())) } {
        match unsafe {
            CreateRemoteThread(
                process,
                None,
                0,
                Some(mem::transmute(free_console)),
                None,
                0,
                None,
            )
        } {
            Ok(thread) => {
                let thread = OwnedHandle(thread);
                unsafe {
                    WaitForSingleObject(thread.raw(), 1_000);
                }
            }
            Err(error) => log(&format!(
                "远程 FreeConsole 跳过：{}",
                classify_windows_error("CreateRemoteThread(FreeConsole)", pid, error)
            )),
        }
    }

    if let Some(alloc_console) = unsafe { GetProcAddress(kernel, PCSTR(b"AllocConsole\0".as_ptr())) } {
        match unsafe {
            CreateRemoteThread(
                process,
                None,
                0,
                Some(mem::transmute(alloc_console)),
                None,
                0,
                None,
            )
        } {
            Ok(thread) => {
                let thread = OwnedHandle(thread);
                let wait = unsafe { WaitForSingleObject(thread.raw(), INJECT_THREAD_TIMEOUT_MS) };
                if wait == WAIT_TIMEOUT {
                    log("远程 AllocConsole 超时，继续 DLL 注入");
                } else if wait == WAIT_FAILED {
                    let code = unsafe { GetLastError() }.0;
                    log(&format!(
                        "远程 AllocConsole 等待失败：{}",
                        classify_last_error("WaitForSingleObject(AllocConsole)", pid, code)
                    ));
                }
            }
            Err(error) => log(&format!(
                "远程 AllocConsole 跳过：{}",
                classify_windows_error("CreateRemoteThread(AllocConsole)", pid, error)
            )),
        }
    }

    Ok(())
}

pub async fn inject_existing_process(
    pid: u32,
    dll_path: String,
    on_progress: Option<InjectProgressCb>,
    skip_acl: bool,
    enable_console: bool,
) -> Result<()> {
    let callback = on_progress.clone();

    crate::tasks::runtime::run_io_blocking(move || -> Result<()> {
        let log = |message: &str| {
            if let Some(callback) = &callback {
                callback(message.to_string());
            }
        };

        let dll = Path::new(&dll_path);
        let metadata = fs::metadata(dll)
            .map_err(|error| anyhow!("注入 DLL 不可访问 {}: {error}", dll.display()))?;
        if !metadata.is_file() {
            return Err(anyhow!("注入目标不是普通文件: {}", dll.display()));
        }

        let desired_access = PROCESS_CREATE_THREAD
            | PROCESS_QUERY_INFORMATION
            | PROCESS_VM_OPERATION
            | PROCESS_VM_WRITE
            | PROCESS_VM_READ;
        let process = unsafe { OpenProcess(desired_access, false, pid) }
            .map_err(|error| classify_windows_error("OpenProcess(最小注入权限)", pid, error))?;
        let process = OwnedHandle(process);
        log(&format!("已使用最小注入权限打开目标进程 PID {pid}"));

        let packaged = is_packaged_process(process.raw(), pid)?;
        if packaged {
            grant_all_application_packages_access(dll).map_err(|error| {
                anyhow!(
                    "AppContainer DLL ACL 准备失败 (PID {pid}, {}): {error}",
                    dll.display()
                )
            })?;
            log("检测到打包/AppContainer 目标，已确保 DLL 仅授予 ALL APPLICATION PACKAGES 读取/执行权限");
        } else if !skip_acl {
            grant_all_application_packages_access(dll)?;
        }

        if enable_console {
            // SAFETY: process is a live process handle opened with PROCESS_CREATE_THREAD.
            unsafe {
                configure_remote_console(process.raw(), pid, &log)?;
            }
        }

        let wide_path: Vec<u16> = OsStr::new(&dll_path).encode_wide().chain(Some(0)).collect();
        let length = wide_path.len() * mem::size_of::<u16>();
        let remote_memory = unsafe {
            VirtualAllocEx(
                process.raw(),
                None,
                length,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if remote_memory.is_null() {
            let code = unsafe { GetLastError() }.0;
            return Err(classify_last_error("VirtualAllocEx(DLL 路径)", pid, code));
        }
        let remote_memory = RemoteAllocation::new(process.raw(), remote_memory);

        unsafe {
            WriteProcessMemory(
                process.raw(),
                remote_memory.address,
                wide_path.as_ptr().cast(),
                length,
                None,
            )
        }
        .map_err(|error| classify_windows_error("WriteProcessMemory(DLL 路径)", pid, error))?;

        let kernel = unsafe { GetModuleHandleW(windows::core::w!("kernel32.dll")) }
            .map_err(|error| classify_windows_error("GetModuleHandleW(kernel32)", pid, error))?;
        let load_library = unsafe { GetProcAddress(kernel, PCSTR(b"LoadLibraryW\0".as_ptr())) }
            .ok_or_else(|| anyhow!("无法解析本机 kernel32!LoadLibraryW"))?;

        let remote_thread = unsafe {
            CreateRemoteThread(
                process.raw(),
                None,
                0,
                Some(mem::transmute(load_library)),
                Some(remote_memory.address),
                0,
                None,
            )
        }
        .map_err(|error| classify_windows_error("CreateRemoteThread(LoadLibraryW)", pid, error))?;
        let remote_thread = OwnedHandle(remote_thread);

        let wait = unsafe { WaitForSingleObject(remote_thread.raw(), INJECT_THREAD_TIMEOUT_MS) };
        if wait == WAIT_TIMEOUT {
            return Err(anyhow!(
                "LoadLibraryW 远程线程等待超过 {} ms (PID {pid})；DLL DllMain 可能阻塞或目标进程已卡死",
                INJECT_THREAD_TIMEOUT_MS
            ));
        }
        if wait == WAIT_FAILED {
            let code = unsafe { GetLastError() }.0;
            return Err(classify_last_error(
                "WaitForSingleObject(LoadLibraryW)",
                pid,
                code,
            ));
        }
        if wait != WAIT_OBJECT_0 {
            return Err(anyhow!(
                "LoadLibraryW 远程线程返回未知等待状态 {:?} (PID {pid})",
                wait
            ));
        }

        let mut exit_code = 0u32;
        unsafe { GetExitCodeThread(remote_thread.raw(), &mut exit_code) }
            .map_err(|error| classify_windows_error("GetExitCodeThread(LoadLibraryW)", pid, error))?;
        if exit_code == 0 {
            return Err(anyhow!(
                "LoadLibraryW 返回 NULL (PID {pid}, DLL={}); 优先检查 AppContainer 文件可见性/ACL、DLL 依赖缺失、架构不匹配以及 DllMain 初始化失败",
                dll.display()
            ));
        }

        log(&format!(
            "注入完成: {dll_path} (PID {pid}, LoadLibraryW=0x{exit_code:08X})"
        ));
        Ok(())
    })
    .await
    .map_err(anyhow::Error::msg)?
}
