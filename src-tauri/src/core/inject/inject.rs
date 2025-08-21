use anyhow::{anyhow, Context, Result};
use std::{ffi::OsStr, fs, mem, os::windows::ffi::OsStrExt, path::Path};
use tracing::debug;
use windows::core::PCSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::Memory::{
    VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenProcess, WaitForSingleObject,
    INFINITE, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_WRITE,
};

/// RAII 封装，自动关闭 HANDLE
struct RemoteHandle(HANDLE);
impl Drop for RemoteHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_invalid() {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// RAII 封装，自动释放远程内存
struct RemoteMemory<'a> {
    process: &'a RemoteHandle,
    address: *mut std::ffi::c_void,
}
impl<'a> Drop for RemoteMemory<'a> {
    fn drop(&mut self) {
        unsafe {
            let _ = VirtualFreeEx(
                self.process.0,
                self.address,
                0,
                MEM_RELEASE,
            );
        }
    }
}

/// 查找指定可执行名对应的 PID
pub fn find_pid(exe_name: &str) -> Result<u32> {
    unsafe {
        // 创建进程快照
        let raw_snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| anyhow!("CreateToolhelp32Snapshot 失败: {}", e.message()))?;
        if raw_snap == INVALID_HANDLE_VALUE {
            return Err(anyhow!("CreateToolhelp32Snapshot 返回 INVALID_HANDLE_VALUE"));
        }
        let snapshot = RemoteHandle(raw_snap);

        // 准备 PROCESSENTRY32W 结构
        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;

        // 首次枚举进程列表（失败会直接返回 Err）
        Process32FirstW(snapshot.0, &mut entry)
            .map_err(|e| anyhow!("Process32FirstW 失败: {}", e.message()))?;

        // 检查第一个结果
        loop {
            // 从 UTF-16 名称数组里取出可执行文件名
            let name = String::from_utf16_lossy(&entry.szExeFile)
                .trim_end_matches('\0')
                .to_string();
            if name.eq_ignore_ascii_case(exe_name) {
                return Ok(entry.th32ProcessID);
            }
            // 继续枚举，失败（包括没有更多进程）就跳出循环
            if Process32NextW(snapshot.0, &mut entry).is_err() {
                break;
            }
        }
    }

    Err(anyhow!("未找到名为 `{}` 的进程", exe_name))
}

/// 将单个 DLL 注入到指定 PID
fn safe_inject(pid: u32, dll_path: &Path) -> Result<()> {
    // 规范化路径并检查文件存在性
    let dll_abs = dll_path
        .canonicalize()
        .with_context(|| format!("无法规范化路径 `{}`", dll_path.display()))?;
    if !dll_abs.exists() || !dll_abs.is_file() {
        return Err(anyhow!("DLL 文件不存在或不是合法文件：{}", dll_abs.display()));
    }

    // 构造 UTF-16 宽字符串（带终止符）
    let wide: Vec<u16> = OsStr::new(&dll_abs)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let byte_len = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        // 打开目标进程，获取必要权限
        let access = PROCESS_QUERY_INFORMATION
            | PROCESS_VM_WRITE
            | PROCESS_VM_OPERATION
            | PROCESS_CREATE_THREAD;
        let h_proc = OpenProcess(access, false, pid)
            .map_err(|e| anyhow!("OpenProcess 失败: {}", e.message()))?;
        let h_proc = RemoteHandle(h_proc);

        // 在目标进程中分配内存
        let remote_addr = VirtualAllocEx(
            h_proc.0,
            None,
            byte_len,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        // 如果分配失败，remote_addr 会是 null
        if remote_addr.is_null() {
            return Err(anyhow!(
        "VirtualAllocEx 失败，错误代码: {}",
        unsafe { GetLastError().0 }
            ));
        }
        let _mem_guard = RemoteMemory {
            process: &h_proc,
            address: remote_addr,
        };

        // 写入 DLL 路径到远程内存
        WriteProcessMemory(
            h_proc.0,
            remote_addr,
            wide.as_ptr() as _,
            byte_len,
            None,
        ).map_err(|e| anyhow!("WriteProcessMemory 失败: {}", e.message()))?;

        // 获取 LoadLibraryW 地址
        let h_kernel = GetModuleHandleA(PCSTR(b"kernel32.dll\0".as_ptr()))
            .map_err(|e| anyhow!("GetModuleHandleA 失败: {}", e.message()))?;
        let proc_addr = GetProcAddress(h_kernel, PCSTR(b"LoadLibraryW\0".as_ptr()))
            .ok_or_else(|| anyhow!("GetProcAddress 未找到 LoadLibraryW"))?;

        // 在目标进程中创建线程，调用 LoadLibraryW
        let h_thread = CreateRemoteThread(
            h_proc.0,
            None,
            0,
            Some(std::mem::transmute(proc_addr)),
            Some(remote_addr),
            0,
            None,
        )
            .map_err(|e| anyhow!("CreateRemoteThread 失败: {}", e.message()))?;
        let h_thread = RemoteHandle(h_thread);

        // 等待线程执行完毕
        WaitForSingleObject(h_thread.0, INFINITE);

        // 检查 LoadLibraryW 返回值
        let mut exit_code: u32 = 0;
        GetExitCodeThread(h_thread.0, &mut exit_code)
            .map_err(|e| anyhow!("GetExitCodeThread 失败: {}", e.message()))?;
        if exit_code == 0 {
            return Err(anyhow!("远程 LoadLibraryW 返回 NULL"));
        }
    }

    Ok(())
}

/// 遍历 `mods` 目录下所有 DLL，注入到指定进程
pub fn inject(base_folder: &Path, exe_name: Option<&str>, pid: Option<u32>) -> Result<()> {
    let mods_dir = base_folder.join("mods");
    if !mods_dir.exists() {
        fs::create_dir_all(&mods_dir)
            .with_context(|| format!("无法创建 mods 目录：{}", mods_dir.display()))?;
        debug!("已创建 mods 目录：{}", mods_dir.display());
    }

    // 确定目标进程 PID
    let target_pid = if let Some(name) = exe_name {
        find_pid(name)?
    } else {
        pid.ok_or_else(|| anyhow!("未提供 exe_name 或 pid"))?
    };

    let mut found = false;
    let mut success_count = 0;

    for entry in fs::read_dir(&mods_dir)? {
        let path = match entry {
            Ok(e) => e.path(),
            Err(e) => {
                debug!("读取目录项失败: {}", e);
                continue;
            }
        };

        if path
            .extension()
            .and_then(|e| e.to_str())
            .map_or(false, |ext| ext.eq_ignore_ascii_case("dll"))
        {
            found = true;
            debug!("向 PID={} 注入 DLL: {}", target_pid, path.display());
            match safe_inject(target_pid, &path) {
                Ok(()) => success_count += 1,
                Err(e) => {
                    debug!("注入 DLL {} 失败: {}", path.display(), e);
                    // 忽略失败，继续下一项
                }
            }
        }
    }

    if !found {
        debug!("{} 中未找到任何 DLL，跳过注入。", mods_dir.display());
    } else if success_count == 0 {
        debug!("所有 DLL 注入均失败。");
    } else {
        debug!("成功注入 {} 个 DLL。", success_count);
    }

    Ok(())
}