use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use gpui::{AnimationExt as _, *};
use std::time::Duration;

const LOADING_SHIMMER_DURATION: Duration = Duration::from_millis(1250);
const LOADING_ROW_HEIGHT: f32 = 76.0;
const LOADING_ROW_SEPARATOR_HEIGHT: f32 = 1.0;
const LOADING_MIN_ROWS: usize = 4;
const LOADING_MAX_ROWS: usize = 8;
const LOADING_BLOCK_PHASE: f32 = 0.055;
const SHIMMER_START: f32 = -0.45;
const SHIMMER_END: f32 = 1.12;
const SHIMMER_WIDTH: f32 = 0.34;

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

fn shimmer_phase(block: usize) -> f32 {
    // 只按横向块位置错开相位。所有列表行使用完全相同的相位，
    // 因此动画不会再形成从上到下的波动。
    (block as f32 * LOADING_BLOCK_PHASE).fract()
}

fn shimmer_animation(phase_offset: f32) -> Animation {
    Animation::new(LOADING_SHIMMER_DURATION)
        .repeat()
        .with_easing(move |t| (t + phase_offset).fract())
}

fn animated_block(
    id: SharedString,
    width: Pixels,
    height: Pixels,
    radius: Pixels,
    base_color: Hsla,
    base_alpha: f32,
    highlight_color: Hsla,
    highlight_alpha: f32,
    phase_offset: f32,
) -> AnyElement {
    let highlight = div()
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(relative(SHIMMER_START))
        .w(relative(SHIMMER_WIDTH))
        .rounded(radius)
        .bg(Hsla {
            a: highlight_alpha,
            ..highlight_color
        })
        .with_animation(id, shimmer_animation(phase_offset), |this, t| {
            let left = SHIMMER_START + (SHIMMER_END - SHIMMER_START) * t;
            this.left(relative(left))
        });

    div()
        .w(width)
        .h(height)
        .rounded(radius)
        .bg(Hsla {
            a: base_alpha,
            ..base_color
        })
        .relative()
        .overflow_hidden()
        .child(highlight)
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
        0.075,
        colors.text_primary,
        0.18,
        shimmer_phase(block),
    )
}

fn skeleton_row(colors: &ThemeColors, row: usize) -> Div {
    let image = animated_block(
        SharedString::from(format!("download-loading-{row}-image")),
        px(42.0),
        px(42.0),
        px(crate::ui::theme::tokens::radius::SM),
        colors.text_secondary,
        0.085,
        colors.text_primary,
        0.18,
        shimmer_phase(0),
    );
    let title = animated_bar(colors, row, 1, px(240.0), px(15.0));
    let tag = animated_block(
        SharedString::from(format!("download-loading-{row}-tag")),
        px(72.0),
        px(20.0),
        px(crate::ui::theme::tokens::radius::SM),
        colors.text_secondary,
        0.065,
        colors.text_primary,
        0.16,
        shimmer_phase(2),
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
        0.10,
        colors.text_primary,
        0.16,
        shimmer_phase(6),
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

    let footer = animated_block(
        SharedString::from("download-loading-footer"),
        px(1080.0),
        px(32.0),
        px(crate::ui::theme::tokens::radius::MD),
        colors.text_secondary,
        0.055,
        colors.text_primary,
        0.16,
        shimmer_phase(0),
    );

    // 每个实际占位块内部都有独立的横向高亮带：从块左侧之外进入，
    // 横穿占位块后从右侧离开。列表行之间没有纵向相位差，视觉方向
    // 始终是左 -> 右；动画只改变高亮带位置，不改变占位块尺寸。
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
        .child(
            div()
                .w_full()
                .px(px(14.0))
                .py(px(11.0))
                .overflow_hidden()
                .child(footer),
        )
}
