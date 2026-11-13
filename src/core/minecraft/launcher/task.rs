#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
pub use windows::*;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "windows")]
pub(crate) use windows::start_launch_task_observed;
#[cfg(target_os = "linux")]
pub(crate) use linux::start_launch_task_observed;
