mod app;
mod archive;
mod assets;
mod config;
mod core;
mod downloads;
mod http;
mod i18n;
mod launch;
mod music;
mod plugins;
mod result;
mod startup;
mod tasks;
mod ui;
mod utils;

pub use app::APP_ID;

pub fn run() -> anyhow::Result<()> {
    startup::run()
}
