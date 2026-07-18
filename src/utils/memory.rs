// src/utils/memory.rs
//! 内存管理工具模块
//! 提供类似 MemReduct 的内存清理功能，以及 GPUI 资源管理

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::debug;

const DEFAULT_TRIM_FORCE: bool = false;

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
        // 非 Windows 平台暂不实现
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
    // 非 Windows 平台暂不实现
    MemoryStats::new()
}

/// 尝试触发一次显式内存整理。
///
/// Rust 没有运行时 GC，这里最多只能帮助释放短生命周期对象并给 allocator 留出回收机会。
/// 如果 Working Set 没有明显下降，并不代表存在泄漏；Private Usage 更接近真实提交量。
pub fn trigger_gc() {
    trigger_mimalloc_collect(DEFAULT_TRIM_FORCE);
    debug!("Triggered mimalloc collect");
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

pub fn force_memory_cleanup() -> MemoryStats {
    trigger_gc();
    let stats = empty_working_set();
    debug!(
        "Forced memory cleanup: WorkingSet={}KB, Private={}KB, Peak={}KB",
        stats.working_set_kb, stats.private_kb, stats.peak_working_set_kb
    );
    stats
}

/// 内存清理管理器
pub struct MemoryManager {
    /// 上次清理时间
    last_cleanup: Arc<AtomicU64>,
    /// 清理间隔（秒）
    cleanup_interval_secs: u64,
    /// 私有提交内存阈值（KB），超过此值触发清理
    memory_threshold_kb: u64,
}

impl MemoryManager {
    pub fn new(cleanup_interval_secs: u64, memory_threshold_kb: u64) -> Self {
        Self {
            last_cleanup: Arc::new(AtomicU64::new(current_unix_seconds())),
            cleanup_interval_secs,
            memory_threshold_kb,
        }
    }

    /// 检查是否需要清理内存
    pub fn should_cleanup(&self) -> bool {
        let now = current_unix_seconds();

        let last = self.last_cleanup.load(Ordering::Relaxed);
        now - last >= self.cleanup_interval_secs
    }

    /// 执行内存清理
    pub fn cleanup(&self) -> MemoryStats {
        let now = current_unix_seconds();

        self.last_cleanup.store(now, Ordering::Relaxed);

        // 1. 触发 GC
        trigger_gc();

        // 2. 清空工作集
        let stats = empty_working_set();

        debug!(
            "Memory cleanup: WorkingSet={}KB, Private={}KB, Peak={}KB",
            stats.working_set_kb, stats.private_kb, stats.peak_working_set_kb
        );

        stats
    }

    /// 根据内存使用情况决定是否清理
    pub fn cleanup_if_needed(&self) -> Option<MemoryStats> {
        let mut stats = MemoryStats::new();
        stats.refresh();

        if stats.private_kb > self.memory_threshold_kb {
            debug!(
                "Private usage {}KB exceeds threshold {}KB (working set {}KB), triggering cleanup",
                stats.private_kb, self.memory_threshold_kb, stats.working_set_kb
            );
            Some(self.cleanup())
        } else {
            None
        }
    }
}

fn current_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 启动后台内存清理任务
pub fn spawn_memory_cleanup_task(
    cleanup_interval_secs: u64,
    memory_threshold_kb: u64,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let manager = MemoryManager::new(cleanup_interval_secs, memory_threshold_kb);

        loop {
            std::thread::sleep(Duration::from_secs(1));

            if manager.should_cleanup() {
                manager.cleanup_if_needed();
            }
        }
    })
}

pub fn spawn_startup_memory_cleanup_task(delay_secs: u64) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(delay_secs));
        let mut before = MemoryStats::new();
        before.refresh();
        debug!(
            "Startup memory cleanup check: WorkingSet={}KB, Private={}KB, Peak={}KB",
            before.working_set_kb, before.private_kb, before.peak_working_set_kb
        );
        let _ = force_memory_cleanup();
    })
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

    #[test]
    fn test_memory_manager() {
        let manager = MemoryManager::new(60, 500 * 1024); // 60 秒间隔，500MB 阈值
        assert!(!manager.should_cleanup()); // 首次应该为 false
    }
}
