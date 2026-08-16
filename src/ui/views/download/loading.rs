use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use gpui::{AnimationExt as _, *};
use std::f32::consts::TAU;
use std::time::Duration;

const LOADING_PULSE_DURATION: Duration = Duration::from_millis(1400);
const LOADING_ROW_HEIGHT: f32 = 76.0;
const LOADING_ROW_SEPARATOR_HEIGHT: f32 = 1.0;
const LOADING_MIN_ROWS: usize = 4;
const LOADING_MAX_ROWS: usize = 8;
const LOADING_ROW_PHASE: f32 = 0.095;
const LOADING_BLOCK_PHASE: f32 = 0.035;

pub(super) fn should_render_loading(state: &DownloadPageState, tab: DownloadTab) -> bool {
    match tab {
        DownloadTab::Game => {
            (state.loading || state.force_refresh_next) && state.versions.is_empty()
        }
        DownloadTab::ResourcePack => {
            (!state.curseforge_loaded && state.curseforge_loading)
                || (state.curseforge_results_loading && state.curseforge_mods.is_empty())
        }
        DownloadTab::Mod => state.levilauncher_loading && !state.levilauncher_loaded,
    }
}

fn loading_row_count(viewport_height: Pixels) -> usize {
    let height = (viewport_height / px(1.0)).max(1.0);
    let reserved = 72.0;
    let available = (height - reserved).max(LOADING_ROW_HEIGHT);
    ((available / (LOADING_ROW_HEIGHT + LOADING_ROW_SEPARATOR_HEIGHT)).ceil() as usize)
        .clamp(LOADING_MIN_ROWS, LOADING_MAX_ROWS)
}

fn pulse_phase(row: usize, block: usize) -> f32 {
    (row as f32 * LOADING_ROW_PHASE + block as f32 * LOADING_BLOCK_PHASE).fract()
}

fn pulse_animation(phase_offset: f32) -> Animation {
    Animation::new(LOADING_PULSE_DURATION)
        .repeat()
        .with_easing(move |t| {
            let phase = (t + phase_offset).fract();
            // 余弦往返的首尾值一致，不会在循环边界发生闪跳。
            (0.5 - 0.5 * (TAU * phase).cos()).clamp(0.0, 1.0)
        })
}

fn animated_block(
    id: SharedString,
    width: Pixels,
    height: Pixels,
    radius: Pixels,
    color: Hsla,
    min_alpha: f32,
    max_alpha: f32,
    phase_offset: f32,
) -> AnyElement {
    div()
        .w(width)
        .h(height)
        .rounded(radius)
        .bg(Hsla {
            a: min_alpha,
            ..color
        })
        .with_animation(id, pulse_animation(phase_offset), move |this, t| {
            this.bg(Hsla {
                a: min_alpha + (max_alpha - min_alpha) * t,
                ..color
            })
        })
        .into_any_element()
}

fn animated_bar(
    colors: &ThemeColors,
    row: usize,
    block: usize,
    width: Pixels,
    height: Pixels,
) -> AnyElement {
    animated_block(
        SharedString::from(format!("download-loading-{row}-{block}")),
        width,
        height,
        px(crate::ui::theme::tokens::radius::FULL),
        colors.text_secondary,
        0.045,
        0.20,
        pulse_phase(row, block),
    )
}

fn skeleton_row(colors: &ThemeColors, row: usize) -> Div {
    let image = animated_block(
        SharedString::from(format!("download-loading-{row}-image")),
        px(42.0),
        px(42.0),
        px(crate::ui::theme::tokens::radius::SM),
        colors.text_secondary,
        0.055,
        0.22,
        pulse_phase(row, 0),
    );
    let title = animated_bar(colors, row, 1, px(240.0), px(15.0));
    let tag = animated_block(
        SharedString::from(format!("download-loading-{row}-tag")),
        px(72.0),
        px(20.0),
        px(crate::ui::theme::tokens::radius::SM),
        colors.text_secondary,
        0.04,
        0.15,
        pulse_phase(row, 2),
    );
    let meta_left = animated_bar(colors, row, 3, px(68.0), px(10.0));
    let meta_right = animated_bar(colors, row, 4, px(96.0), px(10.0));
    let status = animated_bar(colors, row, 5, px(78.0), px(14.0));
    let action = animated_block(
        SharedString::from(format!("download-loading-{row}-action")),
        px(88.0),
        px(28.0),
        px(crate::ui::theme::tokens::radius::FULL),
        colors.accent,
        0.07,
        0.24,
        pulse_phase(row, 6),
    );

    div()
        .w_full()
        .h(px(LOADING_ROW_HEIGHT))
        .min_h(px(LOADING_ROW_HEIGHT))
        .px(px(24.0))
        .py(px(12.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .items_center()
                .min_w(px(0.0))
                .flex_1()
                .child(
                    div()
                        .w(px(64.0))
                        .flex()
                        .items_center()
                        .child(image),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .pr(px(16.0))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(title)
                                .child(tag),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(meta_left)
                                .child(meta_right),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(status)
                .child(action),
        )
}

fn skeleton_row_with_separator(colors: &ThemeColors, row: usize) -> Div {
    div()
        .flex()
        .flex_col()
        .child(skeleton_row(colors, row))
        .child(div().h(px(LOADING_ROW_SEPARATOR_HEIGHT)).bg(Hsla {
            a: 0.055,
            ..colors.border
        }))
}

pub(super) fn render_loading_placeholder(
    colors: &ThemeColors,
    viewport_height: Pixels,
) -> impl IntoElement {
    let row_count = loading_row_count(viewport_height);
    let rows = (0..row_count)
        .map(|row| skeleton_row_with_separator(colors, row).into_any_element())
        .collect::<Vec<_>>();

    let footer_color = colors.text_secondary;
    let footer = div()
        .w_full()
        .h(px(32.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: 0.04,
            ..footer_color
        })
        .with_animation(
            "download-loading-footer",
            pulse_animation(pulse_phase(row_count, 0)),
            move |this, t| {
                this.bg(Hsla {
                    a: 0.04 + 0.13 * t,
                    ..footer_color
                })
            },
        );

    // 动画直接作用在每个实际绘制的占位矩形上，而不是依赖父容器
    // Opacity 或场景动画传播。各块带有轻微相位差，因此会形成从左到右、
    // 从上到下连续流动的呼吸效果，同时尺寸和位置保持完全静态。
    div()
        .size_full()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .min_w(px(0.0))
                .overflow_hidden()
                .flex()
                .flex_col()
                .children(rows),
        )
        .child(div().px(px(14.0)).py(px(11.0)).child(footer))
}
