//! Search previously loaded block-entity overlay positions without touching LevelDB
//! from an input event or GPUI render. These are X/Z candidates, not verified containers.

use super::*;
use crate::ui::components::button::{ghost_button, secondary_button};

const MAX_VISIBLE_CANDIDATES: usize = 10;

/// Retain only the nearest candidates while scanning; never sort or materialize
/// an entire world overlay merely to render ten search results.
fn nearest_block_entity_positions(
    points: impl IntoIterator<Item = (i32, i32)>,
    center: (i32, i32),
    query: &str,
) -> (usize, Vec<(i32, i32)>) {
    let terms = query.split_whitespace().collect::<Vec<_>>();
    let mut total = 0usize;
    let mut nearest = Vec::<(u64, i32, i32)>::with_capacity(MAX_VISIBLE_CANDIDATES);
    for (x, z) in points {
        if !terms.is_empty() {
            let text = format!("x:{x} z:{z} {x},{z}");
            if !terms.iter().all(|term| text.contains(term)) {
                continue;
            }
        }
        total = total.saturating_add(1);
        if nearest
            .iter()
            .any(|(_, old_x, old_z)| *old_x == x && *old_z == z)
        {
            continue;
        }
        let dx = u64::from(x.abs_diff(center.0));
        let dz = u64::from(z.abs_diff(center.1));
        let distance = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz));
        let candidate = (distance, x, z);
        if nearest.len() == MAX_VISIBLE_CANDIDATES
            && nearest
                .last()
                .is_some_and(|furthest| candidate >= *furthest)
        {
            continue;
        }
        nearest.push(candidate);
        nearest.sort_unstable();
        nearest.truncate(MAX_VISIBLE_CANDIDATES);
    }
    (total, nearest.into_iter().map(|(_, x, z)| (x, z)).collect())
}

impl View {
    pub(in super::super) fn render_container_candidates(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let overlay_enabled = self.overlay_options.block_entities;
        // A previous dimension/view's cache can remain present during a new query.
        // Do not present those positions as current until the requested overlay completes.
        let overlay_ready = overlay_enabled && self.professional.overlay_complete;
        let query = self.map_image.container_search.read(cx).value();
        let query = query.trim().to_lowercase();
        let center = self.viewport.center_block(self.active_layout);
        let (matches, positions) = if overlay_ready {
            self.professional
                .overlay_paint
                .as_ref()
                .map_or((0, Vec::new()), |overlay| {
                    nearest_block_entity_positions(
                        overlay
                            .block_entity_points
                            .iter()
                            .map(|p| (p.block_x as i32, p.block_z as i32)),
                        center,
                        &query,
                    )
                })
        } else {
            (0, Vec::new())
        };

        div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .min_w(px(0.0))
            .child(inspector::section_title(
                colors,
                t!("MapViewer.imagegen.container_candidates.section_candidates"),
            ))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_secondary)
                    .child(t!(
                        "MapViewer.imagegen.container_candidates.candidates_hint"
                    )),
            )
            .when(!overlay_enabled, |panel| {
                panel.child(
                    secondary_button(
                        colors,
                        "map-enable-block-entity-overlay",
                        t!("MapViewer.imagegen.container_candidates.load_block_entities"),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if !this.overlay_options.block_entities {
                                this.toggle_block_entity_overlay(cx);
                            }
                        }),
                    ),
                )
            })
            .when(overlay_enabled, |panel| {
                panel
                    .child(
                        Input::new(&self.map_image.container_search)
                            .cleanable(true)
                            .w_full()
                            .h(px(32.0)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.text_secondary)
                            .child(if overlay_ready {
                                t!(
                                    "MapViewer.imagegen.container_candidates.candidates_count",
                                    matches = matches,
                                    limit = MAX_VISIBLE_CANDIDATES,
                                )
                                .to_string()
                            } else if self.professional.overlay_loading {
                                t!("MapViewer.imagegen.container_candidates.loading_index")
                                    .to_string()
                            } else {
                                t!("MapViewer.imagegen.container_candidates.no_index_result")
                                    .to_string()
                            }),
                    )
                    .when(
                        !overlay_ready && !self.professional.overlay_loading,
                        |panel| {
                            panel.child(
                                secondary_button(
                                    colors,
                                    "map-retry-container-candidates",
                                    t!("MapViewer.imagegen.container_candidates.retry_candidates"),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.refresh_professional_overlays(cx);
                                        cx.notify();
                                    }),
                                ),
                            )
                        },
                    )
                    .children(positions.into_iter().map(|(x, z)| {
                        ghost_button(
                            colors,
                            SharedString::from(format!("map-container-candidate-{x}-{z}")),
                            t!(
                                "MapViewer.imagegen.container_candidates.candidate_coords",
                                x = x,
                                z = z,
                            ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.center_viewport_on_block(x, z, cx);
                                cx.notify();
                            }),
                        )
                    }))
                    .when(matches == 0 && overlay_ready, |panel| {
                        panel.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_secondary)
                                .child(t!("MapViewer.imagegen.container_candidates.no_matches")),
                        )
                    })
            })
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_secondary)
                    .child(t!("MapViewer.imagegen.container_candidates.locate_hint")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::nearest_block_entity_positions;

    #[test]
    fn keeps_only_nearest_candidates_and_avoids_duplicate_rows() {
        let points = (0..40).map(|x| (x, 0)).chain([(1, 0), (1, 0)]);
        let (total, result) = nearest_block_entity_positions(points, (0, 0), "");
        assert_eq!(total, 42);
        assert_eq!(result.len(), 10);
        assert_eq!(result[0], (0, 0));
        assert_eq!(result[9], (9, 0));
    }

    #[test]
    fn matches_coordinate_terms_without_scanning_other_dimensions() {
        let points = [(12, 5), (12, 60), (-12, 5)];
        let (total, result) = nearest_block_entity_positions(points, (12, 5), "12,5");
        assert_eq!(total, 1);
        assert_eq!(result, vec![(12, 5)]);
    }
}
