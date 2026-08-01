#![cfg(target_os = "windows")]
use anyhow::{Result, anyhow};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::ffi::{OsStr, c_void};
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::ptr;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_FLAGS, GetThreadContext, SetThreadContext, WriteProcessMemory,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CREATE_SUSPENDED, CreateProcessW, CreateRemoteThread, INFINITE,
    PROCESS_INFORMATION, ResumeThread, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};
use windows::core::{PCSTR, PWSTR};

pub type InjectProgressCb = Arc<dyn Fn(String) + Send + Sync>;

const INTERNAL_SESSION_KEY: &str = "BMCBL_XGAMERUNTIME_PREAUTH";
const PIPE_MAGIC: &[u8; 8] = b"BMCBLXU1";
const PIPE_VERSION: u32 = 1;
const PIPE_HEADER_SIZE: usize = 80;
const MAX_XUSER_PAYLOAD_SIZE: usize = 256 * 1024;
const PIPE_ACCESS_OUTBOUND: u32 = 0x0000_0002;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const PIPE_TYPE_BYTE: u32 = 0;
const PIPE_READMODE_BYTE: u32 = 0;
const PIPE_NOWAIT: u32 = 0x0000_0001;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
const ERROR_NO_DATA: u32 = 232;
const ERROR_PIPE_CONNECTED: u32 = 535;
const ERROR_PIPE_LISTENING: u32 = 536;
const SDDL_REVISION_1: u32 = 1;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    security_descriptor: *mut c_void,
    inherit_handle: i32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "CreateNamedPipeW"]
    fn create_named_pipe_w(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        output_buffer_size: u32,
        input_buffer_size: u32,
        default_timeout: u32,
        security_attributes: *mut SecurityAttributes,
    ) -> *mut c_void;
    #[link_name = "ConnectNamedPipe"]
    fn connect_named_pipe(pipe: *mut c_void, overlapped: *mut c_void) -> i32;
    #[link_name = "DisconnectNamedPipe"]
    fn disconnect_named_pipe(pipe: *mut c_void) -> i32;
    #[link_name = "GetNamedPipeClientProcessId"]
    fn get_named_pipe_client_process_id(pipe: *mut c_void, process_id: *mut u32) -> i32;
    #[link_name = "WriteFile"]
    fn write_file(
        file: *mut c_void,
        buffer: *const c_void,
        bytes_to_write: u32,
        bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    #[link_name = "FlushFileBuffers"]
    fn flush_file_buffers(file: *mut c_void) -> i32;
    #[link_name = "CloseHandle"]
    fn close_handle_raw(handle: *mut c_void) -> i32;
    #[link_name = "GetLastError"]
    fn get_last_error_raw() -> u32;
    #[link_name = "GetCurrentProcessId"]
    fn get_current_process_id_raw() -> u32;
    #[link_name = "LocalFree"]
    fn local_free(memory: *mut c_void) -> *mut c_void;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "ConvertStringSecurityDescriptorToSecurityDescriptorW"]
    fn convert_sddl(
        descriptor: *const u16,
        revision: u32,
        security_descriptor: *mut *mut c_void,
        descriptor_size: *mut u32,
    ) -> i32;
}

struct SensitivePayload(Vec<u8>);

impl SensitivePayload {
    fn into_inner(mut self) -> Vec<u8> {
        mem::take(&mut self.0)
    }
}

impl Drop for SensitivePayload {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

static XUSER_PAYLOADS: OnceLock<Mutex<HashMap<String, SensitivePayload>>> = OnceLock::new();
static XUSER_HANDLE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn register_xuser_launch_payload(payload: Vec<u8>) -> String {
    if payload.is_empty() || payload.len() > MAX_XUSER_PAYLOAD_SIZE {
        return String::new();
    }
    let counter = XUSER_HANDLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let launcher_pid = unsafe { get_current_process_id_raw() };
    let handle = hex::encode(Sha256::digest(
        format!("{launcher_pid}:{counter}:{now}").as_bytes(),
    ));
    let registry = XUSER_PAYLOADS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut registry) = registry.lock() {
        registry.insert(handle.clone(), SensitivePayload(payload));
    } else {
        return String::new();
    }

    // UWP launches never consume this registry entry. Expire every unused
    // handle so credentials cannot remain in the launcher process indefinitely.
    let cleanup_handle = handle.clone();
    let _ = thread::Builder::new()
        .name("bmcbl-xuser-expiry".to_string())
        .spawn(move || {
            thread::sleep(Duration::from_secs(90));
            if let Some(registry) = XUSER_PAYLOADS.get()
                && let Ok(mut registry) = registry.lock()
            {
                registry.remove(&cleanup_handle);
            }
        });
    handle
}

fn take_registered_xuser_payload(handle: &str) -> Option<Vec<u8>> {
    if handle.is_empty() {
        return None;
    }
    XUSER_PAYLOADS
        .get()?
        .lock()
        .ok()?
        .remove(handle)
        .map(SensitivePayload::into_inner)
}

pub fn grant_all_application_packages_access(path: &Path) -> anyhow::Result<()> {
    let output = Command::new("icacls")
        .arg(path)
        .arg("/grant")
        .arg("*S-1-15-2-1:(OI)(CI)M")
        .arg("/grant")
        .arg("*S-1-5-32-545:(OI)(CI)F")
        .arg("/T")
        .arg("/Q")
        .output()
        .map_err(|error| anyhow::Error::msg(format!("Failed to execute icacls: {error}")))?;

    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        eprintln!("Warning: icacls warning for {:?}: {}", path, error);
    }
    Ok(())
}

pub async fn launch_win32_with_injection(
    exe_path: &str,
    args: Option<&str>,
    dll_paths: Vec<String>,
    launch_metadata: Option<Vec<(String, String)>>,
    enable_console: bool,
    on_progress: Option<InjectProgressCb>,
) -> Result<u32> {
    let exe_path_owned = exe_path.to_string();
    let args_owned = args.map(ToOwned::to_owned);
    let callback = on_progress.clone();
    let xuser_payload = launch_metadata.and_then(|metadata| {
        metadata
            .into_iter()
            .find(|(key, _)| key == INTERNAL_SESSION_KEY)
            .and_then(|(_, handle)| take_registered_xuser_payload(&handle))
    });

    crate::tasks::runtime::run_io_blocking(move || -> Result<u32> {
        unsafe {
            let log = |message: &str| {
                if let Some(callback) = &callback {
                    callback(message.to_string());
                }
            };

            if xuser_payload
                .as_ref()
                .is_some_and(|payload| payload.is_empty() || payload.len() > MAX_XUSER_PAYLOAD_SIZE)
            {
                return Err(anyhow!("Win32 XUser 预认证载荷无效"));
            }

            let mut startup_info = STARTUPINFOW::default();
            startup_info.cb = mem::size_of::<STARTUPINFOW>() as u32;
            let mut process_info = PROCESS_INFORMATION::default();
            let mut creation_flags = CREATE_SUSPENDED;
            if enable_console {
                creation_flags |= CREATE_NEW_CONSOLE;
                log("启动标志: CREATE_NEW_CONSOLE (请求独立终端窗口)");
            }

            let mut command_line = format!("\"{}\"", exe_path_owned);
            if let Some(arguments) = &args_owned {
                command_line.push(' ');
                command_line.push_str(arguments);
            }
            let wide_command: Vec<u16> = OsStr::new(&command_line)
                .encode_wide()
                .chain(Some(0))
                .collect();

            CreateProcessW(
                None,
                Some(PWSTR(wide_command.as_ptr() as *mut _)),
                None,
                None,
                false,
                creation_flags,
                None,
                None,
                &startup_info,
                &mut process_info,
            )
            .map_err(|error| anyhow!("CreateProcessW failed: {error:?}"))?;

            let process = process_info.hProcess;
            let main_thread = process_info.hThread;
            let pid = process_info.dwProcessId;
            log(&format!("进程已挂起启动 PID: {pid}"));

            let pending_xuser = if let Some(payload) = xuser_payload {
                match PendingXUserPipe::create(pid, payload, callback.clone()) {
                    Ok(pipe) => {
                        log("已创建仅限目标进程的一次性 XUser 会话通道");
                        Some(pipe)
                    }
                    Err(error) => {
                        let _ = TerminateProcess(process, 1);
                        let _ = CloseHandle(process);
                        let _ = CloseHandle(main_thread);
                        return Err(error);
                    }
                }
            } else {
                log("未提供 XUser 会话；不会创建登录通道或触发 QueryApiImpl Hook");
                None
            };

            if !dll_paths.is_empty() {
                let kernel = GetModuleHandleW(windows::core::w!("kernel32.dll"))?;
                let load_library = GetProcAddress(kernel, PCSTR(b"LoadLibraryW\0".as_ptr()))
                    .ok_or_else(|| anyhow!("LoadLibraryW not found"))?
                    as u64;

                let mut path_addresses = Vec::new();
                for path in &dll_paths {
                    let wide_path: Vec<u16> =
                        OsStr::new(path).encode_wide().chain(Some(0)).collect();
                    let length = wide_path.len() * 2;
                    let remote = VirtualAllocEx(
                        process,
                        None,
                        length,
                        MEM_COMMIT | MEM_RESERVE,
                        PAGE_EXECUTE_READWRITE,
                    );
                    if !remote.is_null() {
                        WriteProcessMemory(
                            process,
                            remote,
                            wide_path.as_ptr().cast(),
                            length,
                            None,
                        )?;
                        path_addresses.push(remote as u64);
                        log(&format!("注入准备: {path}"));
                    }
                }

                let mut context: CONTEXT = mem::zeroed();
                context.ContextFlags = CONTEXT_FLAGS(0x100001);
                GetThreadContext(main_thread, &mut context)?;

                let mut shellcode = Vec::new();
                shellcode.extend_from_slice(&[
                    0x48, 0x83, 0xEC, 0x28, 0x50, 0x53, 0x51, 0x52, 0x41, 0x50, 0x41, 0x51, 0x41,
                    0x52, 0x41, 0x53,
                ]);
                for address in path_addresses {
                    shellcode.extend_from_slice(&[0x48, 0xB9]);
                    shellcode.extend_from_slice(&address.to_le_bytes());
                    shellcode.extend_from_slice(&[0x48, 0xB8]);
                    shellcode.extend_from_slice(&load_library.to_le_bytes());
                    shellcode.extend_from_slice(&[0xFF, 0xD0]);
                }
                shellcode.extend_from_slice(&[
                    0x41, 0x5B, 0x41, 0x5A, 0x41, 0x59, 0x41, 0x58, 0x5A, 0x59, 0x5B, 0x58, 0x48,
                    0x83, 0xC4, 0x28,
                ]);
                shellcode.extend_from_slice(&[0x48, 0xB8]);
                shellcode.extend_from_slice(&context.Rip.to_le_bytes());
                shellcode.extend_from_slice(&[0xFF, 0xE0]);

                let shellcode_memory = VirtualAllocEx(
                    process,
                    None,
                    shellcode.len(),
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_EXECUTE_READWRITE,
                );
                if shellcode_memory.is_null() {
                    let _ = TerminateProcess(process, 1);
                    let _ = CloseHandle(process);
                    let _ = CloseHandle(main_thread);
                    return Err(anyhow!("VirtualAllocEx failed for startup shellcode"));
                }
                WriteProcessMemory(
                    process,
                    shellcode_memory,
                    shellcode.as_ptr().cast(),
                    shellcode.len(),
                    None,
                )?;
                context.Rip = shellcode_memory as u64;
                SetThreadContext(main_thread, &context)?;
            }

            ResumeThread(main_thread);

            if let Some(pending_xuser) = pending_xuser
                && let Err(error) = pending_xuser.serve()
            {
                let _ = TerminateProcess(process, 1);
                let _ = CloseHandle(process);
                let _ = CloseHandle(main_thread);
                return Err(error);
            }

            let _ = CloseHandle(process);
            let _ = CloseHandle(main_thread);
            Ok(pid)
        }
    })
    .await
    .map_err(anyhow::Error::msg)?
}

struct PendingXUserPipe {
    pipe: *mut c_void,
    target_pid: u32,
    payload: SensitivePayload,
    callback: Option<InjectProgressCb>,
}

impl PendingXUserPipe {
    fn create(
        target_pid: u32,
        payload: Vec<u8>,
        callback: Option<InjectProgressCb>,
    ) -> Result<Self> {
        let pipe_name = wide(&format!(r"\\.\pipe\BMCBL.XUser.{target_pid}"));
        let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;OW)");
        let mut descriptor = ptr::null_mut();
        let converted = unsafe {
            convert_sddl(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if converted == 0 || descriptor.is_null() {
            return Err(anyhow!("创建 XUser 管道安全描述符失败"));
        }

        let mut security_attributes = SecurityAttributes {
            length: mem::size_of::<SecurityAttributes>() as u32,
            security_descriptor: descriptor,
            inherit_handle: 0,
        };
        let pipe = unsafe {
            create_named_pipe_w(
                pipe_name.as_ptr(),
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                (PIPE_HEADER_SIZE + payload.len()) as u32,
                0,
                0,
                &mut security_attributes,
            )
        };
        unsafe {
            local_free(descriptor);
        }
        if pipe.is_null() || pipe as isize == -1 {
            return Err(anyhow!("创建 XUser 一次性命名管道失败"));
        }

        Ok(Self {
            pipe,
            target_pid,
            payload: SensitivePayload(payload),
            callback,
        })
    }

    fn serve(self) -> Result<()> {
        let log = |message: &str| {
            if let Some(callback) = &self.callback {
                callback(message.to_string());
            }
        };
        let started = Instant::now();
        let connected = loop {
            if unsafe { connect_named_pipe(self.pipe, ptr::null_mut()) } != 0 {
                break true;
            }
            let error = unsafe { get_last_error_raw() };
            if error == ERROR_PIPE_CONNECTED {
                break true;
            }
            if !matches!(error, ERROR_PIPE_LISTENING | ERROR_NO_DATA)
                || started.elapsed() >= Duration::from_secs(15)
            {
                break false;
            }
            thread::sleep(Duration::from_millis(5));
        };

        if !connected {
            unsafe {
                close_handle_raw(self.pipe);
            }
            return Err(anyhow!("BLoader 未在超时时间内连接 XUser 会话通道"));
        }

        let mut client_pid = 0u32;
        if unsafe { get_named_pipe_client_process_id(self.pipe, &mut client_pid) } == 0
            || client_pid != self.target_pid
        {
            unsafe {
                disconnect_named_pipe(self.pipe);
                close_handle_raw(self.pipe);
            }
            return Err(anyhow!("XUser 会话通道连接者不是目标 Minecraft 进程"));
        }

        let issued_at = now_epoch();
        let expires_at = issued_at.saturating_add(60);
        let launcher_pid = unsafe { get_current_process_id_raw() };
        let digest: [u8; 32] = Sha256::digest(&self.payload.0).into();
        let mut header = [0u8; PIPE_HEADER_SIZE];
        header[0..8].copy_from_slice(PIPE_MAGIC);
        header[8..12].copy_from_slice(&PIPE_VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&self.target_pid.to_le_bytes());
        header[16..20].copy_from_slice(&launcher_pid.to_le_bytes());
        header[24..32].copy_from_slice(&issued_at.to_le_bytes());
        header[32..40].copy_from_slice(&expires_at.to_le_bytes());
        header[40..44].copy_from_slice(&(self.payload.0.len() as u32).to_le_bytes());
        header[48..80].copy_from_slice(&digest);

        write_exact(self.pipe, &header)?;
        write_exact(self.pipe, &self.payload.0)?;
        unsafe {
            flush_file_buffers(self.pipe);
            disconnect_named_pipe(self.pipe);
            close_handle_raw(self.pipe);
        }
        log(
            "XUser 会话载荷已传输至目标进程；等待 BLoader 验证会话、加载系统 Runtime 并报告 QueryApiImpl Hook 状态",
        );
        Ok(())
    }
}

impl Drop for PendingXUserPipe {
    fn drop(&mut self) {
        if !self.pipe.is_null() && self.pipe as isize != -1 {
            unsafe {
                close_handle_raw(self.pipe);
            }
            self.pipe = ptr::null_mut();
        }
    }
}

fn write_exact(handle: *mut c_void, bytes: &[u8]) -> Result<()> {
    let mut offset = 0usize;
    while offset < bytes.len() {
        let mut written = 0u32;
        let chunk = (bytes.len() - offset).min(u32::MAX as usize) as u32;
        let ok = unsafe {
            write_file(
                handle,
                bytes[offset..].as_ptr().cast(),
                chunk,
                &mut written,
                ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 {
            return Err(anyhow!("命名管道提前关闭"));
        }
        offset += written as usize;
    }
    Ok(())
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
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
        unsafe {
            let log = |message: &str| {
                if let Some(callback) = &callback {
                    callback(message.to_string());
                }
            };

            if !skip_acl {
                let path = Path::new(&dll_path);
                let _ = grant_all_application_packages_access(path);
            }

            let process = windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_ALL_ACCESS,
                false,
                pid,
            )
            .map_err(|error| anyhow!("OpenProcess failed: {error:?}"))?;

            if enable_console {
                let kernel = GetModuleHandleW(windows::core::w!("kernel32.dll"))?;

                if let Some(free_console) = GetProcAddress(kernel, PCSTR(b"FreeConsole\0".as_ptr()))
                {
                    if let Ok(remote_thread) = CreateRemoteThread(
                        process,
                        None,
                        0,
                        Some(mem::transmute(free_console)),
                        None,
                        0,
                        None,
                    ) {
                        WaitForSingleObject(remote_thread, 1000);
                        let _ = CloseHandle(remote_thread);
                    }
                }

                if let Some(alloc_console) =
                    GetProcAddress(kernel, PCSTR(b"AllocConsole\0".as_ptr()))
                {
                    if let Ok(remote_thread) = CreateRemoteThread(
                        process,
                        None,
                        0,
                        Some(mem::transmute(alloc_console)),
                        None,
                        0,
                        None,
                    ) {
                        WaitForSingleObject(remote_thread, INFINITE);
                        let _ = CloseHandle(remote_thread);
                    }
                }
            }

            let wide_path: Vec<u16> = OsStr::new(&dll_path).encode_wide().chain(Some(0)).collect();
            let length = wide_path.len() * 2;
            let remote_memory = VirtualAllocEx(
                process,
                None,
                length,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            );
            if remote_memory.is_null() {
                let _ = CloseHandle(process);
                return Err(anyhow!("VirtualAllocEx failed"));
            }
            WriteProcessMemory(
                process,
                remote_memory,
                wide_path.as_ptr().cast(),
                length,
                None,
            )?;

            let kernel = GetModuleHandleW(windows::core::w!("kernel32.dll"))?;
            let load_library = GetProcAddress(kernel, PCSTR(b"LoadLibraryW\0".as_ptr()))
                .ok_or_else(|| anyhow!("LoadLibraryW not found"))?;
            let remote_thread = CreateRemoteThread(
                process,
                None,
                0,
                Some(mem::transmute(load_library)),
                Some(remote_memory),
                0,
                None,
            )
            .map_err(|error| anyhow!("CreateRemoteThread failed: {error:?}"))?;

            WaitForSingleObject(remote_thread, INFINITE);
            let _ = VirtualFreeEx(process, remote_memory, 0, MEM_RELEASE);
            let _ = CloseHandle(remote_thread);
            let _ = CloseHandle(process);
            log(&format!("注入完成: {dll_path}"));
            Ok(())
        }
    })
    .await
    .map_err(anyhow::Error::msg)?
}
