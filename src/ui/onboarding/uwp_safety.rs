#![cfg(target_os = "windows")]

use gpui::*;
use lucide_gpui::icons as lucide_icons;

use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UwpSafetyGuideTrigger {
    #[default]
    Download,
    Import,
}

pub struct UwpSafetyGuideState {
    pub visible: bool,
    pub trigger: Option<UwpSafetyGuideTrigger>,
    acknowledged: bool,
}

impl Global for UwpSafetyGuideState {}

impl Default for UwpSafetyGuideState {
    fn default() -> Self {
        let acknowledged = crate::config::uwp_safety::is_current_guide_acknowledged();
        Self {
            visible: false,
            trigger: None,
            acknowledged,
        }
    }
}

impl UwpSafetyGuideState {
    fn request(&mut self, trigger: UwpSafetyGuideTrigger, defer_until_tour_finishes: bool) {
        if self.acknowledged {
            return;
        }
        self.trigger = Some(trigger);
        self.visible = !defer_until_tour_finishes;
    }

    fn activate_pending(&mut self) {
        if !self.acknowledged && self.trigger.is_some() {
            self.visible = true;
        }
    }

    fn acknowledge(&mut self) {
        self.visible = false;
        self.trigger = None;
        self.acknowledged = true;
    }
}

pub fn request(trigger: UwpSafetyGuideTrigger, cx: &mut App) {
    cx.default_global::<UwpSafetyGuideState>();
    let tour_visible = cx
        .try_global::<super::state::OnboardingTourState>()
        .is_some_and(|tour| tour.visible);
    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.request(trigger, tour_visible);
    });
}

pub fn activate_pending(cx: &mut App) {
    cx.default_global::<UwpSafetyGuideState>();
    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| {
        state.activate_pending();
    });
}

fn acknowledge(cx: &mut App) {
    cx.update_global(|state: &mut UwpSafetyGuideState, _cx| state.acknowledge());
    if let Err(error) = crate::tasks::runtime::spawn_io(async {
        if let Err(error) = crate::config::uwp_safety::acknowledge_current_guide() {
            tracing::error!(%error, "persist UWP safety guide acknowledgement failed");
        }
    }) {
        tracing::error!(%error, "failed to schedule UWP safety guide acknowledgement persistence");
    }
}

pub fn render_uwp_safety_guide(
    state: &UwpSafetyGuideState,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    );
    let size = window.bounds().size;
    let width = size.width / px(1.0);
    let height = size.height / px(1.0);
    let card_w = (width - 28.0).clamp(340.0, 520.0);
    let card_h = (height - 40.0).clamp(390.0, 470.0);
    let trigger = state.trigger.unwrap_or_default();

    let context = match trigger {
        UwpSafetyGuideTrigger::Download => {
            "你正在处理 Windows UWP 版本。下载本身不会卸载 Microsoft Store 版本；只有后续真正切换 UWP 注册时才会进入数据保护流程。"
        }
        UwpSafetyGuideTrigger::Import => {
            "你正在导入 APPX / ZIP UWP 版本。导入只会把版本保存到 BMCBL；只有后续真正切换 UWP 注册时才会进入数据保护流程。"
        }
    };

    let card = div()
        .w(px(card_w))
        .h(px(card_h))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .overflow_hidden()
        .occlude()
        .bg(Hsla {
            a: 0.985,
            ..colors.bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.22,
                ..black()
            },
            blur_radius: px(32.0),
            spread_radius: px(-5.0),
            offset: point(px(0.0), px(14.0)),
        }])
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(20.0))
                .pt(px(18.0))
                .pb(px(14.0))
                .border_b_1()
                .border_color(Hsla {
                    a: 0.18,
                    ..colors.border
                })
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_none()
                        .size(px(42.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.13,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(lucide_icons::icon_shield_check())
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
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(17.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child("UWP 数据保护说明"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(colors.text_secondary)
                                .child("仅在首次下载或导入 UWP 时提示，与主功能导览分离。"),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .p(px(20.0))
                .flex()
                .flex_col()
                .gap(px(11.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(19.0))
                        .text_color(colors.text_secondary)
                        .child(context),
                )
                .child(safety_step(
                    &colors,
                    1,
                    "先判断当前 UWP 注册来源",
                    "BMCBL 会区分自己管理的散装 DevelopmentMode 注册与 Microsoft Store / 外部注册。",
                ))
                .child(safety_step(
                    &colors,
                    2,
                    "Store / 外部数据先备份再替换",
                    "如果检测到 games/com.mojang 数据，会先备份并校验；备份失败就停止卸载和替换注册。",
                ))
                .child(safety_step(
                    &colors,
                    3,
                    "注册成功后再恢复",
                    "目标数据目录为空时才自动恢复备份；BMCBL 自己管理的散装 UWP 切换则优先保留应用数据。",
                ))
                .child(
                    div()
                        .px(px(11.0))
                        .py(px(8.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(Hsla {
                            a: 0.08,
                            ..colors.accent
                        })
                        .text_size(px(11.0))
                        .line_height(px(17.0))
                        .text_color(colors.text_secondary)
                        .child("GDK / MSIXVC 不使用这套 UWP 注册迁移流程，因此不会显示本说明。"),
                ),
        )
        .child(
            div()
                .px(px(20.0))
                .py(px(14.0))
                .border_t_1()
                .border_color(Hsla {
                    a: 0.18,
                    ..colors.border
                })
                .flex()
                .justify_end()
                .child(
                    div()
                        .id("uwp-safety-guide-acknowledge")
                        .h(px(38.0))
                        .px(px(18.0))
                        .rounded(px(crate::ui::theme::tokens::radius::SM))
                        .bg(colors.accent)
                        .cursor_pointer()
                        .hover(|this| this.bg(colors.accent_hover))
                        .active(|this| {
                            this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE)
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.btn_primary_text)
                        .child("我已了解，继续")
                        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                            acknowledge(cx);
                        }),
                ),
        );

    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(Hsla {
            a: 0.24,
            ..black()
        })
        .flex()
        .items_center()
        .justify_center()
        .child(card)
}

fn safety_step(
    colors: &ThemeColors,
    number: u8,
    title: &'static str,
    detail: &'static str,
) -> impl IntoElement {
    div()
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(
            div()
                .flex_none()
                .size(px(26.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.12,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(17.0))
                        .text_color(colors.text_secondary)
                        .child(detail),
                ),
        )
}
