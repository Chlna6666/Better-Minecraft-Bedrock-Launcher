#![recursion_limit = "256"]

mod app;
mod archive;
mod assets;
mod config;
mod core;
mod downloads;
mod http;
#[macro_use]
mod i18n;
mod launch;
mod plugins;
mod result;
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
