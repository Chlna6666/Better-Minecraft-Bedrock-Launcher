#[cfg(target_os = "windows")]
pub mod preflight;
#[cfg(target_os = "windows")]
pub mod start;
#[cfg(target_os = "windows")]
pub mod task;
#[cfg(target_os = "linux")]
#[path = "task_linux.rs"]
pub mod task;
#[cfg(target_os = "windows")]
pub use start::{launch_uwp, wait_for_uwp_pid};
pub use task::{LaunchRequest, start_launch_task};
