#![cfg(target_os = "windows")]
#![expect(
    unsafe_code,
    reason = "CLI output attaches the GUI process to the parent console and installs console standard handles"
)]

use std::fs::OpenOptions;
use std::os::windows::io::AsRawHandle;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{FILE_TYPE_UNKNOWN, GetFileType};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
    STD_OUTPUT_HANDLE, SetStdHandle,
};

/// Makes release builds (windows_subsystem = "windows") able to write terminal
/// CLI output to the console that started them. Redirected stdout/stderr remain
/// untouched so redirected JSON output and pipes keep working.
pub(crate) fn attach_parent_console() {
    // SAFETY: AttachConsole only changes this process's console association; no
    // pointer or handle owned by Rust is passed into the call.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) }.is_err() {
        return;
    }

    let output_missing = current_handle(STD_OUTPUT_HANDLE).is_none_or(|handle| !is_usable(handle));
    let error_missing = current_handle(STD_ERROR_HANDLE).is_none_or(|handle| !is_usable(handle));
    if !output_missing && !error_missing {
        return;
    }

    let Ok(output) = OpenOptions::new().read(true).write(true).open("CONOUT$") else {
        return;
    };
    let console_handle = HANDLE(output.as_raw_handle());
    // The handle is used by std::io until process exit; leaking the File keeps
    // it open without maintaining a process-global registry.
    std::mem::forget(output);

    if output_missing {
        // SAFETY: console_handle stays valid for the process lifetime.
        let _ = unsafe { SetStdHandle(STD_OUTPUT_HANDLE, console_handle) };
    }
    if error_missing {
        // SAFETY: same handle; SetStdHandle only stores it in the process table.
        let _ = unsafe { SetStdHandle(STD_ERROR_HANDLE, console_handle) };
    }
}

fn current_handle(std_handle: STD_HANDLE) -> Option<HANDLE> {
    // SAFETY: GetStdHandle only reads the process standard handle table.
    unsafe { GetStdHandle(std_handle) }.ok()
}

fn is_usable(handle: HANDLE) -> bool {
    // SAFETY: handle came from GetStdHandle; GetFileType only queries it.
    (unsafe { GetFileType(handle) }) != FILE_TYPE_UNKNOWN
}
