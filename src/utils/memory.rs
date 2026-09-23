// src/utils/memory.rs
//! 内存管理工具模块
//! 提供类似 MemReduct 的内存清理功能，以及 GPUI 资源管理

#![expect(unsafe_code, reason = "allocator cleanup crosses the mimalloc native API boundary")]

use tracing::debug;

/// 内存清理统计信息
#[derive(Default, Clone)]
pub struct MemoryStats {
    /// 工作集大小 (KB)
    pub working_set_kb: u64,
    /// 私有内存 (KB)
    pub private_kb: u64,
    /// 峰值工作集 (KB)
    pub peak_working_set_kb: u64,
}

impl MemoryStats {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(windows)]
    pub fn refresh(&mut self) {
        use windows::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;

        // SAFETY: Querying memory stats for the current process with a properly sized struct.
        unsafe {
            let process = GetCurrentProcess();
            let mut mem_info = PROCESS_MEMORY_COUNTERS_EX::default();
            let cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            if GetProcessMemoryInfo(
                process,
                &mut mem_info as *mut _
                    as *mut windows::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS,
                cb,
            )
            .is_ok()
            {
                self.working_set_kb = mem_info.WorkingSetSize as u64 / 1024;
                self.peak_working_set_kb = mem_info.PeakWorkingSetSize as u64 / 1024;
                self.private_kb = mem_info.PrivateUsage as u64 / 1024;
            }
        }
    }

    #[cfg(not(windows))]
    pub fn refresh(&mut self) {
        let system = sysinfo::System::new_all();
        let Ok(process_id) = sysinfo::get_current_pid() else {
            return;
        };
        let Some(process) = system.process(process_id) else {
            return;
        };

        self.working_set_kb = process.memory() / 1024;
        self.private_kb = self.working_set_kb;
        self.peak_working_set_kb = self.peak_working_set_kb.max(self.working_set_kb);
    }
}

pub fn configure_mimalloc_optimizer() {
    use mimalloc::MiMalloc;

    // SAFETY: These calls configure process-global mimalloc defaults during startup.
    // The option identifiers and value types come directly from libmimalloc-sys.
    unsafe {
        libmimalloc_sys::mi_option_set_enabled_default(
            libmimalloc_sys::mi_option_show_errors,
            false,
        );
        libmimalloc_sys::mi_option_set_enabled_default(libmimalloc_sys::mi_option_verbose, false);
        libmimalloc_sys::mi_option_set_enabled_default(
            libmimalloc_sys::mi_option_limit_os_alloc,
            false,
        );
        libmimalloc_sys::mi_option_set_default(libmimalloc_sys::mi_option_reserve_os_memory, 0);
    }

    let version = MiMalloc.version();
    debug!("Configured automatic mimalloc optimizer: version={version}");
}

/// 清理当前进程的工作集。
///
/// 这只会提示操作系统回收可驻留页，不等价于强制 allocator 释放提交内存。
/// 判断是否真的存在泄漏时，应优先关注 `private_kb`，而不是只看 `working_set_kb`。
#[cfg(windows)]
pub fn empty_working_set() -> MemoryStats {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, K32EmptyWorkingSet, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: We operate on the current process handle returned by the OS, and pass a valid,
    // correctly sized buffer to the memory information APIs.
    unsafe {
        let process = GetCurrentProcess();

        // 清空工作集
        let _ = K32EmptyWorkingSet(HANDLE(process.0));

        let mut mem_info = PROCESS_MEMORY_COUNTERS_EX::default();
        let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let mut stats = MemoryStats::new();
        if GetProcessMemoryInfo(
            process,
            &mut mem_info as *mut _
                as *mut windows::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS,
            cb,
        )
        .is_ok()
        {
            stats.working_set_kb = mem_info.WorkingSetSize as u64 / 1024;
            stats.peak_working_set_kb = mem_info.PeakWorkingSetSize as u64 / 1024;
            stats.private_kb = mem_info.PrivateUsage as u64 / 1024;

            debug!(
                "Working Set: {} KB | Private: {} KB | Peak: {} KB",
                stats.working_set_kb, stats.private_kb, stats.peak_working_set_kb
            );
        }
        stats
    }
}

#[cfg(not(windows))]
pub fn empty_working_set() -> MemoryStats {
    let mut stats = MemoryStats::new();
    stats.refresh();
    stats
}

pub fn trigger_mimalloc_collect(force: bool) {
    // SAFETY: `mi_collect` is a process-global allocator maintenance hook. It does not access
    // Rust references; the `force` flag only controls how aggressively mimalloc abandons pages.
    unsafe {
        libmimalloc_sys::mi_collect(force);
    }
}

pub fn force_memory_cleanup_aggressive() -> MemoryStats {
    trigger_mimalloc_collect(true);
    let stats = empty_working_set();
    debug!(
        "Aggressive memory cleanup: WorkingSet={}KB, Private={}KB, Peak={}KB",
        stats.working_set_kb, stats.private_kb, stats.peak_working_set_kb
    );
    stats
}

pub fn spawn_working_set_trim_task(reason: &'static str) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut before = MemoryStats::new();
        before.refresh();
        debug!(
            "Working set trim requested: reason={} before_working_set={}KB before_private={}KB",
            reason, before.working_set_kb, before.private_kb
        );
        let after = empty_working_set();
        debug!(
            "Working set trim finished: reason={} after_working_set={}KB after_private={}KB",
            reason, after.working_set_kb, after.private_kb
        );
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn test_empty_working_set() {
        let stats = empty_working_set();
        assert!(stats.working_set_kb > 0);
    }

}
