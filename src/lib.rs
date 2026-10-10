#![recursion_limit = "512"]

mod app;
mod archive;
mod assets;
mod config;
mod core;
mod downloads;
mod github;
mod http;
#[macro_use]
mod i18n;
mod launch;
mod plugins;
mod result;

/// 插件系统性能基准使用的入口。
///
/// 只在 `cargo bench --features bench-support` 下存在，正常构建不会暴露这些 re-export。
#[cfg(feature = "bench-support")]
pub mod bench_support;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod startup;
mod tasks;
mod ui;
mod utils;

pub use app::APP_ID;

#[cfg(target_os = "windows")]
pub fn run_windows_terminal_host_if_requested() -> anyhow::Result<bool> {
    core::windows_terminal::run_host_from_args().map_err(anyhow::Error::msg)
}

pub fn run() -> anyhow::Result<()> {
    startup::run()
}
