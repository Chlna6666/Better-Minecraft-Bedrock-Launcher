/// OS process memory, independent of renderer allocation accounting. On UMA,
/// process and GPU observations may overlap and must not be summed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcessMemorySnapshot {
    /// Resident process working set, including shared pages.
    pub working_set_bytes: u64,
    /// Private committed process bytes.
    pub private_bytes: u64,
    /// Process commit charge; this is not virtual address space size.
    pub committed_bytes: u64,
}

/// Samples OS process memory explicitly for diagnostics, outside render paths.
/// Unsupported platforms return `None`, rather than a fabricated zero sample.
///
/// # Errors
/// Returns an OS error if the supported process query fails.
#[cfg_attr(
    windows,
    allow(
        unsafe_code,
        reason = "Win32 process counters require FFI with a sized, live output buffer"
    )
)]
pub fn process_memory_snapshot() -> anyhow::Result<Option<ProcessMemorySnapshot>> {
    #[cfg(windows)]
    {
        use windows::Win32::System::{
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
            Threading::GetCurrentProcess,
        };
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        // SAFETY: the current process pseudo-handle is valid and the output
        // pointer references the complete, correctly sized EX structure.
        unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters as *mut _ as *mut _,
                counters.cb,
            )
        }?;
        Ok(Some(ProcessMemorySnapshot {
            working_set_bytes: counters.WorkingSetSize as u64,
            private_bytes: counters.PrivateUsage as u64,
            committed_bytes: counters.PagefileUsage as u64,
        }))
    }
    #[cfg(not(windows))]
    {
        Ok(None)
    }
}
