// Theme scaffolding.
//
// We'll migrate hard-coded colors from the UI components into this module as the UI stabilizes.

pub mod colors;
pub mod tokens;

pub use colors::{
    DarkColors, LightColors, ThemeColors, lerp_theme_colors, parse_hex_color_to_hsla,
};

use std::sync::OnceLock;

/// Shared sigma for BMCBL glass surfaces.
///
/// GPUI backdrop blur follows CSS semantics: this is Gaussian sigma, not the old
/// three-sigma support radius. The previous `6px` glass values therefore map to
/// approximately `2px` here.
pub const GLASS_BACKDROP_BLUR_SIGMA_PX: f32 = 2.0;

/// Stable, low-bandwidth glass configuration used by the application chrome and popovers.
/// Keeping one layout across BMCBL UI surfaces lets Nova reuse the same target geometry instead
/// of creating a different blur variant for every component.
pub fn glass_backdrop_blur_style() -> gpui::BackdropBlurStyle {
    gpui::BackdropBlurStyle::new(gpui::px(GLASS_BACKDROP_BLUR_SIGMA_PX))
        .downsample(2)
        .levels(2)
}

/// 缓存的浅色主题色板（`LightColors::colors()` 每次调用都会重建，渲染热路径改用本函数）。
pub fn light_colors() -> &'static ThemeColors {
    static LIGHT: OnceLock<ThemeColors> = OnceLock::new();
    LIGHT.get_or_init(LightColors::colors)
}

/// 缓存的深色主题色板。
pub fn dark_colors() -> &'static ThemeColors {
    static DARK: OnceLock<ThemeColors> = OnceLock::new();
    DARK.get_or_init(DarkColors::colors)
}

/// 读取当前主题插值后的色板。
///
/// 收敛各视图重复的“读 ThemeState → factor → lerp”样板；主题动画进行中
/// 会返回当帧的插值结果。
pub fn theme_colors(cx: &gpui::App) -> ThemeColors {
    let theme = cx.global::<crate::ui::state::theme::ThemeState>();
    lerp_theme_colors(
        light_colors(),
        dark_colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    )
}
