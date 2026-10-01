//! Offline heatmap explanations projected from owned overlay snapshots.

use super::model::MapViewerWindowView;
use super::prelude::*;

impl MapViewerWindowView {
    pub(super) fn render_load_risk_legend(&self, colors: &ThemeColors, cx: &Context<Self>) -> Div {
        let _i18n = cx.global::<I18n>().clone();
        div()
            .absolute()
            .bottom(px(12.0))
            .left(px(12.0))
            .right(px(12.0))
            .max_w(px(480.0))
            .rounded(px(6.0))
            .px(px(10.0))
            .py(px(6.0))
            .bg(colors.surface)
            .text_xs()
            .child(t!("MapViewer.risk_short_legend"))
    }

    pub(super) fn render_load_risk_details(&self, cx: &Context<Self>) -> Div {
        let _i18n = cx.global::<I18n>().clone();
        let detail = self
            .professional
            .overlay_paint
            .as_ref()
            .and_then(|cache| {
                cache.chunk_loads.iter().find(|chunk| {
                    chunk.chunk_x == self.hover_block_x.div_euclid(16)
                        && chunk.chunk_z == self.hover_block_z.div_euclid(16)
                })
            })
            .map(|chunk| {
                format!(
                    "{}, {} · {} {} · {} {} · {} {}",
                    chunk.chunk_x,
                    chunk.chunk_z,
                    t!("MapViewer.risk_entities"),
                    chunk.entities,
                    t!("MapViewer.risk_block_entities"),
                    chunk.ticking_block_entities,
                    t!("MapViewer.risk_ticks"),
                    chunk.pending_ticks,
                )
            })
            .unwrap_or_else(|| t!("MapViewer.risk_no_counts").to_string());
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .text_xs()
            .child(t!("MapViewer.risk_legend"))
            .child(t!("MapViewer.risk_limits"))
            .child(self.load_risk_source_label(cx))
            .child(detail)
    }

    fn load_risk_source_label(&self, cx: &Context<Self>) -> String {
        let _i18n = cx.global::<I18n>().clone();
        let Some(cache) = self.professional.overlay_paint.as_ref() else {
            return t!("MapViewer.risk_unvalidated").to_string();
        };
        let summary = &cache.entity_cache_summary;
        if summary.requested_tile_count == 0 {
            return t!("MapViewer.risk_direct_query").to_string();
        }
        format!(
            "{} {}/{} · {}",
            t!("MapViewer.risk_tiles"),
            summary.cached_tile_count + summary.rebuilt_tile_count,
            summary.requested_tile_count,
            if summary.source_fingerprints_validated {
                t!("MapViewer.risk_validated")
            } else {
                t!("MapViewer.risk_unvalidated")
            }
        )
    }
}
