// 统一主题颜色系统
// 主题“玻璃/导航”参考 .upstream_bmbl_1/src/index.css
// UpdateModal 的内容颜色参考 .upstream_bmbl_1/src/components/update-modal.css

use gpui::*;

pub fn parse_hex_color_to_hsla(input: &str) -> Option<Hsla> {
    let s = input.trim().trim_start_matches('#');
    if s.len() != 6 && s.len() != 8 {
        return None;
    }

    let rgb_hex = &s[0..6];
    let rgb_value = u32::from_str_radix(rgb_hex, 16).ok()?;
    let mut color: Hsla = rgb(rgb_value).into();

    if s.len() == 8 {
        let a_hex = &s[6..8];
        let a = u8::from_str_radix(a_hex, 16).ok()? as f32 / 255.0;
        color.a = a.clamp(0.0, 1.0);
    }

    Some(color)
}

/// 浅色主题颜色
pub struct LightColors;
/// 深色主题颜色
pub struct DarkColors;

/// 通用主题颜色结构
#[derive(Clone, Copy, PartialEq)]
pub struct ThemeColors {
    // 基础颜色
    pub bg: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,

    // 强调色
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_glow: Hsla,

    // 危险色
    pub danger: Hsla,

    // 表面颜色
    pub border: Hsla,
    pub surface: Hsla,
    pub surface_hover: Hsla,
    pub settings_panel_bg: Hsla,
    pub settings_card_bg: Hsla,
    pub settings_field_bg: Hsla,

    // 遮罩
    pub backdrop: Hsla,

    // 进度条
    pub progress_track: Hsla,
    pub progress_fill: Hsla,

    // 徽章颜色
    pub badge_stable_bg: Hsla,
    pub badge_stable_text: Hsla,
    pub badge_beta_bg: Hsla,
    pub badge_beta_text: Hsla,

    // 按钮颜色
    pub btn_ghost_text: Hsla,
    pub btn_ghost_hover_bg: Hsla,
    pub btn_primary_text: Hsla,

    // 统计卡片图标颜色
    pub stat_blue_bg: Hsla,
    pub stat_blue_text: Hsla,
    pub stat_orange_bg: Hsla,
    pub stat_orange_text: Hsla,
    pub stat_green_bg: Hsla,
    pub stat_green_text: Hsla,

    // 窗口控制按钮
    pub window_control_normal: Hsla,
    pub window_control_hover_bg: Hsla,
    pub window_control_disabled: Hsla,
}

impl LightColors {
    pub fn colors() -> ThemeColors {
        ThemeColors {
            // 基础颜色 - 参考 update-modal.css :root (浅色模式)
            bg: rgb(0xffffff).into(),             // #ffffff 纯白背景
            text_primary: rgb(0x1e293b).into(),   // #1e293b 主文本
            text_secondary: rgb(0x64748b).into(), // #64748b 次级文本
            text_muted: rgb(0x94a3b8).into(),     // #94a3b8 淡化文本

            // 强调色 - 蓝色
            accent: rgb(0x3b82f6).into(),       // #3b82f6
            accent_hover: rgb(0x2563eb).into(), // #2563eb
            accent_glow: Hsla {
                a: 0.40,
                ..rgb(0x3b82f6).into()
            },

            // 危险色 - 避免红色主题（更中性的橙色）
            danger: rgb(0xf97316).into(),

            // 表面颜色 - 浅灰色
            border: rgb(0xd1d5db).into(),        // #d1d5db gray line
            surface: rgb(0xf8fafc).into(),       // #f8fafc
            surface_hover: rgb(0xf1f5f9).into(), // #f1f5f9
            settings_panel_bg: Hsla {
                a: 0.92,
                ..rgb(0xffffff).into()
            },
            settings_card_bg: Hsla {
                a: 0.92,
                ..rgb(0xf1f5f9).into()
            },
            settings_field_bg: Hsla {
                a: 0.92,
                ..rgb(0xffffff).into()
            },

            // 导航/玻璃背景 - 参考 index.css: --nav-bg rgba(255, 255, 255, 0.7)
            backdrop: Hsla {
                a: 0.70,
                ..rgb(0xffffff).into()
            },

            // 进度条
            progress_track: rgb(0xf8fafc).into(),
            progress_fill: rgb(0x3b82f6).into(),

            // 徽章颜色 - 固定色相，只调整明度
            badge_stable_bg: rgb(0xdcfce7).into(), // 稳定版背景（浅绿）
            badge_stable_text: rgb(0x166534).into(), // 深绿色文字
            badge_beta_bg: rgb(0xffedd5).into(),   // 预览版背景（浅橙）
            badge_beta_text: rgb(0x9a3412).into(), // 橙色文字

            // 按钮颜色
            btn_ghost_text: rgb(0x94a3b8).into(),
            btn_ghost_hover_bg: rgb(0xf8fafc).into(),
            btn_primary_text: rgb(0xffffff).into(),

            // 统计卡片图标颜色
            stat_blue_bg: rgb(0xdbeafe).into(),
            stat_blue_text: rgb(0x3b82f6).into(),
            stat_orange_bg: rgb(0xffedd5).into(),
            stat_orange_text: rgb(0xf97316).into(),
            stat_green_bg: rgb(0xdcfce7).into(),
            stat_green_text: rgb(0x22c55e).into(),

            // 窗口控制按钮
            window_control_normal: rgb(0x94a3b8).into(),
            window_control_hover_bg: rgb(0xf8fafc).into(),
            window_control_disabled: rgb(0xcbd5e1).into(),
        }
    }
}

impl DarkColors {
    pub fn colors() -> ThemeColors {
        ThemeColors {
            // 基础颜色 - 参考 update-modal.css [data-theme='dark'] (深色模式)
            bg: rgb(0x1e293b).into(),             // #1e293b 深蓝背景
            text_primary: rgb(0xf8fafc).into(),   // #f8fafc 主文本
            text_secondary: rgb(0xcbd5e1).into(), // #cbd5e1 次级文本
            text_muted: rgb(0x64748b).into(),     // #64748b 淡化文本

            // 强调色 - 亮蓝色
            accent: rgb(0x60a5fa).into(), // #60a5fa
            accent_hover: rgb(0x3b82f6).into(),
            accent_glow: Hsla {
                a: 0.50,
                ..rgb(0x60a5fa).into()
            },

            // 危险色 - 避免红色主题（更中性的橙色）
            danger: rgb(0xfb923c).into(),

            // 表面颜色 - 深色
            border: rgb(0x334155).into(),  // #334155 softer slate line
            surface: rgb(0x0f172a).into(), // #0f172a
            surface_hover: rgb(0x1e293b).into(), // #1e293b
            settings_panel_bg: Hsla {
                a: 0.92,
                ..rgb(0x1e293b).into()
            },
            settings_card_bg: Hsla {
                a: 0.92,
                ..rgb(0x1e293b).into()
            },
            settings_field_bg: Hsla {
                a: 0.92,
                ..rgb(0x0f172a).into()
            },

            // 导航/玻璃背景 - keep neutral during theme interpolation to avoid a blue cast.
            // This matches a CSS glass overlay better when animated.
            backdrop: hsla(0.0, 0.0, 0.0, 0.70),

            // 进度条
            progress_track: rgb(0x1e293b).into(),
            progress_fill: rgb(0x60a5fa).into(),

            // 徽章颜色 - 固定色相，只调整明度
            badge_stable_bg: rgb(0x052e16).into(), // 稳定版背景（深绿）
            badge_stable_text: rgb(0x86efac).into(), // 浅绿色文字
            badge_beta_bg: rgb(0x431407).into(),   // 预览版背景（深橙）
            badge_beta_text: rgb(0xfdba74).into(), // 浅橙色文字

            // 按钮颜色
            btn_ghost_text: rgb(0x64748b).into(),
            btn_ghost_hover_bg: rgb(0x0f172a).into(),
            btn_primary_text: rgb(0xffffff).into(),

            // 统计卡片图标颜色
            stat_blue_bg: rgb(0x0b254f).into(),
            stat_blue_text: rgb(0x60a5fa).into(),
            stat_orange_bg: rgb(0x3b1d06).into(),
            stat_orange_text: rgb(0xfdba74).into(),
            stat_green_bg: rgb(0x052e16).into(),
            stat_green_text: rgb(0x86efac).into(),

            // 窗口控制按钮
            window_control_normal: rgb(0x64748b).into(),
            window_control_hover_bg: rgb(0x0f172a).into(),
            window_control_disabled: rgb(0x475569).into(),
        }
    }
}

/// 根据主题因子插值颜色 (0.0 = 浅色，1.0 = 深色)
pub fn lerp_theme_colors(
    light: &ThemeColors,
    dark: &ThemeColors,
    t: f32,
    accent_override: Option<Hsla>,
) -> ThemeColors {
    let t = t.clamp(0.0, 1.0);
    let mut out = ThemeColors {
        bg: lerp_hsla(light.bg, dark.bg, t),
        text_primary: lerp_hsla(light.text_primary, dark.text_primary, t),
        text_secondary: lerp_hsla(light.text_secondary, dark.text_secondary, t),
        text_muted: lerp_hsla(light.text_muted, dark.text_muted, t),
        accent: lerp_hsla(light.accent, dark.accent, t),
        accent_hover: lerp_hsla(light.accent_hover, dark.accent_hover, t),
        accent_glow: lerp_hsla(light.accent_glow, dark.accent_glow, t),
        danger: lerp_hsla(light.danger, dark.danger, t),
        border: lerp_hsla(light.border, dark.border, t),
        surface: lerp_hsla(light.surface, dark.surface, t),
        surface_hover: lerp_hsla(light.surface_hover, dark.surface_hover, t),
        settings_panel_bg: lerp_hsla(light.settings_panel_bg, dark.settings_panel_bg, t),
        settings_card_bg: lerp_hsla(light.settings_card_bg, dark.settings_card_bg, t),
        settings_field_bg: lerp_hsla(light.settings_field_bg, dark.settings_field_bg, t),
        backdrop: lerp_hsla(light.backdrop, dark.backdrop, t),
        progress_track: lerp_hsla(light.progress_track, dark.progress_track, t),
        progress_fill: lerp_hsla(light.progress_fill, dark.progress_fill, t),
        badge_stable_bg: lerp_hsla(light.badge_stable_bg, dark.badge_stable_bg, t),
        badge_stable_text: lerp_hsla(light.badge_stable_text, dark.badge_stable_text, t),
        badge_beta_bg: lerp_hsla(light.badge_beta_bg, dark.badge_beta_bg, t),
        badge_beta_text: lerp_hsla(light.badge_beta_text, dark.badge_beta_text, t),
        btn_ghost_text: lerp_hsla(light.btn_ghost_text, dark.btn_ghost_text, t),
        btn_ghost_hover_bg: lerp_hsla(light.btn_ghost_hover_bg, dark.btn_ghost_hover_bg, t),
        btn_primary_text: lerp_hsla(light.btn_primary_text, dark.btn_primary_text, t),
        stat_blue_bg: lerp_hsla(light.stat_blue_bg, dark.stat_blue_bg, t),
        stat_blue_text: lerp_hsla(light.stat_blue_text, dark.stat_blue_text, t),
        stat_orange_bg: lerp_hsla(light.stat_orange_bg, dark.stat_orange_bg, t),
        stat_orange_text: lerp_hsla(light.stat_orange_text, dark.stat_orange_text, t),
        stat_green_bg: lerp_hsla(light.stat_green_bg, dark.stat_green_bg, t),
        stat_green_text: lerp_hsla(light.stat_green_text, dark.stat_green_text, t),
        window_control_normal: lerp_hsla(
            light.window_control_normal,
            dark.window_control_normal,
            t,
        ),
        window_control_hover_bg: lerp_hsla(
            light.window_control_hover_bg,
            dark.window_control_hover_bg,
            t,
        ),
        window_control_disabled: lerp_hsla(
            light.window_control_disabled,
            dark.window_control_disabled,
            t,
        ),
    };

    if let Some(accent) = accent_override {
        let (light_accent, light_hover, light_glow) = derive_accent(accent, false);
        let (dark_accent, dark_hover, dark_glow) = derive_accent(accent, true);

        out.accent = lerp_hsla(light_accent, dark_accent, t);
        out.accent_hover = lerp_hsla(light_hover, dark_hover, t);
        out.accent_glow = lerp_hsla(light_glow, dark_glow, t);
        out.progress_fill = out.accent;
    }

    out
}

fn derive_accent(accent: Hsla, dark: bool) -> (Hsla, Hsla, Hsla) {
    let mut accent = Hsla { a: 1.0, ..accent };

    // If the user picks a very dark/light color, nudge it into a usable band.
    if dark {
        accent.l = accent.l.clamp(0.45, 0.72);
    } else {
        accent.l = accent.l.clamp(0.35, 0.65);
    }
    accent.s = accent.s.clamp(0.20, 0.95);

    let hover_l = if dark {
        (accent.l - 0.08).clamp(0.15, 0.90)
    } else {
        (accent.l - 0.10).clamp(0.10, 0.85)
    };

    let hover = Hsla {
        l: hover_l,
        ..accent
    };
    let glow = Hsla {
        a: if dark { 0.55 } else { 0.40 },
        ..accent
    };
    (accent, hover, glow)
}

/// 插值 HSLA 颜色
/// 注意：色相 (h) 会沿着最短路径插值（例如 350°→10° 会经过 0°，而不是 350°→360°→10°）
fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    // 计算色相的最短路径
    let mut h_diff = b.h - a.h;
    if h_diff > 180.0 {
        h_diff -= 360.0;
    } else if h_diff < -180.0 {
        h_diff += 360.0;
    }

    Hsla {
        h: (a.h + h_diff * t).rem_euclid(360.0),
        s: a.s + (b.s - a.s) * t,
        l: a.l + (b.l - a.l) * t,
        a: a.a + (b.a - a.a) * t,
    }
}
