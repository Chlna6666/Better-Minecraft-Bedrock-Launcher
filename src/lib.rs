#![recursion_limit = "256"]

mod app;
mod archive;
mod assets;
mod config;
mod core;
mod downloads;
mod http;
mod i18n;
mod launch;
#[cfg(target_os = "windows")]
mod music;
mod plugins;
mod result;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod sound_effect;
mod startup;
mod tasks;
mod ui;
mod utils;

pub use app::APP_ID;

pub fn run() -> anyhow::Result<()> {
    startup::run()
}
