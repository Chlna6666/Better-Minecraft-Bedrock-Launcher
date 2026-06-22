use anyhow::Result;
use gpui::App;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod asset_source;
pub mod generated;

static STARTUP_FONTS_LOADED: AtomicBool = AtomicBool::new(false);

pub fn load_startup_fonts(_cx: &mut App) -> Result<()> {
    if STARTUP_FONTS_LOADED.swap(true, Ordering::AcqRel) {
        return Ok(());
    }

    Ok(())
}

pub fn load_embedded_fonts(cx: &mut App) -> Result<()> {
    load_startup_fonts(cx)
}

pub fn spawn_deferred_font_load(_cx: &mut App) {}
