//! Native Windows per-PID pipe. Created while the Minecraft process is still
//! suspended, before its BLoader bootstrap gets a chance to connect.
#![expect(unsafe_code, reason = "Windows named pipes need explicit native security and PID checks")]

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use super::{LaunchPlan, new_session_id, serve};

const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const PIPE_NOWAIT: u32 = 0x0000_0001;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x0000_0008;
const PIPE_CONNECTED: u32 = 535;
const PIPE_LISTENING: u32 = 536;
// BLoader may start after a slow PreLoader and game runtime initialization.
// This wait is entirely off the launcher and game main threads.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);
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
    fn create_named_pipe(
        name: *const u16, open_mode: u32, pipe_mode: u32, max_instances: u32,
        outbound_buffer_size: u32, inbound_buffer_size: u32, default_timeout: u32,
        attributes: *mut SecurityAttributes,
    ) -> *mut c_void;
    #[link_name = "ConnectNamedPipe"]
    fn connect_named_pipe(pipe: *mut c_void, overlapped: *mut c_void) -> i32;
    #[link_name = "GetNamedPipeClientProcessId"]
    fn get_named_pipe_client_process_id(pipe: *mut c_void, client_pid: *mut u32) -> i32;
    #[link_name = "SetNamedPipeHandleState"]
    fn set_pipe_mode(pipe: *mut c_void, mode: *mut u32, max_collection: *mut u32, timeout: *mut u32) -> i32;
    #[link_name = "GetLastError"]
    fn last_error() -> u32;
    #[link_name = "LocalFree"]
    fn local_free(pointer: *mut c_void) -> *mut c_void;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "ConvertStringSecurityDescriptorToSecurityDescriptorW"]
    fn convert_sddl(
        sddl: *const u16, revision: u32,
        descriptor: *mut *mut c_void, descriptor_len: *mut u32,
    ) -> i32;
}

fn wide(input: &str) -> Vec<u16> {
    input.encode_utf16().chain(Some(0)).collect()
}

/// Must run before ResumeThread in launch_win32. Game startup is not blocked
/// if the BLoader virtual-Mod channel fails: this feature is optional.
pub(crate) fn start(pid: u32, plan: LaunchPlan, game_version: String) -> Result<(), String> {
    if plan.modules.is_empty() { return Ok(()); }
    let root = plan.plugins_root.to_string_lossy().into_owned();
    if !plan.plugins_root.is_absolute() {
        return Err("plugin root is not an absolute path".into());
    }
    let name = wide(&format!(r"\\.\pipe\BMCBL.PluginMods.{pid}"));
    let mut descriptor = ptr::null_mut();
    let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;OW)");
    if unsafe { convert_sddl(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, ptr::null_mut()) } == 0 {
        return Err("cannot construct virtual Mod named-pipe DACL".into());
    }
    let mut attributes = SecurityAttributes {
        length: std::mem::size_of::<SecurityAttributes>() as u32,
        security_descriptor: descriptor,
        inherit_handle: 0,
    };
    let raw = unsafe {
        create_named_pipe(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1, 65_536, 65_536, 0, &mut attributes,
        )
    };
    unsafe { local_free(descriptor); }
    if raw.is_null() || raw as isize == -1 {
        return Err(format!("CreateNamedPipeW failed: {}", unsafe { last_error() }));
    }
    let file = unsafe { File::from_raw_handle(raw) };
    let session = new_session_id();
    thread::Builder::new()
        .name("bmcbl-virtual-mod-pipe".into())
        .spawn(move || {
            if let Err(error) = run_pipe(file, pid, plan, &root, &session, &game_version) {
                tracing::warn!(pid, session_id = %session, %error,
                    "BLoader virtual Mod handshake or session ended");
            }
        })
        .map_err(|error| format!("cannot start virtual Mod server: {error}"))?;
    Ok(())
}

fn run_pipe(
    mut pipe: File, pid: u32, plan: LaunchPlan, root: &str,
    session: &str, version: &str,
) -> Result<(), String> {
    let started = Instant::now();
    let raw = pipe.as_raw_handle();
    loop {
        if unsafe { connect_named_pipe(raw, ptr::null_mut()) } != 0 {
            break;
        }
        match unsafe { last_error() } {
            PIPE_CONNECTED => break,
            PIPE_LISTENING if started.elapsed() < CONNECT_TIMEOUT => {
                thread::sleep(Duration::from_millis(5));
            }
            code => return Err(format!("virtual Mod pipe connect failed or timed out: {code}")),
        }
    }
    let mut client_pid = 0_u32;
    if unsafe { get_named_pipe_client_process_id(raw, &mut client_pid) } == 0 || client_pid != pid {
        return Err("virtual Mod pipe connected from another Minecraft PID".into());
    }
    let mut blocking_byte_mode = 0_u32;
    if unsafe { set_pipe_mode(raw, &mut blocking_byte_mode, ptr::null_mut(), ptr::null_mut()) } == 0 {
        return Err("failed to enter blocking byte-mode pipe transport".into());
    }
    let writer = pipe.try_clone()
        .map_err(|error| format!("cannot clone virtual Mod pipe for RPC: {error}"))?;
    serve(&mut pipe, writer, &plan, root, "named-pipe", Some(pid), session, version)
}
