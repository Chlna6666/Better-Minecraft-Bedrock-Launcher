use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use gpui::{AnimationExt as _, *};
use std::f32::consts::TAU;
use std::time::Duration;

const GAME_SHIMMER_DURATION: Duration = Duration::from_millis(1200);
const RESOURCE_SWEEP_DURATION: Duration = Duration::from_millis(1550);
const MOD_PULSE_DURATION: Duration = Duration::from_millis(1050);

const GAME_ROW_HEIGHT: f32 = 76.0;
const RESOURCE_ROW_HEIGHT: f32 = 84.0;
const MOD_CARD_HEIGHT: f32 = 160.0;

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

fn visible_count(viewport_height: Pixels, pitch: f32, min: usize, max: usize) -> usize {
    let height = (viewport_height / px(1.0)).max(pitch);
    ((height / pitch).ceil() as usize).clamp(min, max)
}

fn static_block(
    width: Pixels,
    height: Pixels,
    radius: Pixels,
    color: Hsla,
    alpha: f32,
) -> Div {
    div()
        .w(width)
        .h(height)
        .rounded(radius)
        .bg(Hsla { a: alpha, ..color })
}

fn game_shimmer_block(
    id: SharedString,
    width: Pixels,
    height: Pixels,
    radius: Pixels,
    colors: &ThemeColors,
    accent: bool,
    phase: f32,
) -> AnyElement {
    let base_color = if accent { colors.accent } else { colors.text_secondary };
    let start = -0.42f32;
    let end = 1.10f32;
    let band = div()
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(relative(start))
        .w(relative(0.32))
        .rounded(radius)
        .bg(Hsla {
            a: if accent { 0.14 } else { 0.19 },
            ..colors.text_primary
        })
        .with_animation(
            id,
            Animation::new(GAME_SHIMMER_DURATION)
                .repeat()
                .with_easing(move |t| (t + phase).fract()),
            move |this, t| this.left(relative(start + (end - start) * t)),
        );

    div()
        .w(width)
        .h(height)
        .rounded(radius)
        .bg(Hsla {
            a: if accent { 0.10 } else { 0.075 },
            ..base_color
        })
        .relative()
        .overflow_hidden()
        .child(band)
        .into_any_element()
}

fn resource_sweep(id: SharedString, colors: &ThemeColors) -> AnyElement {
    let start = -0.28f32;
    let end = 1.06f32;
    div()
        .absolute()
        .top(px(0.0))
        .bottom(px(0.0))
        .left(relative(start))
        .w(relative(0.24))
        .bg(Hsla {
            a: 0.075,
            ..colors.text_primary
        })
        .with_animation(
            id,
            Animation::new(RESOURCE_SWEEP_DURATION).repeat(),
            move |this, t| this.left(relative(start + (end - start) * t)),
        )
        .into_any_element()
}

fn mod_pulse_block(
    id: SharedString,
    width: Pixels,
    height: Pixels,
    radius: Pixels,
    color: Hsla,
    min_alpha: f32,
    max_alpha: f32,
    phase: f32,
) -> AnyElement {
    div()
        .w(width)
        .h(height)
        .rounded(radius)
        .bg(Hsla { a: min_alpha, ..color })
        .with_animation(
            id,
            Animation::new(MOD_PULSE_DURATION)
                .repeat()
                .with_easing(move |t| {
                    let p = (t + phase).fract();
                    (0.5 - 0.5 * (TAU * p).cos()).clamp(0.0, 1.0)
                }),
            move |this, t| {
                this.bg(Hsla {
                    a: min_alpha + (max_alpha - min_alpha) * t,
                    ..color
                })
            },
        )
        .into_any_element()
}

fn render_game_loading(colors: &ThemeColors, viewport_height: Pixels) -> Div {
    let row_count = visible_count(viewport_height, GAME_ROW_HEIGHT + 1.0, 4, 8);
    let rows = (0..row_count)
        .map(|row| {
            let id = |name: &str| SharedString::from(format!("game-loading-{row}-{name}"));
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .h(px(GAME_ROW_HEIGHT))
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
                                    div().w(px(64.0)).child(game_shimmer_block(
                                        id("icon"),
                                        px(42.0),
                                        px(42.0),
                                        px(crate::ui::theme::tokens::radius::SM),
                                        colors,
                                        false,
                                        0.00,
                                    )),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .pr(px(16.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(10.0))
                                                .child(game_shimmer_block(
                                                    id("title"),
                                                    px(240.0),
                                                    px(15.0),
                                                    px(crate::ui::theme::tokens::radius::FULL),
                                                    colors,
                                                    false,
                                                    0.04,
                                                ))
                                                .child(game_shimmer_block(
                                                    id("badge"),
                                                    px(72.0),
                                                    px(20.0),
                                                    px(crate::ui::theme::tokens::radius::SM),
                                                    colors,
                                                    false,
                                                    0.08,
                                                )),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(game_shimmer_block(
                                                    id("meta-a"),
                                                    px(68.0),
                                                    px(10.0),
                                                    px(crate::ui::theme::tokens::radius::FULL),
                                                    colors,
                                                    false,
                                                    0.12,
                                                ))
                                                .child(game_shimmer_block(
                                                    id("meta-b"),
                                                    px(96.0),
                                                    px(10.0),
                                                    px(crate::ui::theme::tokens::radius::FULL),
                                                    colors,
                                                    false,
                                                    0.16,
                                                )),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(game_shimmer_block(
                                    id("status"),
                                    px(78.0),
                                    px(14.0),
                                    px(crate::ui::theme::tokens::radius::FULL),
                                    colors,
                                    false,
                                    0.20,
                                ))
                                .child(game_shimmer_block(
                                    id("action"),
                                    px(88.0),
                                    px(28.0),
                                    px(crate::ui::theme::tokens::radius::FULL),
                                    colors,
                                    true,
                                    0.24,
                                )),
                        ),
                )
                .child(div().h(px(1.0)).bg(Hsla {
                    a: 0.055,
                    ..colors.border
                }))
                .into_any_element()
        })
        .collect::<Vec<_>>();

    div()
        .size_full()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .overflow_hidden()
        .flex()
        .flex_col()
        .children(rows)
}

fn render_resource_loading(colors: &ThemeColors, viewport_height: Pixels) -> Div {
    let row_count = visible_count(viewport_height, RESOURCE_ROW_HEIGHT, 3, 8);
    let rows = (0..row_count)
        .map(|row| {
            div()
                .w_full()
                .h(px(78.0))
                .min_h(px(78.0))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .border_1()
                .border_color(Hsla {
                    a: 0.08,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.20,
                    ..colors.surface
                })
                .px(px(12.0))
                .py(px(9.0))
                .relative()
                .overflow_hidden()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(static_block(
                    px(42.0),
                    px(42.0),
                    px(crate::ui::theme::tokens::radius::SM),
                    colors.text_secondary,
                    0.09,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(static_block(
                            px(220.0),
                            px(13.0),
                            px(crate::ui::theme::tokens::radius::FULL),
                            colors.text_secondary,
                            0.10,
                        ))
                        .child(static_block(
                            px(360.0),
                            px(11.0),
                            px(crate::ui::theme::tokens::radius::FULL),
                            colors.text_secondary,
                            0.075,
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(static_block(
                                    px(120.0),
                                    px(10.0),
                                    px(crate::ui::theme::tokens::radius::FULL),
                                    colors.text_secondary,
                                    0.075,
                                ))
                                .child(static_block(
                                    px(80.0),
                                    px(10.0),
                                    px(crate::ui::theme::tokens::radius::FULL),
                                    colors.text_secondary,
                                    0.075,
                                ))
                                .child(static_block(
                                    px(92.0),
                                    px(10.0),
                                    px(crate::ui::theme::tokens::radius::FULL),
                                    colors.text_secondary,
                                    0.075,
                                )),
                        ),
                )
                .child(static_block(
                    px(92.0),
                    px(30.0),
                    px(crate::ui::theme::tokens::radius::SM),
                    colors.accent,
                    0.10,
                ))
                .child(resource_sweep(
                    SharedString::from(format!("resource-loading-sweep-{row}")),
                    colors,
                ))
                .into_any_element()
        })
        .collect::<Vec<_>>();

    div()
        .size_full()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .overflow_hidden()
        .px(px(12.0))
        .py(px(12.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .children(rows)
}

fn render_mod_loading(colors: &ThemeColors, viewport_height: Pixels) -> Div {
    let rows = visible_count(viewport_height, MOD_CARD_HEIGHT + 16.0, 2, 4);
    let card_count = (rows * 3).clamp(6, 12);
    let cards = (0..card_count)
        .map(|card| {
            let phase = (card % 3) as f32 * 0.08 + (card / 3) as f32 * 0.025;
            let id = |name: &str| SharedString::from(format!("mod-loading-{card}-{name}"));
            div()
                .w(px(320.0))
                .flex_grow()
                .min_h(px(MOD_CARD_HEIGHT))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .border_1()
                .border_color(Hsla {
                    a: 0.18,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.45,
                    ..colors.surface
                })
                .p(px(14.0))
                .flex()
                .flex_col()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap(px(12.0))
                                .child(mod_pulse_block(
                                    id("avatar"),
                                    px(48.0),
                                    px(48.0),
                                    px(crate::ui::theme::tokens::radius::SM),
                                    colors.accent,
                                    0.05,
                                    0.16,
                                    phase,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(5.0))
                                        .child(mod_pulse_block(
                                            id("name"),
                                            px(168.0),
                                            px(14.0),
                                            px(crate::ui::theme::tokens::radius::FULL),
                                            colors.text_secondary,
                                            0.05,
                                            0.17,
                                            phase + 0.06,
                                        ))
                                        .child(mod_pulse_block(
                                            id("package"),
                                            px(118.0),
                                            px(10.0),
                                            px(crate::ui::theme::tokens::radius::FULL),
                                            colors.text_secondary,
                                            0.04,
                                            0.13,
                                            phase + 0.12,
                                        )),
                                ),
                        )
                        .child(mod_pulse_block(
                            id("desc-a"),
                            px(250.0),
                            px(10.0),
                            px(crate::ui::theme::tokens::radius::FULL),
                            colors.text_secondary,
                            0.04,
                            0.12,
                            phase + 0.18,
                        ))
                        .child(mod_pulse_block(
                            id("desc-b"),
                            px(205.0),
                            px(10.0),
                            px(crate::ui::theme::tokens::radius::FULL),
                            colors.text_secondary,
                            0.04,
                            0.12,
                            phase + 0.22,
                        )),
                )
                .child(
                    div()
                        .pt(px(8.0))
                        .border_t_1()
                        .border_color(Hsla {
                            a: 0.06,
                            ..colors.border
                        })
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(mod_pulse_block(
                                    id("version"),
                                    px(54.0),
                                    px(18.0),
                                    px(crate::ui::theme::tokens::radius::XS),
                                    colors.accent,
                                    0.05,
                                    0.14,
                                    phase + 0.28,
                                ))
                                .child(mod_pulse_block(
                                    id("stars"),
                                    px(42.0),
                                    px(10.0),
                                    px(crate::ui::theme::tokens::radius::FULL),
                                    colors.text_secondary,
                                    0.04,
                                    0.12,
                                    phase + 0.32,
                                )),
                        )
                        .child(mod_pulse_block(
                            id("detail"),
                            px(68.0),
                            px(28.0),
                            px(crate::ui::theme::tokens::radius::SM),
                            colors.accent,
                            0.05,
                            0.15,
                            phase + 0.36,
                        )),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

    div()
        .size_full()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .px(px(20.0))
                .py(px(10.0))
                .border_b_1()
                .border_color(Hsla {
                    a: 0.08,
                    ..colors.border
                })
                .flex()
                .items_center()
                .justify_between()
                .child(static_block(
                    px(180.0),
                    px(18.0),
                    px(crate::ui::theme::tokens::radius::SM),
                    colors.text_secondary,
                    0.06,
                ))
                .child(static_block(
                    px(230.0),
                    px(10.0),
                    px(crate::ui::theme::tokens::radius::FULL),
                    colors.text_secondary,
                    0.045,
                )),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .overflow_hidden()
                .p(px(20.0))
                .flex()
                .flex_wrap()
                .gap(px(16.0))
                .items_stretch()
                .children(cards),
        )
}

#[derive(IntoElement)]
struct DownloadLoadingPlaceholder {
    colors: ThemeColors,
    viewport_height: Pixels,
}

impl RenderOnce for DownloadLoadingPlaceholder {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        match cx.global::<DownloadPageState>().tab {
            DownloadTab::Game => render_game_loading(&self.colors, self.viewport_height),
            DownloadTab::ResourcePack => render_resource_loading(&self.colors, self.viewport_height),
            DownloadTab::Mod => render_mod_loading(&self.colors, self.viewport_height),
        }
    }
}

pub(super) fn render_loading_placeholder(
    colors: &ThemeColors,
    viewport_height: Pixels,
) -> impl IntoElement {
    DownloadLoadingPlaceholder {
        colors: colors.clone(),
        viewport_height,
    }
}
