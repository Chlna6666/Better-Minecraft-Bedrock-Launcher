use anyhow::{anyhow, Context, Result};
use std::ffi::OsStr;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use windows::core::{PCSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::{
    GetThreadContext, SetThreadContext, WriteProcessMemory, CONTEXT, CONTEXT_FLAGS,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READWRITE,
};
use windows::Win32::System::Threading::{
    CreateProcessW, CreateRemoteThread, ResumeThread, WaitForSingleObject,
    CREATE_SUSPENDED, CREATE_NEW_CONSOLE, // [核心] 确保引入此标志
    INFINITE, PROCESS_INFORMATION, STARTUPINFOW,
};

pub type InjectProgressCb = Arc<dyn Fn(String) + Send + Sync>;

// 在你的启动器代码中修改
pub fn grant_all_application_packages_access(path: &Path) -> anyhow::Result<()> {
    // S-1-15-2-1: All Application Packages (所有应用程序包)
    // S-1-5-32-545: Users (普通用户组)

    let output = Command::new("icacls")
        .arg(path)
        .arg("/grant")
        .arg("*S-1-15-2-1:(OI)(CI)M") // [安全修复] 降级为 Modify (M)，游戏只需读写，不需要完全控制
        .arg("/grant")
        .arg("*S-1-5-32-545:(OI)(CI)F") // [Bug修复] 给予用户组 Full (F) 权限，确保用户可以手动删除文件
        .arg("/T") // 递归应用到子文件
        .arg("/Q") // 静默模式
        .output()
        .map_err(|e| anyhow::Error::msg(format!("Failed to execute icacls: {}", e)))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        // 记录警告但不中断流程
        eprintln!("Warning: icacls warning for {:?}: {}", path, err);
    }
    Ok(())
}

pub async fn launch_win32_with_injection(
    exe_path: &str,
    args: Option<&str>,
    dll_paths: Vec<String>,
    enable_console: bool,
    on_progress: Option<InjectProgressCb>,
) -> Result<u32> {
    let exe_path_owned = exe_path.to_string();
    let args_owned = args.map(|s| s.to_string());
    let cb = on_progress.clone();

    tokio::task::spawn_blocking(move || -> Result<u32> {
        unsafe {
            let log = |msg: &str| { if let Some(c) = &cb { c(msg.to_string()); } };

            let mut si = STARTUPINFOW::default();
            si.cb = mem::size_of::<STARTUPINFOW>() as u32;
            let mut pi = PROCESS_INFORMATION::default();

            // [核心修正]
            // 1. 基础标志：挂起进程 (为了注入)
            let mut creation_flags = CREATE_SUSPENDED;

            // 2. 控制台标志：如果启用，则强制请求新窗口
            // 只要加上这个标志，Windows 就会负责弹出默认的终端应用 (Terminal 或 CMD)
            if enable_console {
                creation_flags |= CREATE_NEW_CONSOLE;
                log("启动标志: CREATE_NEW_CONSOLE (请求独立终端窗口)");
            }

            let mut cmd_line_str = format!("\"{}\"", exe_path_owned);
            if let Some(a) = &args_owned {
                cmd_line_str.push_str(" ");
                cmd_line_str.push_str(a);
            }
            let wide_cmd: Vec<u16> = OsStr::new(&cmd_line_str).encode_wide().chain(Some(0)).collect();

            CreateProcessW(
                None,
                Option::from(PWSTR(wide_cmd.as_ptr() as *mut _)),
                None,
                None,
                false, // [关键] 设为 false，彻底切断与启动器终端的继承关系，保证窗口独立
                creation_flags,
                None,
                None,
                &si,
                &mut pi,
            ).map_err(|e| anyhow!("CreateProcessW failed: {:?}", e))?;

            let h_proc = pi.hProcess;
            let h_thread = pi.hThread;
            let pid = pi.dwProcessId;
            log(&format!("进程已挂起启动 PID: {}", pid));

            if !dll_paths.is_empty() {
                let h_kernel = GetModuleHandleW(windows::core::w!("kernel32.dll"))?;
                let load_lib_addr = GetProcAddress(h_kernel, PCSTR(b"LoadLibraryW\0".as_ptr()))
                    .ok_or_else(|| anyhow!("LoadLibraryW not found"))? as u64;

                // [说明] 移除了 AllocConsole 的注入逻辑
                // 因为我们已经使用了 CREATE_NEW_CONSOLE，系统会在进程启动时自动分配控制台

                let mut path_addrs = Vec::new();
                for path in &dll_paths {
                    let wpath: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
                    let len = wpath.len() * 2;
                    let mem = VirtualAllocEx(h_proc, None, len, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
                    if !mem.is_null() {
                        WriteProcessMemory(h_proc, mem, wpath.as_ptr() as _, len, None)?;
                        path_addrs.push(mem as u64);
                        log(&format!("注入准备: {}", path));
                    }
                }

                let mut ctx: CONTEXT = mem::zeroed();
                ctx.ContextFlags = CONTEXT_FLAGS(0x100001);
                GetThreadContext(h_thread, &mut ctx)?;

                let mut shellcode = Vec::new();
                shellcode.extend_from_slice(&[0x48, 0x83, 0xEC, 0x28, 0x50, 0x53, 0x51, 0x52, 0x41, 0x50, 0x41, 0x51, 0x41, 0x52, 0x41, 0x53]);

                for path_addr in path_addrs {
                    shellcode.extend_from_slice(&[0x48, 0xB9]);
                    shellcode.extend_from_slice(&path_addr.to_le_bytes());
                    shellcode.extend_from_slice(&[0x48, 0xB8]);
                    shellcode.extend_from_slice(&load_lib_addr.to_le_bytes());
                    shellcode.extend_from_slice(&[0xFF, 0xD0]);
                }

                shellcode.extend_from_slice(&[0x41, 0x5B, 0x41, 0x5A, 0x41, 0x59, 0x41, 0x58, 0x5A, 0x59, 0x5B, 0x58, 0x48, 0x83, 0xC4, 0x28]);
                shellcode.extend_from_slice(&[0x48, 0xB8]);
                shellcode.extend_from_slice(&ctx.Rip.to_le_bytes());
                shellcode.extend_from_slice(&[0xFF, 0xE0]);

                let shellcode_mem = VirtualAllocEx(h_proc, None, shellcode.len(), MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
                WriteProcessMemory(h_proc, shellcode_mem, shellcode.as_ptr() as _, shellcode.len(), None)?;

                ctx.Rip = shellcode_mem as u64;
                SetThreadContext(h_thread, &ctx)?;
            }

            ResumeThread(h_thread);
            let _ = CloseHandle(h_proc);
            let _ = CloseHandle(h_thread);
            Ok(pid)
        }
    }).await?
}

// inject_existing_process 代码保持原样，因为它是针对已存在进程的
pub async fn inject_existing_process(
    pid: u32,
    dll_path: String,
    on_progress: Option<InjectProgressCb>,
    skip_acl: bool,
    enable_console: bool,
) -> Result<()> {
    let cb = on_progress.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        unsafe {
            let log = |msg: &str| { if let Some(c) = &cb { c(msg.to_string()); } };

            if !skip_acl {
                let path_obj = Path::new(&dll_path);
                let _ = grant_all_application_packages_access(path_obj);
            }

            let h_proc = windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_ALL_ACCESS,
                false,
                pid,
            ).map_err(|e| anyhow!("OpenProcess failed: {:?}", e))?;

            // 对现有进程，只能尝试 RemoteThread 调用 AllocConsole/FreeConsole
            if enable_console {
                let h_kernel = GetModuleHandleW(windows::core::w!("kernel32.dll"))?;

                if let Some(free_console_addr) = GetProcAddress(h_kernel, PCSTR(b"FreeConsole\0".as_ptr())) {
                    let h_free = CreateRemoteThread(
                        h_proc, None, 0, Some(mem::transmute(free_console_addr)), None, 0, None
                    );
                    if let Ok(h) = h_free { WaitForSingleObject(h, 1000); CloseHandle(h); }
                }

                if let Some(alloc_console_addr) = GetProcAddress(h_kernel, PCSTR(b"AllocConsole\0".as_ptr())) {
                    let h_console_thread = CreateRemoteThread(
                        h_proc, None, 0, Some(mem::transmute(alloc_console_addr)), None, 0, None
                    );

                    if let Ok(h) = h_console_thread {
                        WaitForSingleObject(h, INFINITE);
                        let _ = CloseHandle(h);
                    }
                }
            }

            let wide_path: Vec<u16> = OsStr::new(&dll_path).encode_wide().chain(Some(0)).collect();
            let len = wide_path.len() * 2;
            let remote_mem = VirtualAllocEx(h_proc, None, len, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);

            if remote_mem.is_null() {
                let _ = CloseHandle(h_proc);
                return Err(anyhow!("VirtualAllocEx failed"));
            }
            WriteProcessMemory(h_proc, remote_mem, wide_path.as_ptr() as _, len, None)?;

            let h_kernel = GetModuleHandleW(windows::core::w!("kernel32.dll"))?;
            let load_lib = GetProcAddress(h_kernel, PCSTR(b"LoadLibraryW\0".as_ptr()))
                .ok_or_else(|| anyhow!("LoadLibraryW not found"))?;

            let h_thread = CreateRemoteThread(
                h_proc, None, 0, Some(mem::transmute(load_lib)), Some(remote_mem), 0, None
            ).map_err(|e| anyhow!("CreateRemoteThread failed: {:?}", e))?;

            WaitForSingleObject(h_thread, INFINITE);

            let _ = VirtualFreeEx(h_proc, remote_mem, 0, MEM_RELEASE);
            let _ = CloseHandle(h_thread);
            let _ = CloseHandle(h_proc);

            log(&format!("注入完成: {}", dll_path));
            Ok(())
        }
    }).await?
}