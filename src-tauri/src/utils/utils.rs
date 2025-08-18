use std::ffi::OsStr;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;

/// 辅助：将 Rust 字符串转为宽字符（以 `\0` 结尾）
pub fn to_wstr(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(once(0)).collect()
}