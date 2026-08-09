pub mod agreement;
pub(crate) mod bedrock_auth;
pub mod debug;
pub mod diagnostics;
pub mod i18n;
#[cfg(target_os = "windows")]
pub mod launch_prereq;
pub mod launcher;
#[cfg(target_os = "linux")]
pub mod linux_runtime;
pub mod local_versions;
#[cfg(target_os = "windows")]
pub mod music;
#[cfg(target_os = "windows")]
mod music_loader;
#[cfg(target_os = "windows")]
mod music_types;
pub mod navigation;
pub mod quit;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) mod sound_effect;
pub mod theme;
pub mod update;
