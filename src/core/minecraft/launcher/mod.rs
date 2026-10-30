mod bloader;
#[cfg(target_os = "windows")]
pub mod preflight;
#[cfg(target_os = "windows")]
pub mod start;
#[cfg(target_os = "windows")]
pub mod task;
#[cfg(target_os = "linux")]
#[path = "task_linux.rs"]
pub mod task;
pub(crate) use task::start_launch_task_observed;
pub use task::{LaunchRequest, start_launch_task};
