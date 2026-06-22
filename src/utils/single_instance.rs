//! 单实例管理模块
//!
//! 使用 Windows 命名互斥量确保同一时间只有一个实例运行

#[cfg(windows)]
use std::sync::Mutex;

#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;

/// 存储单实例互斥量句柄
/// 使用 RawHandle 包装以避免 Send 检查
#[cfg(windows)]
struct RawHandle(*mut std::ffi::c_void);

#[cfg(windows)]
unsafe impl Send for RawHandle {}
#[cfg(windows)]
unsafe impl Sync for RawHandle {}

#[cfg(windows)]
static SINGLE_INSTANCE_MUTEX: Mutex<Option<RawHandle>> = Mutex::new(None);

/// 设置单实例互斥量句柄
#[cfg(windows)]
pub fn set_mutex_handle(handle: HANDLE) {
    *SINGLE_INSTANCE_MUTEX.lock().unwrap() = Some(RawHandle(handle.0));
}

/// 释放单实例互斥量
/// 在程序退出前调用，确保互斥量被正确释放
#[cfg(windows)]
pub fn release_mutex() {
    let mut mutex_opt = SINGLE_INSTANCE_MUTEX.lock().unwrap();
    if let Some(RawHandle(ptr)) = mutex_opt.take() {
        unsafe {
            let handle = HANDLE(ptr);
            let _ = windows::Win32::System::Threading::ReleaseMutex(handle);
            let _ = windows::Win32::Foundation::CloseHandle(handle);
        }
    }
}

#[cfg(not(windows))]
pub fn release_mutex() {
    // 非 Windows 平台无需操作
}
