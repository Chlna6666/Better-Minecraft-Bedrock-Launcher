use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize)]
struct StartupProcessSample {
    working_set_bytes: u64,
    private_bytes: u64,
    virtual_bytes: u64,
}

#[derive(Serialize)]
struct StartupReport {
    process: StartupProcessSample,
    gpui: gpui::PerformanceMetricsSnapshot,
}

pub(crate) fn report_path() -> Option<PathBuf> {
    std::env::var_os("GPUI_STARTUP_REPORT_PATH").map(PathBuf::from)
}

pub(crate) fn write_if_requested() {
    let Some(path) = report_path() else {
        return;
    };

    let report = StartupReport {
        process: process_memory_sample(),
        gpui: gpui::performance_metrics_snapshot(),
    };

    match serde_json::to_string_pretty(&report) {
        Ok(json) => {
            if let Err(error) = fs::write(&path, json) {
                eprintln!(
                    "failed to write startup report to {}: {error}",
                    path.display()
                );
            }
        }
        Err(error) => {
            eprintln!("failed to serialize startup report: {error}");
        }
    }
}

#[cfg(windows)]
fn process_memory_sample() -> StartupProcessSample {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe {
        let _ = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            counters.cb,
        );
    }

    StartupProcessSample {
        working_set_bytes: counters.WorkingSetSize as u64,
        private_bytes: counters.PrivateUsage as u64,
        virtual_bytes: counters.PagefileUsage as u64,
    }
}

#[cfg(not(windows))]
fn process_memory_sample() -> StartupProcessSample {
    StartupProcessSample {
        working_set_bytes: 0,
        private_bytes: 0,
        virtual_bytes: 0,
    }
}
