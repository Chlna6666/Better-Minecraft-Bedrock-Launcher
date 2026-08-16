use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use gpui::prelude::FluentBuilder as _;
use gpui::{AnimationExt as _, *};
use std::time::Duration;

const LOADING_PULSE_DURATION: Duration = Duration::from_millis(900);
const LOADING_ROW_HEIGHT: f32 = 76.0;
const LOADING_ROW_SEPARATOR_HEIGHT: f32 = 1.0;
const LOADING_MIN_ROWS: usize = 4;
const LOADING_MAX_ROWS: usize = 8;

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

fn skeleton_fill(colors: &ThemeColors, alpha: f32) -> Hsla {
    Hsla {
        a: alpha,
        ..colors.text_secondary
    }
}

fn skeleton_bar(colors: &ThemeColors, width: Pixels, height: Pixels, alpha: f32) -> Div {
    div()
        .w(width)
        .h(height)
        .rounded(px(crate::ui::theme::tokens::radius::FULL))
        .bg(skeleton_fill(colors, alpha))
}

fn skeleton_row(colors: &ThemeColors) -> Div {
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
                        .child(
                            div()
                                .w(px(42.0))
                                .h(px(42.0))
                                .rounded(px(crate::ui::theme::tokens::radius::SM))
                                .bg(skeleton_fill(colors, 0.09)),
                        ),
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
                                .child(skeleton_bar(colors, px(240.0), px(15.0), 0.11))
                                .child(
                                    div()
                                        .w(px(72.0))
                                        .h(px(20.0))
                                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                                        .bg(skeleton_fill(colors, 0.075)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(skeleton_bar(colors, px(68.0), px(10.0), 0.085))
                                .child(skeleton_bar(colors, px(96.0), px(10.0), 0.085)),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(skeleton_bar(colors, px(78.0), px(14.0), 0.085))
                .child(
                    div()
                        .w(px(88.0))
                        .h(px(28.0))
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(Hsla {
                            a: 0.11,
                            ..colors.accent
                        }),
                ),
        )
}

fn skeleton_row_with_separator(colors: &ThemeColors) -> Div {
    div()
        .flex()
        .flex_col()
        .child(skeleton_row(colors))
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
        .map(|_| skeleton_row_with_separator(colors).into_any_element())
        .collect::<Vec<_>>();

    let skeleton_content = div()
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
                .px(px(14.0))
                .py(px(11.0))
                .child(
                    div()
                        .w_full()
                        .h(px(32.0))
                        .rounded(px(crate::ui::theme::tokens::radius::MD))
                        .bg(skeleton_fill(colors, 0.065)),
                ),
        );

    // 统一加载层只声明一个场景级透明度动画。Opacity 不改变布局，
    // GPUI/Nova 可以直接绑定场景动画，避免旧 shimmer 每帧修改 left
    // 导致整组骨架重新布局，同时 Alternate 保证循环端点连续、无跳变。
    let pulse = Animation::from_spec(
        AnimationSpec::new(LOADING_PULSE_DURATION)
            .ease(Easing::InOutCubic)
            .direction(AnimationDirection::Alternate)
            .repeat(RepeatMode::Forever),
    )
    .with_property(AnimationProperty::opacity(0.76, 0.96));

    div()
        .size_full()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .child(skeleton_content)
        .with_animation("download-loading-pulse", pulse, |this, _| this)
}
