use std::ffi::OsStr;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;

pub fn to_wstr(s: &str) -> Vec<u16> {
    // 1) 统一换行到 Windows 风格，避免 TaskDialog 渲染异常
    let s = s.replace("\n", "\r\n");
    // 2) 按 UTF-16 编码并添加终止 null
    s.encode_utf16().chain(once(0)).collect()
}