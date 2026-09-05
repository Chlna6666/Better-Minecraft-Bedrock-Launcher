use std::time::Duration;

use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::state::{OnboardingAnchor, OnboardingScene, OnboardingTourState};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

const WELCOME_WIDE_WIDTH: f32 = 548.0;
const WELCOME_REGULAR_WIDTH: f32 = 492.0;
const WELCOME_TIGHT_WIDTH: f32 = 404.0;
const WELCOME_WIDE_HEIGHT: f32 = 520.0;
const WELCOME_REGULAR_HEIGHT: f32 = 492.0;
const WELCOME_TIGHT_HEIGHT: f32 = 456.0;

/// 统一的首次导览呈现入口。
///
/// 欢迎页使用更宽、更对称的专用布局；其余步骤继续复用 guided_overlay
/// 的真实页面 spotlight/演示数据逻辑，再在最外层补充可中断的场景淡入和
/// 真实锚点呼吸提示。这样视觉改造不会侵入各页面的业务状态。
pub fn render_onboarding_tour(
    state: &OnboardingTourState,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    if state.scene == OnboardingScene::Welcome {
        return render_welcome(state, window, cx);
    }

    let now = window.animation_time();
    let theme = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(now),
        theme.accent,
    );

    let overlay = super::guided_overlay::render_onboarding_tour(state, window, cx);
    let scene = state.scene;
    let mut root = div()
        .absolute()
        .inset_0()
        .child(overlay)
        .with_animation(
            scene_animation_id(scene),
            crate::ui::animation::ease_out_cubic_motion(Duration::from_millis(190)),
            |this, progress| this.opacity(0.55 + 0.45 * progress),
        )
        .into_any_element();

    if let Some(pulse) = render_anchor_pulse(state, &colors) {
        root = div()
            .absolute()
            .inset_0()
            .child(root)
            .child(pulse)
            .into_any_element();
    }

    root
}

fn render_welcome(state: &OnboardingTourState, window: &mut Window, cx: &App) -> AnyElement {
    let now = window.animation_time();
    let theme = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(now),
        theme.accent,
    );
    let size = window.bounds().size;
    let width = size.width / px(1.0);
    let height = size.height / px(1.0);
    let (card_w, card_h) = welcome_size(width, height);
    let i18n = cx.global::<I18n>().clone();

    let card = div()
        .w(px(card_w))
        .h(px(card_h))
        .max_w(relative(1.0))
        .max_h(relative(1.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.975,
            ..colors.bg
        })
        .shadow(vec![BoxShadow {
            color: Hsla { a: 0.22, ..black() },
            blur_radius: px(34.0),
            spread_radius: px(-6.0),
            offset: point(px(0.0), px(14.0)),
        }])
        .overflow_hidden()
        .occlude()
        .flex()
        .flex_col()
        .child(render_welcome_header(state, &colors, &i18n))
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .px(px(22.0))
                .py(px(14.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.0))
                        .line_height(px(19.0))
                        .text_color(colors.text_secondary)
                        .child(t!("Onboarding.welcome.intro")),
                )
                .child(animated_welcome_feature(
                    &colors,
                    0,
                    lucide_icons::icon_download(),
                    t!("Onboarding.welcome.get_game"),
                    t!("Onboarding.welcome.get_game_detail"),
                ))
                .child(animated_welcome_feature(
                    &colors,
                    1,
                    lucide_icons::icon_activity(),
                    t!("Onboarding.welcome.tasks"),
                    t!("Onboarding.welcome.tasks_detail"),
                ))
                .child(animated_welcome_feature(
                    &colors,
                    2,
                    lucide_icons::icon_settings_2(),
                    t!("Onboarding.welcome.manage"),
                    t!("Onboarding.welcome.manage_detail"),
                ))
                .child(
                    div()
                        .w_full()
                        .px(px(12.0))
                        .py(px(9.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.055,
                            ..colors.surface
                        })
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            svg()
                                .path(lucide_icons::icon_info())
                                .size(px(15.0))
                                .text_color(colors.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(10.5))
                                .line_height(px(16.0))
                                .text_color(colors.text_secondary)
                                .child(t!("Onboarding.welcome.demo_hint")),
                        ),
                ),
        )
        .child(render_welcome_footer(&colors, &i18n))
        .with_animation(
            "onboarding-welcome-card-enter",
            crate::ui::animation::spring_motion(crate::ui::animation::spring_snappy()),
            |this, progress| {
                this.opacity(progress)
                    .scale(0.965 + 0.035 * progress)
                    .mt(px((1.0 - progress) * 14.0))
            },
        );

    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(Hsla { a: 0.13, ..black() })
        .flex()
        .items_center()
        .justify_center()
        .p(px(18.0))
        .child(card)
        .with_animation(
            "onboarding-welcome-backdrop-enter",
            crate::ui::animation::ease_out_cubic_motion(Duration::from_millis(220)),
            |this, progress| this.opacity(progress),
        )
        .into_any_element()
}

fn welcome_size(width: f32, height: f32) -> (f32, f32) {
    let (ideal_w, ideal_h) = if width >= 1180.0 && height >= 680.0 {
        (WELCOME_WIDE_WIDTH, WELCOME_WIDE_HEIGHT)
    } else if width >= 760.0 && height >= 520.0 {
        (WELCOME_REGULAR_WIDTH, WELCOME_REGULAR_HEIGHT)
    } else {
        (WELCOME_TIGHT_WIDTH, WELCOME_TIGHT_HEIGHT)
    };

    let max_w = (width - 36.0).max(300.0);
    let max_h = (height - 36.0).max(330.0);
    (ideal_w.min(max_w), ideal_h.min(max_h))
}

fn render_welcome_header(state: &OnboardingTourState, colors: &ThemeColors, i18n: &I18n) -> Div {
    div()
        .w_full()
        .px(px(22.0))
        .pt(px(18.0))
        .pb(px(14.0))
        .border_b_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .flex_none()
                .size(px(46.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.12,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_route())
                        .size(px(21.0))
                        .text_color(colors.accent),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(t!("Onboarding.header.welcome")),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(17.0))
                        .text_color(colors.text_secondary)
                        .child(t!("Onboarding.header.welcome_detail")),
                ),
        )
        .child(
            div()
                .flex_none()
                .px(px(9.0))
                .py(px(4.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.accent
                })
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.accent)
                .child(format!(
                    "{} / {}",
                    state.scene.index(),
                    OnboardingScene::COUNT
                )),
        )
}

fn animated_welcome_feature(
    colors: &ThemeColors,
    index: usize,
    icon: &'static str,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> AnyElement {
    let title = title.into();
    let detail = detail.into();
    let row = div()
        .w_full()
        .min_h(px(68.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.065,
            ..colors.accent
        })
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(
            div()
                .flex_none()
                .size(px(38.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(17.0)).text_color(colors.accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(10.5))
                        .line_height(px(16.0))
                        .text_color(colors.text_secondary)
                        .child(detail),
                ),
        );

    let (id, delay) = match index {
        0 => ("onboarding-welcome-feature-0", 30),
        1 => ("onboarding-welcome-feature-1", 80),
        _ => ("onboarding-welcome-feature-2", 130),
    };

    row.with_animations(
        id,
        vec![
            Animation::new(Duration::from_millis(delay)),
            crate::ui::animation::ease_out_cubic_motion(Duration::from_millis(260)),
        ],
        |this, animation_index, progress| {
            if animation_index == 0 {
                this.opacity(0.0).ml(px(10.0))
            } else {
                this.opacity(progress).ml(px((1.0 - progress) * 10.0))
            }
        },
    )
    .into_any_element()
}

fn render_welcome_footer(colors: &ThemeColors, i18n: &I18n) -> Div {
    div()
        .w_full()
        .px(px(22.0))
        .py(px(12.0))
        .border_t_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.0))
        .child(
            div()
                .id("onboarding-welcome-skip")
                .h(px(40.0))
                .px(px(14.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_secondary)
                .hover(|this| {
                    this.bg(Hsla {
                        a: 0.08,
                        ..colors.surface
                    })
                })
                .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
                .child(t!("Onboarding.skip"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    crate::ui::onboarding::skip(cx);
                }),
        )
        .child(
            div()
                .id("onboarding-welcome-next")
                .h(px(40.0))
                .min_w(px(88.0))
                .px(px(18.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(colors.accent)
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(7.0))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.btn_primary_text)
                .hover(|this| this.bg(colors.accent_hover))
                .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
                .child(t!("Onboarding.next"))
                .child(
                    svg()
                        .path(lucide_icons::icon_arrow_right())
                        .size(px(15.0))
                        .text_color(colors.btn_primary_text),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    crate::ui::onboarding::advance(cx);
                }),
        )
}

fn render_anchor_pulse(state: &OnboardingTourState, colors: &ThemeColors) -> Option<AnyElement> {
    let (anchor, padding) = match state.scene {
        OnboardingScene::DownloadNavigation => (OnboardingAnchor::DownloadTabs, 8.0),
        OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => (OnboardingAnchor::DownloadToolbar, 7.0),
        OnboardingScene::ImportPackage => (OnboardingAnchor::DownloadImport, 9.0),
        OnboardingScene::TasksOverview => (OnboardingAnchor::TasksPage, 7.0),
        OnboardingScene::SettingsOverview | OnboardingScene::PlatformSetup => {
            (OnboardingAnchor::SettingsTabs, 8.0)
        }
        OnboardingScene::ToolsOverview => (OnboardingAnchor::ToolsSidebar, 8.0),
        _ => return None,
    };
    let bounds = state.anchor(anchor)?;
    let x = bounds.origin.x / px(1.0) - padding;
    let y = bounds.origin.y / px(1.0) - padding;
    let w = bounds.size.width / px(1.0) + padding * 2.0;
    let h = bounds.size.height / px(1.0) + padding * 2.0;

    Some(
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .rounded(px(crate::ui::theme::tokens::radius::MD))
            .border_2()
            .border_color(Hsla {
                a: 0.72,
                ..colors.accent
            })
            .with_animation(
                "onboarding-anchor-pulse",
                crate::ui::animation::repeating_linear_motion(Duration::from_millis(1500)),
                |this, progress| {
                    let wave = (progress * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                    this.opacity(0.45 + 0.45 * wave)
                },
            )
            .into_any_element(),
    )
}

fn scene_animation_id(scene: OnboardingScene) -> &'static str {
    match scene {
        OnboardingScene::Welcome => "onboarding-scene-welcome",
        OnboardingScene::DownloadNavigation => "onboarding-scene-download-navigation",
        OnboardingScene::GameDownload => "onboarding-scene-game-download",
        OnboardingScene::ResourcePackDownload => "onboarding-scene-resource-pack-download",
        OnboardingScene::ModDownload => "onboarding-scene-mod-download",
        OnboardingScene::ImportPackage => "onboarding-scene-import-package",
        OnboardingScene::TasksOverview => "onboarding-scene-tasks-overview",
        OnboardingScene::ManageOverview => "onboarding-scene-manage-overview",
        OnboardingScene::ManageContent => "onboarding-scene-manage-content",
        OnboardingScene::SettingsOverview => "onboarding-scene-settings-overview",
        OnboardingScene::ToolsOverview => "onboarding-scene-tools-overview",
        OnboardingScene::PlatformSetup => "onboarding-scene-platform-setup",
        OnboardingScene::Finish => "onboarding-scene-finish",
    }
}