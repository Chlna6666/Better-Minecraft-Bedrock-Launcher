#![cfg_attr(windows, expect(unsafe_code, reason = "writing Unicode text to Win32 consoles"))]

//! Terminal output boundary for the BMCBL application and CLI.
//! Strings stay Unicode internally; console handles receive UTF-16 and
//! redirected stdout/stderr retain UTF-8 without changing the global code page.

use std::io::{self, Write as _};
#[cfg(not(windows))]
use std::io::IsTerminal as _;

#[cfg(windows)]
use windows::Win32::System::Console::{
    CONSOLE_MODE, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode,
    GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
};

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "WriteConsoleW"]
    fn write_console_w_raw(
        handle: *mut core::ffi::c_void,
        text: *const u16,
        count: u32,
        written: *mut u32,
        reserved: *mut core::ffi::c_void,
    ) -> i32;
}

#[cfg(windows)]
fn console_handle(stream: STD_HANDLE) -> Option<windows::Win32::Foundation::HANDLE> {
    let handle = unsafe { GetStdHandle(stream) }.ok()?;
    if handle.is_invalid() {
        return None;
    }
    let mut mode = CONSOLE_MODE(0);
    unsafe { GetConsoleMode(handle, &mut mode) }.ok()?;
    Some(handle)
}

#[cfg(windows)]
fn next_utf16_chunk(wide: &[u16], start: usize) -> usize {
    let mut end = (start + 8192).min(wide.len());
    if end < wide.len() && (0xD800..=0xDBFF).contains(&wide[end - 1]) {
        end -= 1;
    }
    end
}

#[cfg(windows)]
fn write_console_unicode(stream: STD_HANDLE, text: &str) -> io::Result<bool> {
    let Some(handle) = console_handle(stream) else {
        return Ok(false);
    };
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut offset = 0usize;
    while offset < wide.len() {
        let end = next_utf16_chunk(&wide, offset);
        let mut written = 0u32;
        let result = unsafe {
            write_console_w_raw(
                handle.0,
                wide[offset..end].as_ptr(),
                (end - offset) as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "console write returned zero"));
        }
        offset += written as usize;
    }
    Ok(true)
}

pub(crate) fn write_stdout(text: &str) -> io::Result<()> {
    #[cfg(windows)]
    if write_console_unicode(STD_OUTPUT_HANDLE, text)? {
        return Ok(());
    }
    std::io::stdout().lock().write_all(text.as_bytes())
}

pub(crate) fn write_stderr(text: &str) -> io::Result<()> {
    #[cfg(windows)]
    if write_console_unicode(STD_ERROR_HANDLE, text)? {
        return Ok(());
    }
    std::io::stderr().lock().write_all(text.as_bytes())
}

/// Colour is an optional console capability, not a property of a Windows
/// version or IsTerminal alone. Failed VT negotiation means plain text.
pub(crate) fn supports_ansi_stdout() -> bool {
    #[cfg(windows)]
    {
        let Some(handle) = console_handle(STD_OUTPUT_HANDLE) else {
            return false;
        };
        let mut mode = CONSOLE_MODE(0);
        if unsafe { GetConsoleMode(handle, &mut mode) }.is_err() {
            return false;
        }
        let desired = CONSOLE_MODE(
            mode.0 | ENABLE_PROCESSED_OUTPUT.0 | ENABLE_VIRTUAL_TERMINAL_PROCESSING.0,
        );
        if unsafe { SetConsoleMode(handle, desired) }.is_err() {
            return false;
        }
        let mut actual = CONSOLE_MODE(0);
        return unsafe { GetConsoleMode(handle, &mut actual) }.is_ok()
            && (actual.0 & ENABLE_VIRTUAL_TERMINAL_PROCESSING.0) != 0;
    }
    #[cfg(not(windows))]
    {
        std::io::stdout().is_terminal()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn utf16_chunk_does_not_split_surrogates() {
        let mut wide = vec![b'a' as u16; 8191];
        wide.push(0xD83D);
        wide.push(0xDE80);
        assert_eq!(next_utf16_chunk(&wide, 0), 8191);
        assert_eq!(next_utf16_chunk(&wide, 8191), wide.len());
    }

    #[test]
    fn non_ascii_text_encodes_as_utf16() {
        let text = "BMCBL 中文 日本語";
        let wide: Vec<u16> = text.encode_utf16().collect();
        assert_eq!(String::from_utf16(&wide).unwrap(), text);
    }
}
