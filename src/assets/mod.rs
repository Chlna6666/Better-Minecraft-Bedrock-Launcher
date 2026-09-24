use anyhow::Result;
use gpui::{App, AppContext as _};
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

pub fn spawn_deferred_font_load(cx: &mut App) {
    let text_system = cx.text_system().clone();
    cx.spawn(async move |cx| {
        let text_system_for_prepare = text_system.clone();
        cx.background_spawn(async move {
            text_system_for_prepare.prepare_system_fonts();
        })
        .await;

        let font_catalog_changed = cx.update(|_| text_system.publish_prepared_system_fonts())?;
        if font_catalog_changed {
            let text_system_for_prewarm = text_system.clone();
            cx.background_spawn(async move {
                text_system_for_prewarm.prewarm_loaded_fonts();
            })
            .await;
            cx.update(|cx| cx.refresh_windows())?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}
