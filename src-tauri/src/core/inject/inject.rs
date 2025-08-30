use anyhow::{anyhow, Result};
use std::{ffi::OsStr, mem, os::windows::ffi::OsStrExt, path::Path, time::Duration};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;
use tokio::task;
use windows::core::{PCSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::Authorization::{ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP};
use windows::Win32::Security::{ACE_FLAGS, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};
use windows::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::Memory::{VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE};
use windows::Win32::System::Threading::{CreateRemoteThread, GetExitCodeThread, OpenProcess, WaitForSingleObject, INFINITE, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_WRITE};

struct RemoteHandle(HANDLE);
impl Drop for RemoteHandle {
    fn drop(&mut self) {
        unsafe { let _ = CloseHandle(self.0); }
    }
}

struct RemoteMemory<'a> {
    process: &'a RemoteHandle,
    address: *mut std::ffi::c_void,
}
impl<'a> Drop for RemoteMemory<'a> {
    fn drop(&mut self) {
        unsafe { let _ = VirtualFreeEx(self.process.0, self.address, 0, MEM_RELEASE); }
    }
}

pub fn find_pid(exe_name: &str) -> Result<u32> {
    unsafe {
        let raw_snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        if raw_snap.is_invalid() { return Err(anyhow!("CreateToolhelp32Snapshot 返回 INVALID_HANDLE_VALUE")); }
        let snapshot = RemoteHandle(raw_snap);
        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        Process32FirstW(snapshot.0, &mut entry)?;
        loop {
            let name = String::from_utf16_lossy(&entry.szExeFile).trim_end_matches('\0').to_string();
            if name.eq_ignore_ascii_case(exe_name) { return Ok(entry.th32ProcessID); }
            if Process32NextW(snapshot.0, &mut entry).is_err() { break; }
        }
    }
    Err(anyhow!("未找到名为 `{}` 的进程", exe_name))
}

fn add_acl_for_sid(path: &Path, sid_str: &str) -> Result<()> {
    unsafe {
        let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let sid_utf16: Vec<u16> = sid_str.encode_utf16().chain(Some(0)).collect();
        let mut sid_ptr: PSID = PSID(std::ptr::null_mut());
        ConvertStringSidToSidW(PWSTR(sid_utf16.as_ptr() as *mut _), &mut sid_ptr)?;

        let mut ea = EXPLICIT_ACCESS_W::default();
        ea.grfAccessPermissions = (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).0;
        ea.grfAccessMode = SET_ACCESS;
        ea.grfInheritance = ACE_FLAGS(0);
        ea.Trustee.TrusteeForm = TRUSTEE_IS_SID;
        ea.Trustee.TrusteeType = TRUSTEE_IS_WELL_KNOWN_GROUP;
        ea.Trustee.ptstrName = PWSTR(sid_ptr.0 as *mut u16);

        let mut p_old_dacl: *mut windows::Win32::Security::ACL = std::ptr::null_mut();
        let mut p_sec_desc: PSECURITY_DESCRIPTOR = PSECURITY_DESCRIPTOR(std::ptr::null_mut());
        let _ = GetNamedSecurityInfoW(PWSTR(wide_path.as_ptr() as *mut _), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, Some(&mut p_old_dacl), None, &mut p_sec_desc);

        let mut p_new_dacl: *mut windows::Win32::Security::ACL = std::ptr::null_mut();
        let _ = SetEntriesInAclW(Some(&[ea]), Some(p_old_dacl), &mut p_new_dacl);
        let _ = SetNamedSecurityInfoW(PWSTR(wide_path.as_ptr() as *mut _), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, Some(p_new_dacl), None);
    }
    Ok(())
}

pub async fn fast_inject(pid: u32, _dll_path: PathBuf, wide: Vec<u16>) -> Result<()> {
    // 所有阻塞 WinAPI 操作放到 spawn_blocking
    tokio::task::spawn_blocking(move || -> Result<()> {
        unsafe {
            // 打开目标进程
            let h_proc = OpenProcess(
                PROCESS_QUERY_INFORMATION | PROCESS_VM_WRITE | PROCESS_VM_OPERATION | PROCESS_CREATE_THREAD,
                false,
                pid,
            ).map_err(|e| anyhow!("OpenProcess failed: {:?}", e))?;

            // 计算写入字节数（u16 * 2）
            let size_in_bytes = wide.len() * std::mem::size_of::<u16>();

            // 分配远程内存
            let remote_addr = VirtualAllocEx(h_proc, None, size_in_bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
            if remote_addr.is_null() {
                return Err(anyhow!("VirtualAllocEx failed"));
            }

            // 写入远程内存（注意：WriteProcessMemory 在你的环境中返回 Result，因此用 map_err ? 来处理）
            WriteProcessMemory(
                h_proc,
                remote_addr,
                wide.as_ptr() as _,
                size_in_bytes,
                None,
            ).map_err(|e| anyhow!("WriteProcessMemory failed: {:?}", e))?;

            // 获取 LoadLibraryW 地址
            let h_kernel = GetModuleHandleA(PCSTR(b"kernel32.dll\0".as_ptr()))
                .map_err(|e| anyhow!("GetModuleHandleA failed: {:?}", e))?;
            let proc_addr = GetProcAddress(h_kernel, PCSTR(b"LoadLibraryW\0".as_ptr()))
                .ok_or_else(|| anyhow!("LoadLibraryW not found"))?;

            // 创建远程线程
            let h_thread = CreateRemoteThread(
                h_proc,
                None,
                0,
                Some(std::mem::transmute(proc_addr)),
                Some(remote_addr),
                0,
                None,
            ).map_err(|e| anyhow!("CreateRemoteThread failed: {:?}", e))?;

            // 等待线程结束
            WaitForSingleObject(h_thread, INFINITE);

            // 获取线程返回值（LoadLibrary 返回模块句柄，0 表示失败）
            let mut exit_code: u32 = 0;
            GetExitCodeThread(h_thread, &mut exit_code)
                .map_err(|e| anyhow!("GetExitCodeThread failed: {:?}", e))?;
            if exit_code == 0 {
                // 清理远程内存（可选）
                let _ = VirtualFreeEx(h_proc, remote_addr, 0, MEM_RELEASE);
                return Err(anyhow!("DLL injection LoadLibraryW returned 0"));
            }

            // 可选清理：释放远程内存、关闭句柄等（视需要）
            // let _ = VirtualFreeEx(h_proc, remote_addr, 0, MEM_RELEASE);
            // CloseHandle(h_thread);
            // CloseHandle(h_proc);
        }

        Ok(())
    }).await??;

    Ok(())
}
