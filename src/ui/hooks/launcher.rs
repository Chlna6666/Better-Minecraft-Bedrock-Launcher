#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
pub use windows::*;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "windows")]
pub(crate) use windows::{resume_after_uwp_check, snapshot_changed};
#[cfg(target_os = "linux")]
pub(crate) use linux::snapshot_changed;
