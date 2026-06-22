use crate::music::{MusicDragTarget, MusicSnapshot};
use crate::ui::navigation::{self, AppRoute, RouteTarget};
use crate::ui::state::quit::QuitState;
use crate::ui::state::theme::ThemeState;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::{DarkColors, LightColors, lerp_theme_colors};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::time::Duration;
use std::time::Instant;

fn icon_path(path: &'static str) -> Svg {
    svg().path(path)
}

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_color(a: impl Into<Hsla>, b: impl Into<Hsla>, t: f32) -> Hsla {
    let a: Hsla = a.into();
    let b: Hsla = b.into();
    Hsla {
        h: lerp_f32(a.h, b.h, t),
        s: lerp_f32(a.s, b.s, t),
        l: lerp_f32(a.l, b.l, t),
        a: lerp_f32(a.a, b.a, t),
    }
}

#[derive(Clone)]
struct NavItem {
    icon_path: &'static str,
    image_icon_path: Option<std::path::PathBuf>,
    label: SharedString,
    target: RouteTarget,
}

pub struct AppChromeState {
    pub titlebar_gesture: crate::ui::window::chrome::TitlebarGestureState,
    music_inline_from: f32,
    music_inline_to: f32,
    music_inline_started_at: Option<Instant>,
    music_inline_duration: Duration,
    music_inline_target_expanded: bool,
}

impl Global for AppChromeState {}

impl Default for AppChromeState {
    fn default() -> Self {
        Self {
            titlebar_gesture: crate::ui::window::chrome::TitlebarGestureState::default(),
            music_inline_from: 0.0,
            music_inline_to: 0.0,
            music_inline_started_at: None,
            music_inline_duration: Duration::from_millis(180),
            music_inline_target_expanded: false,
        }
    }
}

impl AppChromeState {
    fn ease_out_back(t: f32, overshoot: f32) -> f32 {
        let p = t - 1.0;
        1.0 + (overshoot + 1.0) * p.powi(3) + overshoot * p.powi(2)
    }

    fn ease_in_back(t: f32, overshoot: f32) -> f32 {
        t * t * ((overshoot + 1.0) * t - overshoot)
    }

    pub fn set_music_inline_expanded(&mut self, expanded: bool, now: Instant) {
        // 目标态未变化时直接返回，避免重复重启动画。
        if self.music_inline_target_expanded == expanded {
            return;
        }

        self.music_inline_target_expanded = expanded;
        self.music_inline_from = self.music_inline_factor(now);
        self.music_inline_to = if expanded { 1.0 } else { 0.0 };
        self.music_inline_started_at = Some(now);
        self.music_inline_duration = if expanded {
            Duration::from_millis(280)
        } else {
            Duration::from_millis(220)
        };
    }

    pub fn music_inline_factor(&self, now: Instant) -> f32 {
        let Some(started_at) = self.music_inline_started_at else {
            return if self.music_inline_target_expanded {
                1.0
            } else {
                0.0
            };
        };

        let elapsed = now.saturating_duration_since(started_at);
        let duration = self.music_inline_duration.max(Duration::from_millis(1));
        let t = (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0);
        // 展开与收起都增加回弹感，收起更快一些以降低“慢触发”体感。
        let eased = if self.music_inline_to > self.music_inline_from {
            Self::ease_out_back(t, 0.48).clamp(0.0, 1.12)
        } else {
            // 先轻微反向再收回，形成“回收回弹”观感。
            (1.0 - Self::ease_in_back(1.0 - t, 0.34)).clamp(-0.08, 1.0)
        };
        (self.music_inline_from + (self.music_inline_to - self.music_inline_from) * eased)
            .clamp(-0.06, 1.12)
    }

    pub fn music_inline_animating(&self, now: Instant) -> bool {
        self.music_inline_started_at.is_some_and(|started_at| {
            now.saturating_duration_since(started_at) < self.music_inline_duration
        })
    }

    pub fn music_inline_target_expanded(&self) -> bool {
        self.music_inline_target_expanded
    }
}

pub fn render_app_chrome(
    app_version: SharedString,
    active_index: usize,
    pill_steps: f32,
    pill_direction: f32,
    pill_leading_progress: f32,
    pill_trailing_progress: f32,
    labels_layout_factor: f32,
    labels_opacity_factor: f32,
    music_snapshot: MusicSnapshot,
    music_expanded_factor: f32,
    music_progress_ratio: f32,
    music_volume_ratio: f32,
    music_drag_target: Option<MusicDragTarget>,
    music_inline_factor: f32,
    route: RouteTarget,
    window_width: Pixels,
    theme_k: f32,
    target_dark: bool,
    update_available: bool,
    _update_modal_open: bool,
    accent_override: Option<Hsla>,
    glass_effect_enabled: bool,
    plugin_pages: Vec<crate::plugins::runtime::PluginPage>,
) -> AnyElement {
    let topbar_top = px(6.);
    let topbar_h = px(60.);
    let topbar_radius = px(30.);

    let ww = window_width / px(1.);
    let inset_x = px((ww * 0.03).clamp(16.0, 28.0)); // width: 94%
    let layout_k = labels_layout_factor.clamp(0.0, 1.0);
    let label_k = labels_opacity_factor.clamp(0.0, 1.0);

    // 对齐网页端：常规横向内边距 24px，窄窗口降到约 16px。
    let nav_pad_x = if ww <= 1000.0 { px(16.) } else { px(24.) };
    let inner_w = (window_width - inset_x * 2.0 - nav_pad_x * 2.0).max(px(320.));
    let left_min_w = px(180.);

    // 使用统一主题颜色系统
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme_k.clamp(0.0, 1.0),
        accent_override,
    );

    let theme_k = theme_k.clamp(0.0, 1.0);
    let nav_bg = Hsla {
        a: if theme_k < 0.5 { 0.75 } else { 0.60 },
        ..colors.surface
    };
    let text_color = colors.text_primary;
    let border_color = colors.border;
    let icon_hover_bg = colors.surface_hover;
    let capsule_shell_bg = if theme_k < 0.5 {
        Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 0.04,
        }
    } else {
        Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 0.20,
        }
    };
    let capsule_shell_border = border_color.opacity(if theme_k < 0.5 { 0.05 } else { 0.08 });

    let accent = colors.accent;
    let tab_pill_bg = accent;
    let mut tab_pill_border = tab_pill_bg;
    tab_pill_border.a = 0.18;
    let music_available = music_snapshot.available;
    let music_inline_w = crate::ui::main_window::music_player::mini_capsule_width_for_factor(
        music_available,
        music_inline_factor,
    );
    let right_btn_w = px(40.);
    let right_gap = px(8.);
    let right_divider_w = px(1.) + px(8.) * 2.0;
    let slot_outer_gap = px(12.0);
    let music_capsule_gap = px(12.0);
    let right_static_controls_w = right_btn_w * 3.0 + right_divider_w + right_gap * 4.0;
    let right_controls_hit_w = right_static_controls_w
        + if music_available {
            music_capsule_gap + music_inline_w
        } else {
            px(0.0)
        };

    // 导航图标与网页端保持一致（在 gpui-component 可用图标集范围内）。
    let mut nav_items = vec![
        NavItem {
            icon_path: lucide_icons::icon_house(),
            image_icon_path: None,
            label: SharedString::from("启动"),
            target: RouteTarget::Builtin(AppRoute::Home),
        },
        NavItem {
            icon_path: lucide_icons::icon_download(),
            image_icon_path: None,
            label: SharedString::from("下载"),
            target: RouteTarget::Builtin(AppRoute::Download),
        },
        NavItem {
            icon_path: lucide_icons::icon_list(),
            image_icon_path: None,
            label: SharedString::from("版本"),
            target: RouteTarget::Builtin(AppRoute::Manage),
        },
        NavItem {
            icon_path: lucide_icons::icon_wrench(),
            image_icon_path: None,
            label: SharedString::from("工具"),
            target: RouteTarget::Builtin(AppRoute::Tools),
        },
        NavItem {
            icon_path: lucide_icons::icon_activity(),
            image_icon_path: None,
            label: SharedString::from("任务"),
            target: RouteTarget::Builtin(AppRoute::Tasks),
        },
        NavItem {
            icon_path: lucide_icons::icon_settings(),
            image_icon_path: None,
            label: SharedString::from("设置"),
            target: RouteTarget::Builtin(AppRoute::Settings),
        },
    ];
    nav_items.extend(plugin_pages.into_iter().map(|page| NavItem {
        icon_path: lucide_icons::icon_plug(),
        image_icon_path: page.icon_path,
        label: page.navigation.as_ref().map_or_else(
            || page.title.clone(),
            |navigation| SharedString::from(navigation.label.clone()),
        ),
        target: RouteTarget::Plugin {
            plugin_id: page.plugin_id,
            page_id: page.page_id,
        },
    }));

    // 采用固定宽度 tab，保证 active pill 的几何计算稳定，动画不会抖动。
    // 图标尺寸参考 WebView2（CSS 约为 20px）。
    let link_pad_x = if ww <= 1000.0 { px(12.0) } else { px(16.0) };
    let icon_w = px(18.0);
    let expanded_label_w = px(33.0);
    let expanded_label_gap = px(8.0);
    // WebView2 使用文本自然宽度；这里对中文短标签使用固定桶宽，
    // 以保持胶囊视觉对称，并让 pill 轨迹计算更稳定。
    let mut label_w = expanded_label_w * layout_k;
    let mut label_gap = expanded_label_gap * layout_k;
    let collapsed_item_w = link_pad_x * 2.0 + icon_w;
    let expanded_item_w = collapsed_item_w + expanded_label_gap + expanded_label_w;
    let mut item_w = collapsed_item_w + label_gap + label_w;
    // 规格要求：胶囊总高 47px、tab 色块高 35px、图标 18x18。
    let item_h = px(35.0);
    let mut capsule_gap = px(3.0);
    let capsule_pad_x = px(6.0);
    let capsule_pad_y = px(6.0);
    let tab_radius = px(18.0);
    let nav_len = nav_items.len();
    let active_index = active_index.min(nav_len.saturating_sub(1));

    let collapsed_capsule_w = capsule_pad_x * 2.0
        + collapsed_item_w * (nav_len as f32)
        + capsule_gap * (nav_len.saturating_sub(1) as f32);
    let expanded_capsule_w = capsule_pad_x * 2.0
        + expanded_item_w * (nav_len as f32)
        + capsule_gap * (nav_len.saturating_sub(1) as f32);
    let max_side_slot_w =
        ((inner_w - collapsed_capsule_w - slot_outer_gap * 2.0) / 2.0).max(left_min_w);
    let right_controls_w = (right_static_controls_w
        + if music_available {
            music_capsule_gap + music_inline_w
        } else {
            px(0.0)
        })
    .min(max_side_slot_w);
    let left_content_w = if update_available {
        px(168.0)
    } else {
        px(124.0)
    };
    let side_internal_safety_w = px(50.0);
    let left_slot_w = (left_content_w + side_internal_safety_w).max(left_min_w);
    let right_slot_w = (right_controls_w + side_internal_safety_w).max(left_min_w);
    // 不再强制左右对称：右侧音乐胶囊展开后会占更多宽度，
    // 中间导航胶囊应自动向左偏移，避免与右侧控件发生遮挡。
    let mut capsule_w = capsule_pad_x * 2.0
        + item_w * (nav_len as f32)
        + capsule_gap * (nav_len.saturating_sub(1) as f32);
    let max_capsule_w = (inner_w - left_slot_w - right_slot_w - slot_outer_gap * 2.0).max(px(204.));
    let allow_nav_shrink = layout_k <= 0.02;
    let fit_scale = if allow_nav_shrink {
        (max_capsule_w / expanded_capsule_w).clamp(0.82, 1.0)
    } else {
        1.0
    };
    if allow_nav_shrink && fit_scale < 1.0 {
        item_w *= fit_scale;
        label_w *= fit_scale;
        label_gap *= fit_scale;
        capsule_gap *= fit_scale;
        let scaled_collapsed_item_w = (collapsed_item_w * fit_scale).max(px(42.0));
        item_w = scaled_collapsed_item_w + label_gap + label_w;
        capsule_w = capsule_pad_x * 2.0
            + item_w * (nav_len as f32)
            + capsule_gap * (nav_len.saturating_sub(1) as f32);
    }

    let center_slot_w = (inner_w - left_slot_w - right_slot_w).max(capsule_w);
    let capsule_offset_in_center = ((center_slot_w - capsule_w) / 2.0).max(px(0.0));

    let step_w_px = (item_w / px(1.)) + (capsule_gap / px(1.));
    let max_offset = step_w_px * (nav_len.saturating_sub(1) as f32);
    let current_left_px = (step_w_px * pill_steps).clamp(0.0, max_offset);
    let previous_left_px = (current_left_px - step_w_px * pill_direction).clamp(0.0, max_offset);
    let current_inner_left_px = current_left_px;
    let previous_inner_left_px = previous_left_px;
    let current_inner_right_px = current_left_px + item_w / px(1.);
    let previous_inner_right_px = previous_left_px + item_w / px(1.);

    let (left_edge_px, right_edge_px) = if pill_direction >= 0.0 {
        let left = lerp_f32(
            previous_inner_left_px,
            current_inner_left_px,
            pill_trailing_progress,
        );
        let right = lerp_f32(
            previous_inner_right_px,
            current_inner_right_px,
            pill_leading_progress,
        );
        (left, right)
    } else {
        let left = lerp_f32(
            previous_inner_left_px,
            current_inner_left_px,
            pill_leading_progress,
        );
        let right = lerp_f32(
            previous_inner_right_px,
            current_inner_right_px,
            pill_trailing_progress,
        );
        (left, right)
    };

    let max_right_px = max_offset + item_w / px(1.);
    let clamped_left_px = left_edge_px.clamp(0.0, max_right_px);
    let clamped_right_px = right_edge_px.clamp(0.0, max_right_px);
    let pill_inner_inset = 1.5;
    let pill_offset = px(clamped_left_px.min(clamped_right_px) + pill_inner_inset);
    let pill_w = px(((clamped_right_px - clamped_left_px).abs() - pill_inner_inset * 2.0).max(0.0));

    let icon_btn_path = move |id: &'static str, icon_path_value: &'static str, close: bool| {
        div()
            .id(id)
            .w(px(40.))
            .h(px(40.))
            .rounded(px(12.))
            .flex()
            .items_center()
            .justify_center()
            .opacity(0.7)
            .cursor_pointer()
            .text_color(text_color)
            .hover(move |style| {
                if close {
                    style.bg(icon_hover_bg).opacity(1.0)
                } else {
                    style.bg(icon_hover_bg).opacity(1.0)
                }
            })
            .active(|style| style.opacity(1.0))
            .child(
                icon_path(icon_path_value)
                    .size(px(16.0))
                    .text_color(text_color),
            )
    };

    let left = div()
        .w(left_slot_w)
        .flex()
        .items_center()
        .gap(px(11.))
        .overflow_hidden()
        .child(
            div()
                .w(px(35.))
                .h(px(35.))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img("icons/logo.svg")
                        .w(px(38.))
                        .h(px(38.))
                        .object_fit(ObjectFit::Contain)
                        .decode_to_bounds(),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(7.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .line_height(px(17.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(accent)
                                .child("BMCBL"),
                        )
                        .child(
                            div()
                                .text_size(px(10.5))
                                .line_height(px(11.))
                                .text_color(lerp_color(rgb(0x64748b), rgb(0x9aa4b2), theme_k))
                                .child(app_version),
                        ),
                )
                .children(update_available.then(|| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .h(px(19.))
                        .px(px(9.))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.15,
                            ..colors.accent
                        })
                        .border_1()
                        .border_color(Hsla {
                            a: 0.30,
                            ..colors.accent
                        })
                        .hover(|e| {
                            e.bg(Hsla {
                                a: 0.20,
                                ..colors.accent
                            })
                        })
                        .cursor_pointer()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _window, cx| {
                            cx.stop_propagation();
                            let now = Instant::now();
                            cx.update_global(|u: &mut UpdateState, cx| {
                                u.request_open_modal(now);
                            });
                        })
                        .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(colors.accent))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .line_height(px(10.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.accent)
                                .child("新"),
                        )
                })),
        );

    let capsule = {
        // 激活态底板放在可点击链接层下方，避免遮挡鼠标命中区域。
        let pill_layer = div().absolute().inset_0().child(
            div()
                .absolute()
                .left(pill_offset)
                .top(px(0.))
                .w(pill_w)
                .h_full()
                .rounded(tab_radius)
                .bg(tab_pill_bg)
                .border_1()
                .border_color(tab_pill_border),
        );

        let links = div()
            .relative()
            .child(pill_layer)
            .child(div().flex().items_center().gap(capsule_gap).children(
                nav_items.iter().cloned().enumerate().map(|(idx, item)| {
                    let active = idx == active_index;
                    let fg: Hsla = if active {
                        rgb(0xffffff).into()
                    } else {
                        text_color
                    };
                    let icon = item.image_icon_path.clone().map_or_else(
                        || {
                            icon_path(item.icon_path)
                                .size(icon_w)
                                .text_color(fg)
                                .into_any_element()
                        },
                        |path| {
                            img(path)
                                .size(icon_w)
                                .rounded(px(5.0))
                                .object_fit(ObjectFit::Contain)
                                .decode_to_bounds()
                                .into_any_element()
                        },
                    );
                    let icon_box = div()
                        .w(icon_w)
                        .h_full()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .child(icon);
                    // 让图标始终在视觉中心，标签通过宽度/透明度展开，
                    // 避免“整块内容左右漂移”的观感。
                    let content_side_pad =
                        ((item_w - icon_w - label_gap - label_w) / 2.0).max(px(4.0));
                    let label = div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .w(label_w)
                        .ml(label_gap)
                        .opacity(label_k)
                        .h_full()
                        .flex()
                        .items_center()
                        .flex_none()
                        .text_size(px(12.5))
                        .line_height(px(14.0))
                        .text_left()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .child(item.label.clone());

                    let inactive_opacity = lerp_f32(0.78, 0.88, theme_k);
                    let link_content = div()
                        .w(item_w)
                        .h(item_h)
                        .rounded(tab_radius)
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .opacity(if active { 1.0 } else { inactive_opacity })
                        .cursor_pointer()
                        .hover(|s| s.opacity(1.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_start()
                                .w_full()
                                .h_full()
                                .px(content_side_pad)
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_start()
                                        .w_full()
                                        .h_full()
                                        .child(icon_box)
                                        .child(label),
                                ),
                        )
                        .into_any_element();

                    div()
                        .w(item_w)
                        .h(item_h)
                        .rounded(tab_radius)
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .opacity(if active { 1.0 } else { inactive_opacity })
                        .cursor_pointer()
                        .occlude()
                        .hover(|s| s.opacity(1.0))
                        .on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                            cx.stop_propagation();
                            navigation::navigate_target(cx, item.target.clone());
                        })
                        .child(link_content)
                }),
            ))
            .px(px(0.)); // keep stable

        div()
            .relative()
            .flex()
            .items_center()
            .gap(px(4.))
            .bg(capsule_shell_bg)
            .px(capsule_pad_x)
            .py(capsule_pad_y)
            // 胶囊容器圆角必须与内部 tab 圆角 (px(18.)) 加上 capsule_pad 对齐
            // rounded = tab_rounded + capsule_pad = 18 + 5 = 23，近似为 px(22.)
            .rounded(px((tab_radius / px(1.) + capsule_pad_y / px(1.)).max(14.0)))
            .border_1()
            .border_color(capsule_shell_border.opacity(0.72))
            .overflow_hidden()
            .child(links)
    };

    let center = div()
        .w(center_slot_w)
        .flex()
        .justify_start()
        .overflow_hidden()
        .child(
            div()
                .relative()
                .left(capsule_offset_in_center)
                .child(capsule),
        );

    let player_after_width = px(40.) + (px(1.) + px(8.) * 2.0) + px(40.) + px(8.) * 3.0;
    let music_render = crate::ui::main_window::music_player::render_music_player(
        music_snapshot,
        music_expanded_factor,
        music_progress_ratio,
        music_volume_ratio,
        music_drag_target,
        music_inline_factor,
        window_width,
        topbar_top + topbar_h + px(10.0),
        inset_x + nav_pad_x + player_after_width,
        accent,
        text_color,
        capsule_shell_border,
        capsule_shell_bg,
        lerp_color(
            Hsla {
                h: 0.08,
                s: 0.55,
                l: 0.64,
                a: 0.96,
            },
            Hsla {
                h: 0.08,
                s: 0.42,
                l: 0.26,
                a: 0.94,
            },
            theme_k,
        ),
        Hsla {
            a: lerp_f32(0.08, 0.18, theme_k),
            ..rgb(0x000000).into()
        },
    );

    let right_system_controls = div()
        .flex()
        .items_center()
        .gap(right_gap)
        .child(
            icon_btn_path(
                "theme-toggle",
                if target_dark {
                    lucide_icons::icon_sun()
                } else {
                    lucide_icons::icon_moon()
                },
                false,
            )
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _window, cx| {
                cx.stop_propagation();
                cx.update_global(|theme: &mut ThemeState, cx| {
                    theme.toggle(Instant::now());
                    ThemeState::sync_component_theme(theme.target_dark, cx);
                });
            }),
        )
        .child(
            div()
                .w(px(1.))
                .h(px(20.))
                .bg(text_color)
                .opacity(0.15)
                .mx(px(8.)),
        )
        .child(
            icon_btn_path("win-minimize", lucide_icons::icon_minus(), false)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    cx.stop_propagation();
                    window.minimize_window();
                }),
        )
        .child(
            icon_btn_path("win-close", lucide_icons::icon_x(), true)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _window, cx| {
                    cx.stop_propagation();
                    // Do not send WM_CLOSE directly: if Windows destroys the HWND before GPUI drops its
                    // PlatformWindow, GPUI's Drop will call DestroyWindow again and log "invalid window handle".
                    let now = Instant::now();
                    let (started, duration) = cx.update_global(|q: &mut QuitState, cx| {
                        let started = q.request_quit(now);
                        if started {}
                        (started, q.duration())
                    });
                    if !started {
                        return;
                    }

                    cx.spawn(async move |cx| -> gpui::Result<()> {
                        cx.background_executor().timer(duration).await;
                        cx.update(|cx| cx.quit())?;
                        Ok(())
                    })
                    .detach_and_log_err(cx);
                }),
        );

    let right = div()
        .w(right_slot_w)
        .flex()
        .items_center()
        .justify_end()
        .pr(slot_outer_gap)
        .child(
            div()
                .flex()
                .items_center()
                .gap(music_capsule_gap)
                .when(music_available, |this| this.child(music_render.inline))
                .child(right_system_controls),
        );

    let is_interactive_zone = {
        let inset_x = inset_x;
        let nav_pad_x = nav_pad_x;
        let right_controls_hit_w = right_controls_hit_w;
        let capsule_pad_x = capsule_pad_x;
        let capsule_pad_y = capsule_pad_y;
        let capsule_gap = capsule_gap;
        let item_w = item_w;
        let left_slot_w = left_slot_w;
        let center_slot_w = center_slot_w;
        let capsule_offset_in_center = capsule_offset_in_center;
        move |pos: Point<Pixels>, window_size: Size<Pixels>| {
            // 在右侧系统按钮和 tab 区域内，禁止触发窗口拖拽/双击最大化。
            // 行为与 WebView2 一致：胶囊背景可拖拽，具体 tab 不可拖拽。
            let right_x = window_size.width - inset_x - nav_pad_x - right_controls_hit_w;
            let right_block = Bounds::new(
                point(right_x, topbar_top),
                size(right_controls_hit_w, topbar_h),
            );
            if right_block.contains(&pos) {
                return true;
            }

            let capsule_h = item_h + capsule_pad_y * 2.0;
            let capsule_w = capsule_pad_x * 2.0
                + (nav_len as f32) * item_w
                + ((nav_len.saturating_sub(1)) as f32) * capsule_gap;
            let capsule_origin = point(
                inset_x + nav_pad_x + left_slot_w + capsule_offset_in_center,
                topbar_top + (topbar_h - capsule_h) / 2.0,
            );

            let tabs_origin = capsule_origin + point(capsule_pad_x, capsule_pad_y);
            for idx in 0..nav_len {
                let x = tabs_origin.x + (idx as f32) * (item_w + capsule_gap);
                let tab = Bounds::new(point(x, tabs_origin.y), size(item_w, item_h));
                if tab.contains(&pos) {
                    return true;
                }
            }

            false
        }
    };

    // 模拟原生标题栏行为：
    // - 双击切换最大化/还原
    // - 按下后必须发生位移才开始拖拽，避免普通点击被误判为拖窗
    let titlebar_mouse_down = {
        move |e: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            let window_size = window.window_bounds().get_bounds().size;
            if is_interactive_zone(e.position, window_size) {
                return;
            }

            cx.update_global(|topbar: &mut AppChromeState, _cx| {
                topbar
                    .titlebar_gesture
                    .handle_mouse_down(e, window, Instant::now());
            });
        }
    };

    let titlebar_mouse_move = {
        move |e: &MouseMoveEvent, window: &mut Window, cx: &mut App| {
            if !e.dragging() {
                return;
            }

            cx.update_global(|topbar: &mut AppChromeState, _cx| {
                topbar.titlebar_gesture.handle_mouse_move(e, window);
            });
        }
    };

    let titlebar_mouse_up = move |_e: &MouseUpEvent, _window: &mut Window, cx: &mut App| {
        cx.update_global(|topbar: &mut AppChromeState, _cx| {
            topbar.titlebar_gesture.handle_mouse_up();
        });
    };

    let nav_bar = div()
        .absolute()
        .top(topbar_top)
        .left(inset_x)
        .right(inset_x)
        .h(topbar_h)
        .rounded(topbar_radius)
        .bg(nav_bg)
        .overflow_hidden()
        .border_1()
        .border_color(border_color.opacity(lerp_f32(0.06, 0.10, theme_k)))
        .child(
            div()
                .relative()
                .size_full()
                .px(nav_pad_x)
                .flex()
                .items_center()
                .justify_between()
                .child(left)
                .child(center)
                .child(right),
        );

    let drag_surface = div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(topbar_top + topbar_h)
        .when(!cfg!(windows), |this| {
            this.occlude()
                .on_mouse_down(MouseButton::Left, titlebar_mouse_down)
                .on_mouse_move(titlebar_mouse_move)
                .on_mouse_up(MouseButton::Left, titlebar_mouse_up)
        })
        .when(cfg!(windows), |this| {
            this.window_control_area(WindowControlArea::Drag)
        })
        .child(nav_bar);

    let root_h = topbar_top
        + topbar_h
        + if music_render.overlay.is_some() {
            px(190.0)
        } else {
            px(0.0)
        };
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(root_h)
        .children(music_render.backdrop)
        .child(drag_surface)
        .children(music_render.overlay)
        .into_any_element()
}
