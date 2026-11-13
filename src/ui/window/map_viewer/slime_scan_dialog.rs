use super::model::*;
use super::overlays::{SlimeFarmSearchScopeSource, practical_slime_farm_scope};
use super::panels::{mode_button, panel_field_label, status_badge};
use super::prelude::*;
use crate::ui::components::{
    button::{ghost_button, primary_button},
    dialog, modal,
};
use std::rc::Rc;

pub(super) fn advanced_slime_scan_bounds(
    center_chunk: (i32, i32),
    preset: AdvancedSlimeScanPreset,
) -> SlimeChunkBounds {
    let radius = preset.radius_chunks();
    SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: center_chunk.0.saturating_sub(radius),
        max_chunk_x: center_chunk.0.saturating_add(radius),
        min_chunk_z: center_chunk.1.saturating_sub(radius),
        max_chunk_z: center_chunk.1.saturating_add(radius),
    }
}

impl View {
    pub(super) fn open_advanced_slime_scan_dialog(&mut self, cx: &mut Context<Self>) {
        let _i18n = cx.global::<I18n>().clone();
        if self.dimension != Dimension::Overworld {
            self.status = t!("MapViewer.slime_overworld_only");
            cx.notify();
            return;
        }

        let default_anchor = self
            .advanced_slime_scan
            .map(|scan| scan.anchor)
            .unwrap_or_else(|| {
                if self.professional.selection.is_some() {
                    AdvancedSlimeScanAnchor::SelectionCenter
                } else if self.selected_player_slime_bounds().is_some() {
                    AdvancedSlimeScanAnchor::SelectedPlayer
                } else {
                    AdvancedSlimeScanAnchor::ViewportCenter
                }
            });
        let default_preset = self
            .advanced_slime_scan
            .map_or(AdvancedSlimeScanPreset::Regional, |scan| scan.preset);

        self.advanced_slime_scan_dialog = Some(AdvancedSlimeScanDialogState {
            anchor: default_anchor,
            preset: default_preset,
            mode: self.slime_farm_search_mode,
            dismiss: modal::DismissHandle::new(),
        });
        cx.notify();
    }

    pub(super) fn set_advanced_slime_scan_anchor(
        &mut self,
        anchor: AdvancedSlimeScanAnchor,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.advanced_slime_scan_dialog.as_mut() {
            dialog.anchor = anchor;
            cx.notify();
        }
    }

    pub(super) fn set_advanced_slime_scan_preset(
        &mut self,
        preset: AdvancedSlimeScanPreset,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.advanced_slime_scan_dialog.as_mut() {
            dialog.preset = preset;
            cx.notify();
        }
    }

    pub(super) fn set_advanced_slime_scan_mode(
        &mut self,
        mode: SlimeFarmSearchMode,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.advanced_slime_scan_dialog.as_mut() {
            dialog.mode = mode;
            cx.notify();
        }
    }

    fn scan_anchor_chunk(
        &self,
        anchor: AdvancedSlimeScanAnchor,
    ) -> Option<(i32, i32)> {
        match anchor {
            AdvancedSlimeScanAnchor::ViewportCenter => {
                let (block_x, block_z) = self.viewport.center_block(self.active_layout);
                Some((block_x.div_euclid(16), block_z.div_euclid(16)))
            }
            AdvancedSlimeScanAnchor::SelectionCenter => self
                .professional
                .selection
                .map(|selection| selection.bounds().center()),
            AdvancedSlimeScanAnchor::SelectedPlayer => self
                .selected_player_slime_bounds()
                .map(SlimeChunkBounds::center),
            AdvancedSlimeScanAnchor::WorldOrigin => Some((0, 0)),
        }
    }

    fn start_advanced_slime_scan(&mut self, cx: &mut Context<Self>) -> bool {
        let _i18n = cx.global::<I18n>().clone();
        let Some(dialog) = self.advanced_slime_scan_dialog.clone() else {
            return false;
        };
        let Some(center_chunk) = self.scan_anchor_chunk(dialog.anchor) else {
            self.status = t!("MapViewer.slime_advanced_anchor_unavailable");
            cx.notify();
            return false;
        };
        let requested = advanced_slime_scan_bounds(center_chunk, dialog.preset);
        let Some(scope) =
            practical_slime_farm_scope(requested, SlimeFarmSearchScopeSource::Advanced)
        else {
            self.status = t!("MapViewer.slime_scope_outside_practical");
            cx.notify();
            return false;
        };

        self.cancel_slime_farm_candidate_query();
        self.professional.slime_farm_candidates = None;
        self.professional.highlighted_slime_candidate = None;
        self.slime_farm_search_mode = dialog.mode;
        self.advanced_slime_scan = Some(AdvancedSlimeScanRequest {
            bounds: requested,
            preset: dialog.preset,
            anchor: dialog.anchor,
            max_results: dialog.preset.max_results(),
        });
        let chunk_count = scope.bounds.chunk_count().to_string();
        let radius = dialog.preset.radius_blocks().to_string();
        self.status = t!(
            "MapViewer.slime_advanced_started",
            chunks = &chunk_count,
            radius = &radius
        );
        self.refresh_professional_render_caches(cx);
        cx.notify();
        true
    }

    pub(super) fn render_advanced_slime_scan_modal(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let _i18n = cx.global::<I18n>().clone();
        let dialog_state = self.advanced_slime_scan_dialog.clone()?;
        let center_chunk = self.scan_anchor_chunk(dialog_state.anchor);
        let preview_scope = center_chunk
            .map(|center| advanced_slime_scan_bounds(center, dialog_state.preset))
            .and_then(|bounds| {
                practical_slime_farm_scope(bounds, SlimeFarmSearchScopeSource::Advanced)
            });
        let anchor_ready = center_chunk.is_some() && preview_scope.is_some();

        let anchor_buttons = [
            (
                AdvancedSlimeScanAnchor::ViewportCenter,
                t!("MapViewer.slime_advanced_anchor_viewport"),
                true,
            ),
            (
                AdvancedSlimeScanAnchor::SelectionCenter,
                t!("MapViewer.slime_advanced_anchor_selection"),
                self.professional.selection.is_some(),
            ),
            (
                AdvancedSlimeScanAnchor::SelectedPlayer,
                t!("MapViewer.slime_advanced_anchor_player"),
                self.selected_player_slime_bounds().is_some(),
            ),
            (
                AdvancedSlimeScanAnchor::WorldOrigin,
                t!("MapViewer.slime_advanced_anchor_origin"),
                true,
            ),
        ]
        .into_iter()
        .map(|(anchor, label, available)| {
            let button = mode_button(colors, label, dialog_state.anchor == anchor).min_w(px(128.0));
            if available {
                button
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.set_advanced_slime_scan_anchor(anchor, cx)
                        }),
                    )
                    .into_any_element()
            } else {
                button.opacity(0.42).into_any_element()
            }
        })
        .collect::<Vec<_>>();

        let preset_buttons = [
            (
                AdvancedSlimeScanPreset::Nearby,
                t!("MapViewer.slime_advanced_preset_nearby"),
            ),
            (
                AdvancedSlimeScanPreset::Regional,
                t!("MapViewer.slime_advanced_preset_regional"),
            ),
            (
                AdvancedSlimeScanPreset::Comprehensive,
                t!("MapViewer.slime_advanced_preset_comprehensive"),
            ),
        ]
        .into_iter()
        .map(|(preset, label)| {
            mode_button(colors, label, dialog_state.preset == preset)
                .min_w(px(128.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.set_advanced_slime_scan_preset(preset, cx)
                    }),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

        let mode_buttons = [
            (
                SlimeFarmSearchMode::Quad2x2,
                t!("MapViewer.slime_mode_quad"),
            ),
            (
                SlimeFarmSearchMode::Rectangle2x3,
                t!("MapViewer.slime_mode_rect_six"),
            ),
            (
                SlimeFarmSearchMode::Square3x3,
                t!("MapViewer.slime_mode_square_nine"),
            ),
            (
                SlimeFarmSearchMode::LargestConnected,
                t!("MapViewer.slime_mode_largest"),
            ),
        ]
        .into_iter()
        .map(|(mode, label)| {
            mode_button(colors, label, dialog_state.mode == mode)
                .min_w(px(128.0))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.set_advanced_slime_scan_mode(mode, cx)
                    }),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

        let radius_blocks = dialog_state.preset.radius_blocks().to_string();
        let max_results = dialog_state.preset.max_results().to_string();
        let scanned_chunks = preview_scope
            .map(|scope| scope.bounds.chunk_count())
            .unwrap_or_else(|| dialog_state.preset.query_chunk_count())
            .to_string();
        let center_text = center_chunk.map_or_else(
            || t!("MapViewer.slime_advanced_center_unavailable"),
            |(x, z)| SharedString::from(format!("chunk {x}, {z}")),
        );
        let summary = t!(
            "MapViewer.slime_advanced_summary",
            radius = &radius_blocks,
            chunks = &scanned_chunks,
            results = &max_results
        );

        let body = div()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scrollbar()
            .px(px(22.0))
            .pb(px(14.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(panel_field_label(
                colors,
                t!("MapViewer.slime_advanced_anchor"),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(7.0))
                    .children(anchor_buttons),
            )
            .child(status_badge(colors, center_text))
            .child(panel_field_label(
                colors,
                t!("MapViewer.slime_advanced_preset"),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(7.0))
                    .children(preset_buttons),
            )
            .child(panel_field_label(colors, t!("MapViewer.slime_search_mode")))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(7.0))
                    .children(mode_buttons),
            )
            .when(dialog_state.mode.is_exact_template(), |this| {
                this.child(status_badge(
                    colors,
                    t!("MapViewer.slime_pattern_isolation_hint"),
                ))
            })
            .when(
                dialog_state.mode == SlimeFarmSearchMode::Square3x3,
                |this| this.child(status_badge(colors, t!("MapViewer.slime_3x3_rare"))),
            )
            .child(status_badge(colors, summary))
            .when(!anchor_ready, |this| {
                this.child(status_badge(
                    colors,
                    t!("MapViewer.slime_advanced_anchor_unavailable"),
                ))
            })
            .when_some(preview_scope, |this, scope| {
                this.when(scope.precision_degraded, |this| {
                    this.child(status_badge(
                        colors,
                        t!("MapViewer.slime_scope_precision_warning"),
                    ))
                })
                .when(scope.clipped_for_precision, |this| {
                    this.child(status_badge(colors, t!("MapViewer.slime_scope_clipped")))
                })
            });

        let cancel_dismiss = dialog_state.dismiss.clone();
        let cancel_button = ghost_button(colors, "map-slime-advanced-cancel", t!("common.cancel"))
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                cancel_dismiss.dismiss(cx);
            });

        let mut confirm_button = primary_button(
            colors,
            "map-slime-advanced-start",
            t!("MapViewer.slime_advanced_start"),
        );
        if anchor_ready {
            let confirm_view = cx.entity().downgrade();
            let confirm_dismiss = dialog_state.dismiss.clone();
            confirm_button = confirm_button.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let started = confirm_view
                    .update(cx, |this, cx| this.start_advanced_slime_scan(cx))
                    .unwrap_or(false);
                if started {
                    confirm_dismiss.dismiss(cx);
                }
            });
        } else {
            confirm_button = confirm_button.opacity(0.45);
        }

        let content = dialog::container(colors, px(680.0))
            .child(dialog::header(
                colors,
                t!("MapViewer.slime_advanced_title"),
                Some(t!("MapViewer.slime_advanced_description")),
            ))
            .child(body)
            .child(dialog::actions(
                colors,
                cancel_button,
                confirm_button,
            ));

        let dismiss_view = cx.entity().downgrade();
        let on_dismiss = Rc::new(move |cx: &mut App| {
            let _ = dismiss_view.update(cx, |this, cx| {
                this.advanced_slime_scan_dialog = None;
                cx.notify();
            });
        });

        Some(modal::dismissible_layer(
            content,
            colors.backdrop,
            on_dismiss,
            modal::DismissOptions::default().with_handle(dialog_state.dismiss),
        ))
    }
}
