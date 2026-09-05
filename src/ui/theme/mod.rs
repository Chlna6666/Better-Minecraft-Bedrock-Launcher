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

/// Shared BMCBL glass style.
///
/// Sampling quality is selected by GPUI from the blur sigma. BMCBL deliberately does not pin a
/// downsample factor or pass count here, so backend quality policy can evolve without duplicating
/// renderer tuning in every application surface.
pub fn glass_backdrop_blur_style() -> gpui::BackdropBlurStyle {
    gpui::BackdropBlurStyle::new(gpui::px(GLASS_BACKDROP_BLUR_SIGMA_PX)).auto_quality()
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
