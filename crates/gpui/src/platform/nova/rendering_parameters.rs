#![expect(
    unsafe_code,
    reason = "system text rendering parameters are read through platform FFI"
)]

pub(super) struct RenderingParameters {
    pub(super) gamma_ratios: [f32; 4],
    pub(super) grayscale_enhanced_contrast: f32,
    pub(super) subpixel_enhanced_contrast: f32,
    pub(super) is_bgr: bool,
}

impl RenderingParameters {
    pub(super) fn from_env() -> Self {
        let system = system_rendering_parameters();
        let gamma = std::env::var("ZED_FONTS_GAMMA")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(system.gamma)
            .clamp(1.0, 2.2);
        let grayscale_enhanced_contrast = std::env::var("ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(system.grayscale_enhanced_contrast)
            .max(0.0);
        let subpixel_enhanced_contrast = std::env::var("ZED_FONTS_SUBPIXEL_ENHANCED_CONTRAST")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(system.subpixel_enhanced_contrast)
            .max(0.0);
        Self {
            gamma_ratios: gamma_ratios(gamma),
            grayscale_enhanced_contrast,
            subpixel_enhanced_contrast,
            is_bgr: system.is_bgr,
        }
    }
}

#[derive(Clone, Copy)]
struct SystemRenderingParameters {
    gamma: f32,
    grayscale_enhanced_contrast: f32,
    subpixel_enhanced_contrast: f32,
    is_bgr: bool,
}

impl Default for SystemRenderingParameters {
    fn default() -> Self {
        Self {
            gamma: 1.45,
            grayscale_enhanced_contrast: 0.35,
            subpixel_enhanced_contrast: 0.5,
            is_bgr: false,
        }
    }
}

#[cfg(target_os = "windows")]
fn system_rendering_parameters() -> SystemRenderingParameters {
    use windows::{
        Win32::Graphics::DirectWrite::{
            DWRITE_FACTORY_TYPE_SHARED, DWRITE_PIXEL_GEOMETRY_BGR, DWriteCreateFactory,
            IDWriteFactory5, IDWriteRenderingParams1,
        },
        core::Interface,
    };

    let parameters = (|| -> Option<SystemRenderingParameters> {
        let factory: IDWriteFactory5 =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()? };
        let render_params: IDWriteRenderingParams1 =
            unsafe { factory.CreateRenderingParams().ok()? }
                .cast()
                .ok()?;
        Some(SystemRenderingParameters {
            gamma: unsafe { render_params.GetGamma() },
            grayscale_enhanced_contrast: unsafe { render_params.GetGrayscaleEnhancedContrast() },
            subpixel_enhanced_contrast: unsafe { render_params.GetEnhancedContrast() },
            is_bgr: unsafe { render_params.GetPixelGeometry() } == DWRITE_PIXEL_GEOMETRY_BGR,
        })
    })();

    parameters.unwrap_or_default()
}

#[cfg(not(target_os = "windows"))]
fn system_rendering_parameters() -> SystemRenderingParameters {
    SystemRenderingParameters::default()
}

fn gamma_ratios(gamma: f32) -> [f32; 4] {
    const GAMMA_INCORRECT_TARGET_RATIOS: [[f32; 4]; 13] = [
        [0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0],
        [0.0166 / 4.0, -0.0807 / 4.0, 0.2227 / 4.0, -0.0751 / 4.0],
        [0.0350 / 4.0, -0.1760 / 4.0, 0.4325 / 4.0, -0.1370 / 4.0],
        [0.0543 / 4.0, -0.2821 / 4.0, 0.6302 / 4.0, -0.1876 / 4.0],
        [0.0739 / 4.0, -0.3963 / 4.0, 0.8167 / 4.0, -0.2287 / 4.0],
        [0.0933 / 4.0, -0.5161 / 4.0, 0.9926 / 4.0, -0.2616 / 4.0],
        [0.1121 / 4.0, -0.6395 / 4.0, 1.1588 / 4.0, -0.2877 / 4.0],
        [0.1300 / 4.0, -0.7649 / 4.0, 1.3159 / 4.0, -0.3080 / 4.0],
        [0.1469 / 4.0, -0.8911 / 4.0, 1.4644 / 4.0, -0.3234 / 4.0],
        [0.1627 / 4.0, -1.0170 / 4.0, 1.6051 / 4.0, -0.3347 / 4.0],
        [0.1773 / 4.0, -1.1420 / 4.0, 1.7385 / 4.0, -0.3426 / 4.0],
        [0.1908 / 4.0, -1.2652 / 4.0, 1.8650 / 4.0, -0.3476 / 4.0],
        [0.2031 / 4.0, -1.3864 / 4.0, 1.9851 / 4.0, -0.3501 / 4.0],
    ];
    const NORM13: f32 = ((0x10000 as f64) / (255.0 * 255.0) * 4.0) as f32;
    const NORM24: f32 = ((0x100 as f64) / 255.0 * 4.0) as f32;
    let index = ((gamma * 10.0).round() as usize).clamp(10, 22) - 10;
    let ratios = GAMMA_INCORRECT_TARGET_RATIOS[index];
    [
        ratios[0] * NORM13,
        ratios[1] * NORM24,
        ratios[2] * NORM13,
        ratios[3] * NORM24,
    ]
}
