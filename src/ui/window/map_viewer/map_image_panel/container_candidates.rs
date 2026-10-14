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
        if nearest.iter().any(|(_, old_x, old_z)| *old_x == x && *old_z == z) {
            continue;
        }
        let dx = u64::from(x.abs_diff(center.0));
        let dz = u64::from(z.abs_diff(center.1));
        let distance = dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz));
        let candidate = (distance, x, z);
        if nearest.len() == MAX_VISIBLE_CANDIDATES
            && nearest.last().is_some_and(|furthest| candidate >= *furthest)
        {
            continue;
        }
        nearest.push(candidate);
        nearest.sort_unstable();
        nearest.truncate(MAX_VISIBLE_CANDIDATES);
    }
    (total, nearest.into_iter().map(|(_, x, z)| (x, z)).collect())
}

impl MapViewerWindowView {
    pub(in super::super) fn render_map_container_candidates(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let overlay_enabled = self.overlay_options.block_entities;
        let query = self.map_image.container_search.read(cx).value();
        let query = query.trim().to_lowercase();
        let center = self.viewport.center_block(self.active_layout);
        let (matches, positions) = if overlay_enabled {
            self.professional.overlay_paint.as_ref().map_or(
                (0, Vec::new()),
                |overlay| {
                    nearest_block_entity_positions(
                        overlay.block_entity_points.iter().map(|p| {
                            (p.block_x as i32, p.block_z as i32)
                        }),
                        center,
                        &query,
                    )
                },
            )
        } else {
            (0, Vec::new())
        };

        div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .min_w(px(0.0))
            .child(generator_section_title(colors, "附近方块实体候选"))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_secondary)
                    .child("复用当前已加载的世界叠加数据，只包含 X/Z；不保证是箱子、也未确认 Y 高度或可用容量。"),
            )
            .when(!overlay_enabled, |panel| {
                panel.child(
                    secondary_button(colors, "map-enable-block-entity-overlay", "加载当前视野的方块实体")
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
                            .child(if self.professional.overlay_loading {
                                "正在更新可见范围索引…".to_owned()
                            } else {
                                format!("匹配 {matches} 条位置记录 · 按距视口中心排序，最多显示 {MAX_VISIBLE_CANDIDATES} 条")
                            }),
                    )
                    .children(positions.into_iter().map(|(x, z)| {
                        ghost_button(
                            colors,
                            SharedString::from(format!("map-container-candidate-{x}-{z}")),
                            format!("X {x} · Z {z}  →  定位"),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.center_viewport_on_block(x, z, cx);
                                cx.notify();
                            }),
                        )
                    }))
                    .when(matches == 0 && !self.professional.overlay_loading, |panel| {
                        panel.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_secondary)
                                .child("当前缓存范围未找到匹配项；可以缩放/移动地图，或清空搜索。"),
                        )
                    })
            })
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(colors.text_secondary)
                    .child("定位后调整 Y 层，在地图上右键实际容器并选「设为地图写入目标」。写入前仍会验证目标和容量。"),
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
