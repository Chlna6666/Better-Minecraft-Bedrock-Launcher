#![cfg(target_os = "windows")]

use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;
use windows::Win32::System::Console::GetConsoleWindow;

const TERMINAL_HOST_FLAG: &str = "--bmcbl-terminal-host";
const TERMINAL_START_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINAL_BIND_TIMEOUT: Duration = Duration::from_secs(90);
const TERMINAL_POLL_INTERVAL: Duration = Duration::from_millis(50);
const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
const INFINITE_WAIT: u32 = u32::MAX;

#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "AttachConsole"]
    fn attach_console_raw(process_id: u32) -> i32;
    #[link_name = "GetCurrentProcessId"]
    fn get_current_process_id_raw() -> u32;
    #[link_name = "OpenProcess"]
    fn open_process_raw(
        desired_access: u32,
        inherit_handle: i32,
        process_id: u32,
    ) -> *mut c_void;
    #[link_name = "WaitForSingleObject"]
    fn wait_for_single_object_raw(handle: *mut c_void, milliseconds: u32) -> u32;
    #[link_name = "CloseHandle"]
    fn close_handle_raw(handle: *mut c_void) -> i32;
}

#[derive(Debug, Serialize, Deserialize)]
struct TerminalHostResponse {
    state: String,
    host_pid: Option<u32>,
    error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TerminalHostCommand {
    minecraft_pid: Option<u32>,
    cancel: bool,
}

#[derive(Debug)]
pub(crate) struct TerminalHostHandle {
    pub(crate) host_pid: u32,
    command_path: PathBuf,
    armed: bool,
}

impl TerminalHostHandle {
    /// Hands the Minecraft PID to the terminal helper after the original BMCBL
    /// process has completed its secure suspended launch. The helper then stays
    /// alive until Minecraft exits, keeping the Windows Terminal tab alive.
    pub(crate) fn bind_minecraft(mut self, minecraft_pid: u32) -> Result<(), String> {
        if minecraft_pid == 0 {
            return Err("Windows Terminal Host 收到无效 Minecraft PID".to_string());
        }
        let command = TerminalHostCommand {
            minecraft_pid: Some(minecraft_pid),
            cancel: false,
        };
        let bytes = serde_json::to_vec(&command)
            .map_err(|error| format!("序列化 Windows Terminal Host 绑定请求失败: {error}"))?;
        fs::write(&self.command_path, bytes)
            .map_err(|error| format!("写入 Windows Terminal Host 绑定请求失败: {error}"))?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for TerminalHostHandle {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let command = TerminalHostCommand {
            minecraft_pid: None,
            cancel: true,
        };
        if let Ok(bytes) = serde_json::to_vec(&command) {
            let _ = fs::write(&self.command_path, bytes);
        }
    }
}

fn host_paths() -> Result<(PathBuf, PathBuf), String> {
    let dir = std::env::temp_dir().join("BMCBL").join("terminal-host");
    fs::create_dir_all(&dir)
        .map_err(|error| format!("创建 Windows Terminal 启动交换目录失败: {error}"))?;
    let nonce = Uuid::new_v4().simple().to_string();
    Ok((
        dir.join(format!("command-{nonce}.json")),
        dir.join(format!("response-{nonce}.json")),
    ))
}

fn write_response(path: &Path, response: &TerminalHostResponse) -> Result<(), String> {
    let bytes = serde_json::to_vec(response)
        .map_err(|error| format!("序列化 Windows Terminal Host 响应失败: {error}"))?;
    fs::write(path, bytes)
        .map_err(|error| format!("写入 Windows Terminal Host 响应失败: {error}"))
}

fn read_response(path: &Path) -> Option<TerminalHostResponse> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn cleanup_exchange_files(command: &Path, response: &Path) {
    let _ = fs::remove_file(command);
    let _ = fs::remove_file(response);
}

fn cmd_quote(path: &Path) -> String {
    let text = path.to_string_lossy().replace('%', "%%");
    format!("\"{text}\"")
}

/// Creates only a Windows Terminal console host. Minecraft and Xbox auth remain
/// owned by the original BMCBL GUI process so the process-local XUser payload
/// registry and the one-shot authenticated pipe keep exactly their old trust
/// boundary.
pub(crate) async fn start_host() -> Result<Option<TerminalHostHandle>, String> {
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("无法定位 BMCBL.exe: {error}"))?;
    let (command_path, response_path) = host_paths()?;

    // BMCBL.exe is a WINDOWS-subsystem binary in release builds. Keep cmd.exe as
    // the real console process in the WT tab; the lightweight BMCBL helper then
    // AttachConsole(ATTACH_PARENT_PROCESS) to that same ConPTY session.
    let host_command = format!(
        "start \"\" /wait /b {} {} {} {}",
        cmd_quote(&current_exe),
        TERMINAL_HOST_FLAG,
        cmd_quote(&command_path),
        cmd_quote(&response_path),
    );
    let spawn_result = Command::new("wt.exe")
        .arg("-w")
        .arg("-1")
        .arg("new-tab")
        .arg("--title")
        .arg("Minecraft BLoader Console")
        .arg("--suppressApplicationTitle")
        .arg("cmd.exe")
        .arg("/d")
        .arg("/s")
        .arg("/c")
        .arg(host_command)
        .spawn();

    if let Err(error) = spawn_result {
        cleanup_exchange_files(&command_path, &response_path);
        tracing::warn!(?error, "无法启动 Windows Terminal，将回退到系统控制台");
        return Ok(None);
    }

    let started = Instant::now();
    loop {
        if let Some(response) = read_response(&response_path) {
            match response.state.as_str() {
                "ready" => {
                    let host_pid = response
                        .host_pid
                        .filter(|pid| *pid != 0)
                        .ok_or_else(|| "Windows Terminal Host 未返回有效 PID".to_string())?;
                    let _ = fs::remove_file(&response_path);
                    return Ok(Some(TerminalHostHandle {
                        host_pid,
                        command_path,
                        armed: true,
                    }));
                }
                "error" => {
                    let message = response
                        .error
                        .unwrap_or_else(|| "Windows Terminal Host 初始化失败".to_string());
                    cleanup_exchange_files(&command_path, &response_path);
                    tracing::warn!(error = %message, "Windows Terminal Host 初始化失败，将回退到系统控制台");
                    return Ok(None);
                }
                _ => {}
            }
        }

        if started.elapsed() >= TERMINAL_START_TIMEOUT {
            let cancel = TerminalHostCommand {
                minecraft_pid: None,
                cancel: true,
            };
            if let Ok(bytes) = serde_json::to_vec(&cancel) {
                let _ = fs::write(&command_path, bytes);
            }
            let _ = fs::remove_file(&response_path);
            tracing::warn!("Windows Terminal Host 握手超时，将回退到系统控制台");
            return Ok(None);
        }
        sleep(TERMINAL_POLL_INTERVAL).await;
    }
}

/// Runs before normal GPUI startup. This helper owns only the terminal session;
/// it never initializes the BMCBL runtime, reads Xbox credentials, registers an
/// XUser payload, or creates Minecraft.
pub(crate) fn run_host_from_args() -> Result<bool, String> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(flag) = args.next() else {
        return Ok(false);
    };
    if flag.to_string_lossy() != TERMINAL_HOST_FLAG {
        return Ok(false);
    }

    let command_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "Windows Terminal Host 缺少 command 路径".to_string())?;
    let response_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "Windows Terminal Host 缺少 response 路径".to_string())?;

    if unsafe { GetConsoleWindow().0.is_null() } {
        unsafe {
            let _ = attach_console_raw(ATTACH_PARENT_PROCESS);
        }
    }
    if unsafe { GetConsoleWindow().0.is_null() } {
        let _ = write_response(
            &response_path,
            &TerminalHostResponse {
                state: "error".to_string(),
                host_pid: None,
                error: Some("BMCBL Host 未附加到 Windows Terminal 控制台".to_string()),
            },
        );
        return Ok(true);
    }

    let host_pid = unsafe { get_current_process_id_raw() };
    write_response(
        &response_path,
        &TerminalHostResponse {
            state: "ready".to_string(),
            host_pid: Some(host_pid),
            error: None,
        },
    )?;

    // Wait only for the original BMCBL process to bind a Minecraft PID. No
    // authentication material crosses this helper boundary.
    let started = Instant::now();
    let minecraft_pid = loop {
        if let Ok(bytes) = fs::read(&command_path)
            && let Ok(command) = serde_json::from_slice::<TerminalHostCommand>(&bytes)
        {
            let _ = fs::remove_file(&command_path);
            if command.cancel {
                let _ = fs::remove_file(&response_path);
                return Ok(true);
            }
            if let Some(pid) = command.minecraft_pid.filter(|pid| *pid != 0) {
                break pid;
            }
        }
        if started.elapsed() >= TERMINAL_BIND_TIMEOUT {
            let _ = fs::remove_file(&response_path);
            return Ok(true);
        }
        std::thread::sleep(TERMINAL_POLL_INTERVAL);
    };

    unsafe {
        let process = open_process_raw(SYNCHRONIZE_ACCESS, 0, minecraft_pid);
        if !process.is_null() {
            let _ = wait_for_single_object_raw(process, INFINITE_WAIT);
            let _ = close_handle_raw(process);
        }
    }
    let _ = fs::remove_file(&response_path);
    Ok(true)
}
