#![cfg(target_os = "windows")]

use serde::{Deserialize, Serialize};
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;
use windows::Win32::System::Console::GetConsoleWindow;

use crate::core::inject::inject::launch_win32_with_injection;

const TERMINAL_HOST_FLAG: &str = "--bmcbl-terminal-host";
const TERMINAL_HOST_PID_ENV: &str = "BMCBL_TERMINAL_HOST_PID";
const TERMINAL_START_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINAL_LAUNCH_TIMEOUT: Duration = Duration::from_secs(90);
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
struct TerminalLaunchRequest {
    exe_path: String,
    args: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TerminalHostResponse {
    state: String,
    pid: Option<u32>,
    gamertag: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalLaunchResult {
    pub(crate) pid: u32,
    pub(crate) gamertag: Option<String>,
}

fn host_paths() -> Result<(PathBuf, PathBuf), String> {
    let dir = std::env::temp_dir().join("BMCBL").join("terminal-host");
    fs::create_dir_all(&dir)
        .map_err(|error| format!("创建 Windows Terminal 启动交换目录失败: {error}"))?;
    let nonce = Uuid::new_v4().simple().to_string();
    Ok((
        dir.join(format!("request-{nonce}.json")),
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

fn cleanup_exchange_files(request: &Path, response: &Path) {
    let _ = fs::remove_file(request);
    let _ = fs::remove_file(response);
}

/// Starts BMCBL's lightweight terminal-host mode inside a brand-new Windows
/// Terminal window. No Xbox credential is passed through argv or the exchange
/// JSON: the host process prepares the account from BMCBL's normal secure store.
pub(crate) async fn launch_minecraft(
    exe_path: &str,
    args: Option<&str>,
) -> Result<Option<TerminalLaunchResult>, String> {
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("无法定位 BMCBL.exe: {error}"))?;
    let (request_path, response_path) = host_paths()?;
    let request = TerminalLaunchRequest {
        exe_path: exe_path.to_string(),
        args: args.map(ToOwned::to_owned),
    };
    let request_bytes = serde_json::to_vec(&request)
        .map_err(|error| format!("序列化 Windows Terminal 启动请求失败: {error}"))?;
    fs::write(&request_path, request_bytes)
        .map_err(|error| format!("写入 Windows Terminal 启动请求失败: {error}"))?;

    let spawn_result = Command::new("wt.exe")
        .arg("-w")
        .arg("-1")
        .arg("new-tab")
        .arg("--title")
        .arg("Minecraft BLoader Console")
        .arg("--suppressApplicationTitle")
        .arg(&current_exe)
        .arg(TERMINAL_HOST_FLAG)
        .arg(&request_path)
        .arg(&response_path)
        .spawn();

    if let Err(error) = spawn_result {
        cleanup_exchange_files(&request_path, &response_path);
        tracing::warn!(?error, "无法启动 Windows Terminal，将回退到系统控制台");
        return Ok(None);
    }

    let started = Instant::now();
    let mut host_ready = false;
    loop {
        if let Some(response) = read_response(&response_path) {
            match response.state.as_str() {
                "ready" => host_ready = true,
                "launched" => {
                    let pid = response
                        .pid
                        .ok_or_else(|| "Windows Terminal Host 未返回 Minecraft PID".to_string())?;
                    let result = TerminalLaunchResult {
                        pid,
                        gamertag: response.gamertag,
                    };
                    cleanup_exchange_files(&request_path, &response_path);
                    return Ok(Some(result));
                }
                "error" => {
                    let message = response
                        .error
                        .unwrap_or_else(|| "Windows Terminal Host 启动失败".to_string());
                    cleanup_exchange_files(&request_path, &response_path);
                    if host_ready {
                        return Err(message);
                    }
                    tracing::warn!(error = %message, "Windows Terminal Host 未进入启动阶段，将回退到系统控制台");
                    return Ok(None);
                }
                _ => {}
            }
        }

        let timeout = if host_ready {
            TERMINAL_LAUNCH_TIMEOUT
        } else {
            TERMINAL_START_TIMEOUT
        };
        if started.elapsed() >= timeout {
            cleanup_exchange_files(&request_path, &response_path);
            if host_ready {
                return Err("Windows Terminal 已建立，但 Minecraft 启动等待超时".to_string());
            }
            tracing::warn!("Windows Terminal Host 握手超时，将回退到系统控制台");
            return Ok(None);
        }
        sleep(TERMINAL_POLL_INTERVAL).await;
    }
}

/// Runs before the normal GPUI startup when BMCBL.exe is invoked by wt.exe.
/// The helper owns no UI and stays alive until Minecraft exits so the Terminal
/// tab remains open for BLoader/Mod output.
pub(crate) fn run_host_from_args() -> Result<bool, String> {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(flag) = args.next() else {
        return Ok(false);
    };
    if flag != TERMINAL_HOST_FLAG {
        return Ok(false);
    }

    let request_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "Windows Terminal Host 缺少 request 路径".to_string())?;
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
                pid: None,
                gamertag: None,
                error: Some("BMCBL Host 未附加到 Windows Terminal 控制台".to_string()),
            },
        );
        return Ok(true);
    }

    let host_pid = unsafe { get_current_process_id_raw() };
    // This process has not initialized GPUI or worker threads yet. The variable
    // contains only a process id; credentials remain in BMCBL's secure store.
    unsafe {
        std::env::set_var(TERMINAL_HOST_PID_ENV, host_pid.to_string());
    }

    write_response(
        &response_path,
        &TerminalHostResponse {
            state: "ready".to_string(),
            pid: None,
            gamertag: None,
            error: None,
        },
    )?;

    let request_bytes = match fs::read(&request_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let message = format!("读取 Windows Terminal 启动请求失败: {error}");
            let _ = write_response(
                &response_path,
                &TerminalHostResponse {
                    state: "error".to_string(),
                    pid: None,
                    gamertag: None,
                    error: Some(message),
                },
            );
            return Ok(true);
        }
    };
    let _ = fs::remove_file(&request_path);
    let request: TerminalLaunchRequest = match serde_json::from_slice(&request_bytes) {
        Ok(request) => request,
        Err(error) => {
            let message = format!("解析 Windows Terminal 启动请求失败: {error}");
            let _ = write_response(
                &response_path,
                &TerminalHostResponse {
                    state: "error".to_string(),
                    pid: None,
                    gamertag: None,
                    error: Some(message),
                },
            );
            return Ok(true);
        }
    };

    let runtime = crate::tasks::runtime::initialize_app_runtime()
        .map_err(|error| format!("初始化 Windows Terminal Host 运行时失败: {error}"))?;
    let launch_result = runtime.block_on(async {
        let auth = crate::core::bedrock_auth::prepare_launch_windows().await?;
        let gamertag = auth.as_ref().map(|auth| auth.gamertag.clone());
        let secure_launch_metadata = if let Some(auth) = &auth {
            let metadata = auth.take_secure_launch_metadata();
            if metadata.is_empty() {
                return Err("无法登记 BLoader XUser 一次性安全会话".to_string());
            }
            Some(metadata)
        } else {
            None
        };

        let callback: Arc<dyn Fn(String) + Send + Sync> = Arc::new(|message| {
            tracing::debug!(target: "windows-terminal-host", %message);
        });
        let pid = launch_win32_with_injection(
            &request.exe_path,
            request.args.as_deref(),
            Vec::new(),
            secure_launch_metadata,
            false,
            Some(callback),
        )
        .await
        .map_err(|error| format!("Windows Terminal Host 启动 Minecraft 失败: {error:?}"))?;
        Ok::<_, String>((pid, gamertag))
    });

    let (pid, gamertag) = match launch_result {
        Ok(result) => result,
        Err(error) => {
            let _ = write_response(
                &response_path,
                &TerminalHostResponse {
                    state: "error".to_string(),
                    pid: None,
                    gamertag: None,
                    error: Some(error),
                },
            );
            return Ok(true);
        }
    };

    write_response(
        &response_path,
        &TerminalHostResponse {
            state: "launched".to_string(),
            pid: Some(pid),
            gamertag,
            error: None,
        },
    )?;

    // Keep the Windows Terminal command process alive for the full Minecraft
    // lifetime. BLoader attaches to this process' console using the inherited
    // BMCBL_TERMINAL_HOST_PID marker before its PreLoader chain begins.
    unsafe {
        let process = open_process_raw(SYNCHRONIZE_ACCESS, 0, pid);
        if !process.is_null() {
            let _ = wait_for_single_object_raw(process, INFINITE_WAIT);
            let _ = close_handle_raw(process);
        }
    }
    let _ = fs::remove_file(&response_path);
    Ok(true)
}
