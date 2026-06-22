use crate::ui::components::input::{Input, InputState};
use crate::ui::theme::colors::{ThemeColors, parse_hex_color_to_hsla};
use crate::ui::views::settings::state::SettingsPageState;
use gpui::*;

const SATURATION_VALUE_WIDTH: f32 = 240.0;
const SATURATION_VALUE_HEIGHT: f32 = 160.0;
const HUE_BAR_WIDTH: f32 = 240.0;
const ALPHA_BAR_WIDTH: f32 = 240.0;
const COLOR_BAR_HEIGHT: f32 = 14.0;
const HUE_SEGMENTS: usize = 12;
const ALPHA_CHECKER_SEGMENTS: usize = 24;
const PRESET_CHIP_SIZE: f32 = 22.0;
const BAR_HANDLE_WIDTH: f32 = 12.0;
const BAR_HANDLE_HEIGHT: f32 = 18.0;

#[derive(Clone, Copy)]
enum ColorDragKind {
    SaturationValue,
    Hue,
    Alpha,
}

pub const DEFAULT_THEME_COLOR_PRESETS: &[&str] = &[
    "#a0d9b6", "#3b82f6", "#f97316", "#22c55e", "#ec4899", "#f59e0b", "#14b8a6", "#8b5cf6",
    "#ef4444", "#0ea5e9", "#84cc16", "#f43f5e",
];

pub fn normalize_hex_color(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let hex_trimmed = trimmed.trim_start_matches('#').to_ascii_lowercase();
    if (hex_trimmed.len() == 6 || hex_trimmed.len() == 8)
        && hex_trimmed.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Some(format!("#{hex_trimmed}"));
    }

    parse_css_rgb_like(trimmed)
        .or_else(|| parse_css_hsl_like(trimmed))
        .map(|rgba| rgba_to_hex_string(rgba.0, rgba.1, rgba.2, rgba.3))
}

fn split_css_function(input: &str, name: &str) -> Option<String> {
    let lower = input.trim().to_ascii_lowercase();
    let prefix = format!("{name}(");
    if !lower.starts_with(&prefix) || !lower.ends_with(')') {
        return None;
    }
    let content = lower[prefix.len()..lower.len().saturating_sub(1)].trim();
    if content.is_empty() {
        return None;
    }
    Some(content.to_string())
}

fn parse_css_rgb_like(input: &str) -> Option<(u8, u8, u8, u8)> {
    let content = split_css_function(input, "rgb").or_else(|| split_css_function(input, "rgba"))?;
    let parts = content
        .split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if parts.len() != 3 && parts.len() != 4 {
        return None;
    }

    let red = parse_rgb_channel(parts[0])?;
    let green = parse_rgb_channel(parts[1])?;
    let blue = parse_rgb_channel(parts[2])?;
    let alpha = if parts.len() == 4 {
        parse_alpha_channel(parts[3])?
    } else {
        255
    };
    Some((red, green, blue, alpha))
}

fn parse_css_hsl_like(input: &str) -> Option<(u8, u8, u8, u8)> {
    let content = split_css_function(input, "hsl").or_else(|| split_css_function(input, "hsla"))?;
    let parts = content
        .split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if parts.len() != 3 && parts.len() != 4 {
        return None;
    }

    let mut hue = parts[0].trim_end_matches("deg").parse::<f32>().ok()?;
    if !hue.is_finite() {
        return None;
    }
    hue = hue.rem_euclid(360.0);

    let saturation = parse_percentage_channel(parts[1])?;
    let lightness = parse_percentage_channel(parts[2])?;
    let alpha = if parts.len() == 4 {
        parse_alpha_channel(parts[3])?
    } else {
        255
    };

    let (red, green, blue) = hsl_to_rgb(hue, saturation, lightness);
    Some((red, green, blue, alpha))
}

fn parse_rgb_channel(value: &str) -> Option<u8> {
    if let Some(raw_percent) = value.strip_suffix('%') {
        let percent = raw_percent.trim().parse::<f32>().ok()?;
        if !percent.is_finite() {
            return None;
        }
        let normalized = (percent / 100.0).clamp(0.0, 1.0);
        return Some((normalized * 255.0).round() as u8);
    }

    let channel = value.parse::<f32>().ok()?;
    if !channel.is_finite() {
        return None;
    }
    Some(channel.clamp(0.0, 255.0).round() as u8)
}

fn parse_percentage_channel(value: &str) -> Option<f32> {
    let raw = value.strip_suffix('%')?.trim().parse::<f32>().ok()?;
    if !raw.is_finite() {
        return None;
    }
    Some((raw / 100.0).clamp(0.0, 1.0))
}

fn parse_alpha_channel(value: &str) -> Option<u8> {
    if let Some(raw_percent) = value.strip_suffix('%') {
        let percent = raw_percent.trim().parse::<f32>().ok()?;
        if !percent.is_finite() {
            return None;
        }
        let normalized = (percent / 100.0).clamp(0.0, 1.0);
        return Some((normalized * 255.0).round() as u8);
    }

    let alpha = value.parse::<f32>().ok()?;
    if !alpha.is_finite() {
        return None;
    }
    if alpha <= 1.0 {
        return Some((alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    }
    Some(alpha.clamp(0.0, 255.0).round() as u8)
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < (1.0 / 6.0) {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < (2.0 / 3.0) {
        p + (q - p) * ((2.0 / 3.0) - t) * 6.0
    } else {
        p
    }
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (u8, u8, u8) {
    if saturation <= f32::EPSILON {
        let gray = (lightness.clamp(0.0, 1.0) * 255.0).round() as u8;
        return (gray, gray, gray);
    }

    let h = hue / 360.0;
    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    let red = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let green = hue_to_rgb(p, q, h);
    let blue = hue_to_rgb(p, q, h - 1.0 / 3.0);

    (
        (red * 255.0).round().clamp(0.0, 255.0) as u8,
        (green * 255.0).round().clamp(0.0, 255.0) as u8,
        (blue * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn rgba_to_hex_string(red: u8, green: u8, blue: u8, alpha: u8) -> String {
    if alpha == 255 {
        format!("#{red:02x}{green:02x}{blue:02x}")
    } else {
        format!("#{red:02x}{green:02x}{blue:02x}{alpha:02x}")
    }
}

fn parse_normalized_hex_to_rgba(input: &str) -> Option<(u8, u8, u8, u8)> {
    let normalized = normalize_hex_color(input)?;
    let body = normalized.trim_start_matches('#');
    if body.len() != 6 && body.len() != 8 {
        return None;
    }
    let red = u8::from_str_radix(&body[0..2], 16).ok()?;
    let green = u8::from_str_radix(&body[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&body[4..6], 16).ok()?;
    let alpha = if body.len() == 8 {
        u8::from_str_radix(&body[6..8], 16).ok()?
    } else {
        255
    };
    Some((red, green, blue, alpha))
}

fn rgb_to_hsv(red: u8, green: u8, blue: u8) -> (f32, f32, f32) {
    let r = red as f32 / 255.0;
    let g = green as f32 / 255.0;
    let b = blue as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta <= f32::EPSILON {
        0.0
    } else if (max - r).abs() <= f32::EPSILON {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() <= f32::EPSILON {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (
        hue.rem_euclid(360.0),
        saturation.clamp(0.0, 1.0),
        max.clamp(0.0, 1.0),
    )
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (u8, u8, u8) {
    let h = hue.rem_euclid(360.0);
    let s = saturation.clamp(0.0, 1.0);
    let v = value.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0).rem_euclid(2.0)) - 1.0).abs());
    let m = v - c;

    let (r1, g1, b1) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn hsva_to_hex(hue: f32, saturation: f32, value: f32, alpha: f32) -> String {
    let (red, green, blue) = hsv_to_rgb(hue, saturation, value);
    let alpha_byte = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    rgba_to_hex_string(red, green, blue, alpha_byte)
}

fn to_number(value: Pixels) -> f32 {
    value / px(1.0)
}

fn clamp_unit(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn ratio_from_bounds(position: Pixels, start: Pixels, length: f32) -> f32 {
    clamp_unit(to_number(position - start) / length)
}

fn saturation_value_from_bounds(position: Point<Pixels>, bounds: Bounds<Pixels>) -> (f32, f32) {
    let saturation = ratio_from_bounds(position.x, bounds.left(), SATURATION_VALUE_WIDTH);
    let value = 1.0 - ratio_from_bounds(position.y, bounds.top(), SATURATION_VALUE_HEIGHT);
    (saturation, value)
}

pub fn color_picker_control(
    id: &'static str,
    colors: &ThemeColors,
    current_hex: &str,
    input_state: Option<&Entity<InputState>>,
    presets: &'static [&'static str],
    popup_open: bool,
    _drag_target: &str,
    _drag_origin_x: f32,
    _drag_origin_y: f32,
    _drag_origin_hue: f32,
    _drag_origin_saturation: f32,
    _drag_origin_value: f32,
    _drag_origin_alpha: f32,
    popup_anchor_x: f32,
    popup_anchor_y: f32,
    on_pick: impl Fn(&str, &mut Window, &mut App) + Clone + 'static,
) -> Stateful<Div> {
    let normalized = normalize_hex_color(current_hex).unwrap_or_else(|| "#a0d9b6".to_string());
    let preview = parse_hex_color_to_hsla(&normalized).unwrap_or(colors.accent);
    let preview_swatch = Hsla { a: 1.0, ..preview };
    let (red, green, blue, alpha_byte) =
        parse_normalized_hex_to_rgba(&normalized).unwrap_or((160, 217, 182, 255));
    let (current_hue, current_saturation, current_value) = rgb_to_hsv(red, green, blue);
    let current_alpha = alpha_byte as f32 / 255.0;

    let input_control: AnyElement = if let Some(state) = input_state {
        Input::new(state)
            .appearance(false)
            .bordered(false)
            .focus_bordered(false)
            .cleanable(false)
            .w_full()
            .h(px(32.0))
            .px(px(0.0))
            .text_size(px(13.0))
            .into_any_element()
    } else {
        div()
            .w_full()
            .h(px(32.0))
            .flex()
            .items_center()
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(colors.text_secondary)
                    .child(SharedString::from(normalized.clone())),
            )
            .into_any_element()
    };

    let input = div()
        .w(px(180.0))
        .h(px(36.0))
        .rounded(px(12.0))
        .text_color(colors.text_primary)
        .bg(Hsla {
            a: 0.82,
            ..colors.settings_field_bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.28,
            ..colors.border
        })
        .px(px(5.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .w(px(12.0))
                .h(px(12.0))
                .rounded(px(4.0))
                .bg(preview_swatch)
                .border_1()
                .border_color(Hsla {
                    a: 0.42,
                    ..colors.border
                }),
        )
        .child(div().flex_1().min_w(px(0.0)).h_full().child(input_control));

    let mut presets_row = div().flex().flex_wrap().items_center().gap(px(8.0));
    for preset in presets {
        let on_pick = on_pick.clone();
        let normalized_preset = normalize_hex_color(preset).unwrap_or_else(|| preset.to_string());
        let active = normalized_preset.eq_ignore_ascii_case(&normalized);
        let swatch = parse_hex_color_to_hsla(&normalized_preset).unwrap_or(colors.accent);
        let mut chip = div()
            .w(px(PRESET_CHIP_SIZE))
            .h(px(PRESET_CHIP_SIZE))
            .rounded(px(6.0))
            .bg(swatch)
            .border_1()
            .border_color(if active {
                Hsla {
                    a: 0.70,
                    ..colors.border
                }
            } else {
                Hsla {
                    a: 0.26,
                    ..colors.border
                }
            });
        chip = chip
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                on_pick(normalized_preset.as_str(), window, cx);
            });
        presets_row = presets_row.child(chip);
    }

    let saturation_value_cursor_x =
        (current_saturation * SATURATION_VALUE_WIDTH).clamp(0.0, SATURATION_VALUE_WIDTH);
    let saturation_value_cursor_y =
        ((1.0 - current_value) * SATURATION_VALUE_HEIGHT).clamp(0.0, SATURATION_VALUE_HEIGHT);
    let hue_cursor_x = ((current_hue / 360.0) * HUE_BAR_WIDTH).clamp(0.0, HUE_BAR_WIDTH);
    let alpha_cursor_x = (current_alpha * ALPHA_BAR_WIDTH).clamp(0.0, ALPHA_BAR_WIDTH);
    let hue_preview_hex = hsva_to_hex(current_hue, 1.0, 1.0, 1.0);
    let hue_preview = parse_hex_color_to_hsla(&hue_preview_hex).unwrap_or(colors.accent);
    let opaque_preview_hex = hsva_to_hex(current_hue, current_saturation, current_value, 1.0);
    let opaque_preview = parse_hex_color_to_hsla(&opaque_preview_hex).unwrap_or(colors.accent);

    let saturation_value_surface = div()
        .relative()
        .w(px(SATURATION_VALUE_WIDTH))
        .h(px(SATURATION_VALUE_HEIGHT))
        .rounded(px(8.0))
        .overflow_hidden()
        .border_1()
        .border_color(Hsla {
            a: 0.40,
            ..colors.border
        })
        .child(div().absolute().inset_0().bg(hue_preview))
        .child(div().absolute().inset_0().bg(linear_gradient(
            90.0,
            linear_color_stop(hsla(0.0, 0.0, 1.0, 1.0), 0.0),
            linear_color_stop(hsla(0.0, 0.0, 1.0, 0.0), 1.0),
        )))
        .child(div().absolute().inset_0().bg(linear_gradient(
            180.0,
            linear_color_stop(hsla(0.0, 0.0, 0.0, 0.0), 0.0),
            linear_color_stop(hsla(0.0, 0.0, 0.0, 1.0), 1.0),
        )));

    let mut hue_bar_fill = div()
        .w_full()
        .h_full()
        .rounded(px(999.0))
        .overflow_hidden()
        .flex();
    for segment in 0..HUE_SEGMENTS {
        let start_hue = 360.0 * segment as f32 / HUE_SEGMENTS as f32;
        let end_hue = 360.0 * (segment + 1) as f32 / HUE_SEGMENTS as f32;
        let start_color = parse_hex_color_to_hsla(&hsva_to_hex(start_hue, 1.0, 1.0, 1.0))
            .unwrap_or(colors.accent);
        let end_color =
            parse_hex_color_to_hsla(&hsva_to_hex(end_hue, 1.0, 1.0, 1.0)).unwrap_or(colors.accent);
        hue_bar_fill = hue_bar_fill.child(div().flex_1().h_full().bg(linear_gradient(
            90.0,
            linear_color_stop(start_color, 0.0),
            linear_color_stop(end_color, 1.0),
        )));
    }

    let hue_bar_grid = div()
        .relative()
        .w(px(HUE_BAR_WIDTH))
        .h(px(COLOR_BAR_HEIGHT))
        .rounded(px(999.0))
        .border_1()
        .border_color(Hsla {
            a: 0.40,
            ..colors.border
        })
        .p(px(1.0))
        .child(hue_bar_fill);

    let mut alpha_checker_grid = div().absolute().inset_0().flex();
    for segment in 0..ALPHA_CHECKER_SEGMENTS {
        alpha_checker_grid =
            alpha_checker_grid.child(div().flex_1().h_full().bg(if segment % 2 == 0 {
                hsla(0.0, 0.0, 1.0, 0.30)
            } else {
                hsla(0.0, 0.0, 0.0, 0.14)
            }));
    }

    let alpha_bar_fill = div()
        .relative()
        .w_full()
        .h_full()
        .rounded(px(999.0))
        .overflow_hidden()
        .child(alpha_checker_grid)
        .child(div().absolute().inset_0().bg(linear_gradient(
            90.0,
            linear_color_stop(
                Hsla {
                    a: 0.0,
                    ..opaque_preview
                },
                0.0,
            ),
            linear_color_stop(opaque_preview, 1.0),
        )));

    let alpha_bar_grid = div()
        .relative()
        .w(px(ALPHA_BAR_WIDTH))
        .h(px(COLOR_BAR_HEIGHT))
        .rounded(px(999.0))
        .border_1()
        .border_color(Hsla {
            a: 0.40,
            ..colors.border
        })
        .p(px(1.0))
        .child(alpha_bar_fill);

    let popup = div()
        .w(px(286.0))
        .rounded(px(12.0))
        .bg(Hsla {
            a: 0.97,
            ..colors.settings_panel_bg
        })
        .border_1()
        .border_color(Hsla {
            a: 0.40,
            ..colors.border
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.20,
            },
            blur_radius: px(26.0),
            spread_radius: px(-8.0),
            offset: point(px(0.0), px(14.0)),
        }])
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .on_mouse_down_out(|_event, _window, cx| {
            cx.stop_propagation();
            cx.update_global(|state: &mut SettingsPageState, cx| {
                state.theme_color_picker_popup_open = false;
                state.theme_color_picker_drag_target = SharedString::from("");
            });
        })
        .child(
            div()
                .text_size(px(12.0))
                .text_color(colors.text_primary)
                .child(format!(
                    "HSB: hsb({:.0}, {:.0}%, {:.0}%)",
                    current_hue.round(),
                    (current_saturation * 100.0).round(),
                    (current_value * 100.0).round()
                )),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id}-sv")))
                .relative()
                .w(px(SATURATION_VALUE_WIDTH))
                .h(px(SATURATION_VALUE_HEIGHT))
                .cursor_pointer()
                .on_drag(
                    ColorDragKind::SaturationValue,
                    |_: &ColorDragKind, _, _, cx: &mut App| cx.new(|_| Empty),
                )
                .on_mouse_down(MouseButton::Left, {
                    move |event, _window, cx| {
                        cx.stop_propagation();
                        cx.update_global(|state: &mut SettingsPageState, cx| {
                            state.theme_color_picker_popup_open = true;
                            state.theme_color_picker_drag_target = SharedString::from("sv");
                            state.theme_color_picker_drag_origin_x = to_number(event.position.x);
                            state.theme_color_picker_drag_origin_y = to_number(event.position.y);
                            state.theme_color_picker_drag_origin_hue = current_hue;
                            state.theme_color_picker_drag_origin_saturation = current_saturation;
                            state.theme_color_picker_drag_origin_value = current_value;
                            state.theme_color_picker_drag_origin_alpha = current_alpha;
                        });
                    }
                })
                .on_drag_move::<ColorDragKind>({
                    let on_pick = on_pick.clone();
                    move |event, window, cx| {
                        if !matches!(event.drag(cx), ColorDragKind::SaturationValue) {
                            return;
                        }

                        let (next_saturation, next_value) =
                            saturation_value_from_bounds(event.event.position, event.bounds);
                        let next_hex =
                            hsva_to_hex(current_hue, next_saturation, next_value, current_alpha);
                        on_pick(next_hex.as_str(), window, cx);
                    }
                })
                .on_mouse_up(MouseButton::Left, |_event, _window, cx| {
                    cx.update_global(|state: &mut SettingsPageState, cx| {
                        state.theme_color_picker_drag_target = SharedString::from("");
                    });
                })
                .on_mouse_up_out(MouseButton::Left, |_event, _window, cx| {
                    cx.update_global(|state: &mut SettingsPageState, cx| {
                        state.theme_color_picker_drag_target = SharedString::from("");
                    });
                })
                .child(saturation_value_surface)
                .child(
                    div()
                        .absolute()
                        .left(px(saturation_value_cursor_x - 5.0))
                        .top(px(saturation_value_cursor_y - 5.0))
                        .w(px(10.0))
                        .h(px(10.0))
                        .rounded_full()
                        .border_2()
                        .border_color(hsla(0.0, 0.0, 1.0, 0.98)),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id}-hue")))
                .relative()
                .w(px(HUE_BAR_WIDTH))
                .h(px(COLOR_BAR_HEIGHT))
                .cursor_pointer()
                .on_drag(
                    ColorDragKind::Hue,
                    |_: &ColorDragKind, _, _, cx: &mut App| cx.new(|_| Empty),
                )
                .on_mouse_down(MouseButton::Left, {
                    move |event, _window, cx| {
                        cx.stop_propagation();
                        cx.update_global(|state: &mut SettingsPageState, cx| {
                            state.theme_color_picker_popup_open = true;
                            state.theme_color_picker_drag_target = SharedString::from("hue");
                            state.theme_color_picker_drag_origin_x = to_number(event.position.x);
                            state.theme_color_picker_drag_origin_y = to_number(event.position.y);
                            state.theme_color_picker_drag_origin_hue = current_hue;
                            state.theme_color_picker_drag_origin_saturation = current_saturation;
                            state.theme_color_picker_drag_origin_value = current_value;
                            state.theme_color_picker_drag_origin_alpha = current_alpha;
                        });
                    }
                })
                .on_drag_move::<ColorDragKind>({
                    let on_pick = on_pick.clone();
                    move |event, window, cx| {
                        if !matches!(event.drag(cx), ColorDragKind::Hue) {
                            return;
                        }
                        let next_hue = ratio_from_bounds(
                            event.event.position.x,
                            event.bounds.left(),
                            HUE_BAR_WIDTH,
                        ) * 360.0;
                        let next_hex =
                            hsva_to_hex(next_hue, current_saturation, current_value, current_alpha);
                        on_pick(next_hex.as_str(), window, cx);
                    }
                })
                .on_mouse_up(MouseButton::Left, |_event, _window, cx| {
                    cx.update_global(|state: &mut SettingsPageState, cx| {
                        state.theme_color_picker_drag_target = SharedString::from("");
                    });
                })
                .on_mouse_up_out(MouseButton::Left, |_event, _window, cx| {
                    cx.update_global(|state: &mut SettingsPageState, cx| {
                        state.theme_color_picker_drag_target = SharedString::from("");
                    });
                })
                .child(hue_bar_grid)
                .child(
                    div()
                        .absolute()
                        .left(px(hue_cursor_x - (BAR_HANDLE_WIDTH / 2.0)))
                        .top(px(-2.0))
                        .w(px(BAR_HANDLE_WIDTH))
                        .h(px(BAR_HANDLE_HEIGHT))
                        .rounded(px(999.0))
                        .bg(hsla(0.0, 0.0, 1.0, 0.96))
                        .border_2()
                        .border_color(hsla(0.0, 0.0, 0.08, 0.82))
                        .shadow(vec![BoxShadow {
                            color: hsla(0.0, 0.0, 0.0, 0.18),
                            blur_radius: px(8.0),
                            spread_radius: px(0.0),
                            offset: point(px(0.0), px(1.0)),
                        }])
                        .child(
                            div()
                                .w(px(2.0))
                                .h(px(8.0))
                                .rounded(px(999.0))
                                .bg(hsla(0.0, 0.0, 0.20, 0.60))
                                .mx_auto()
                                .mt(px(3.0)),
                        ),
                ),
        )
        .child(
            div()
                .id(SharedString::from(format!("{id}-alpha")))
                .relative()
                .w(px(ALPHA_BAR_WIDTH))
                .h(px(COLOR_BAR_HEIGHT))
                .cursor_pointer()
                .on_drag(
                    ColorDragKind::Alpha,
                    |_: &ColorDragKind, _, _, cx: &mut App| cx.new(|_| Empty),
                )
                .on_mouse_down(MouseButton::Left, {
                    move |event, _window, cx| {
                        cx.stop_propagation();
                        cx.update_global(|state: &mut SettingsPageState, cx| {
                            state.theme_color_picker_popup_open = true;
                            state.theme_color_picker_drag_target = SharedString::from("alpha");
                            state.theme_color_picker_drag_origin_x = to_number(event.position.x);
                            state.theme_color_picker_drag_origin_y = to_number(event.position.y);
                            state.theme_color_picker_drag_origin_hue = current_hue;
                            state.theme_color_picker_drag_origin_saturation = current_saturation;
                            state.theme_color_picker_drag_origin_value = current_value;
                            state.theme_color_picker_drag_origin_alpha = current_alpha;
                        });
                    }
                })
                .on_drag_move::<ColorDragKind>({
                    let on_pick = on_pick.clone();
                    move |event, window, cx| {
                        if !matches!(event.drag(cx), ColorDragKind::Alpha) {
                            return;
                        }
                        let next_alpha = ratio_from_bounds(
                            event.event.position.x,
                            event.bounds.left(),
                            ALPHA_BAR_WIDTH,
                        );
                        let next_hex =
                            hsva_to_hex(current_hue, current_saturation, current_value, next_alpha);
                        on_pick(next_hex.as_str(), window, cx);
                    }
                })
                .on_mouse_up(MouseButton::Left, |_event, _window, cx| {
                    cx.update_global(|state: &mut SettingsPageState, cx| {
                        state.theme_color_picker_drag_target = SharedString::from("");
                    });
                })
                .on_mouse_up_out(MouseButton::Left, |_event, _window, cx| {
                    cx.update_global(|state: &mut SettingsPageState, cx| {
                        state.theme_color_picker_drag_target = SharedString::from("");
                    });
                })
                .child(alpha_bar_grid)
                .child(
                    div()
                        .absolute()
                        .left(px(alpha_cursor_x - (BAR_HANDLE_WIDTH / 2.0)))
                        .top(px(-2.0))
                        .w(px(BAR_HANDLE_WIDTH))
                        .h(px(BAR_HANDLE_HEIGHT))
                        .rounded(px(999.0))
                        .bg(hsla(0.0, 0.0, 1.0, 0.96))
                        .border_2()
                        .border_color(hsla(0.0, 0.0, 0.08, 0.82))
                        .shadow(vec![BoxShadow {
                            color: hsla(0.0, 0.0, 0.0, 0.18),
                            blur_radius: px(8.0),
                            spread_radius: px(0.0),
                            offset: point(px(0.0), px(1.0)),
                        }])
                        .child(
                            div()
                                .w(px(2.0))
                                .h(px(8.0))
                                .rounded(px(999.0))
                                .bg(hsla(0.0, 0.0, 0.20, 0.60))
                                .mx_auto()
                                .mt(px(3.0)),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(colors.text_secondary)
                        .child(SharedString::from(normalized.clone())),
                )
                .child(
                    div()
                        .w(px(PRESET_CHIP_SIZE))
                        .h(px(PRESET_CHIP_SIZE))
                        .rounded(px(6.0))
                        .bg(preview_swatch)
                        .border_1()
                        .border_color(Hsla {
                            a: 0.40,
                            ..colors.border
                        }),
                ),
        );

    let popup_toggle = div()
        .h(px(32.0))
        .px(px(6.0))
        .rounded(px(8.0))
        .bg(colors.settings_field_bg)
        .border_1()
        .border_color(Hsla {
            a: 0.40,
            ..colors.border
        })
        .flex()
        .items_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |event, _window, cx| {
            cx.stop_propagation();
            cx.update_global(|state: &mut SettingsPageState, cx| {
                state.theme_color_picker_popup_open = !state.theme_color_picker_popup_open;
                state.theme_color_picker_popup_anchor_x = to_number(event.position.x);
                state.theme_color_picker_popup_anchor_y = to_number(event.position.y);
                if !state.theme_color_picker_popup_open {
                    state.theme_color_picker_drag_target = SharedString::from("");
                }
            });
        })
        .child(
            div()
                .w(px(PRESET_CHIP_SIZE))
                .h(px(PRESET_CHIP_SIZE))
                .rounded(px(7.0))
                .bg(preview_swatch)
                .border_1()
                .border_color(Hsla {
                    a: 0.36,
                    ..colors.border
                }),
        );

    let mut root = div()
        .id(id)
        .w(px(360.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(
            div()
                .w_full()
                .h(px(40.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(popup_toggle)
                .child(input)
                .justify_end(),
        );

    if popup_open {
        root = root.child(deferred(
            anchored()
                .snap_to_window_with_margin(px(8.0))
                .anchor(Corner::TopLeft)
                .position(point(px(popup_anchor_x), px(popup_anchor_y + 12.0)))
                .child(popup),
        ));
    }

    root.child(presets_row)
}
