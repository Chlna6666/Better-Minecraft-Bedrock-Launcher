#![expect(
    unsafe_code,
    reason = "system text rendering parameters are read through platform FFI"
)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PackedSubpixelParameters(u32);

impl PackedSubpixelParameters {
    const BGR_BIT: u32 = 1 << 31;
    const VALUE_BITS: u32 = !Self::BGR_BIT;

    fn new(is_bgr: bool, clear_type_level: f32) -> Self {
        let clear_type_level = if clear_type_level.is_finite() {
            clear_type_level.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut bits = clear_type_level.to_bits() & Self::VALUE_BITS;
        if is_bgr {
            bits |= Self::BGR_BIT;
        }
        Self(bits)
    }
}

impl From<PackedSubpixelParameters> for u32 {
    fn from(value: PackedSubpixelParameters) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RenderingParameters {
    pub(super) gamma_ratios: [f32; 4],
    pub(super) grayscale_enhanced_contrast: f32,
    pub(super) subpixel_enhanced_contrast: f32,
    /// Packed shader word kept in the existing `is_bgr` ABI slot: bit 31 is the BGR flag and
    /// bits 0..30 are the exact non-negative f32 bit pattern of DirectWrite ClearTypeLevel.
    pub(super) is_bgr: PackedSubpixelParameters,
    windows_hwnd: Option<isize>,
    windows_monitor: Option<isize>,
}

impl RenderingParameters {
    pub(super) fn from_env() -> Self {
        Self::from_system(system_rendering_parameters())
    }

    #[cfg(target_os = "windows")]
    pub(super) fn from_env_for_window(hwnd: isize) -> Self {
        let monitor = monitor_for_window(hwnd);
        let system = monitor
            .and_then(system_rendering_parameters_for_monitor)
            .unwrap_or_else(system_rendering_parameters);
        let mut parameters = Self::from_system(system);
        parameters.windows_hwnd = Some(hwnd);
        parameters.windows_monitor = monitor;
        parameters
    }

    #[cfg(target_os = "windows")]
    pub(super) fn refresh_for_current_monitor(&mut self) -> bool {
        let Some(hwnd) = self.windows_hwnd else {
            return false;
        };
        let Some(monitor) = monitor_for_window(hwnd) else {
            return false;
        };
        if self.windows_monitor == Some(monitor) {
            return false;
        }

        let system = system_rendering_parameters_for_monitor(monitor)
            .unwrap_or_else(system_rendering_parameters);
        let mut next = Self::from_system(system);
        next.windows_hwnd = Some(hwnd);
        next.windows_monitor = Some(monitor);
        let changed = !self.same_visual_parameters(&next);
        *self = next;
        changed
    }

    fn same_visual_parameters(&self, other: &Self) -> bool {
        self.gamma_ratios == other.gamma_ratios
            && self.grayscale_enhanced_contrast == other.grayscale_enhanced_contrast
            && self.subpixel_enhanced_contrast == other.subpixel_enhanced_contrast
            && self.is_bgr == other.is_bgr
    }

    fn from_system(system: SystemRenderingParameters) -> Self {
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
        let clear_type_level = std::env::var("ZED_FONTS_CLEARTYPE_LEVEL")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(system.clear_type_level)
            .clamp(0.0, 1.0);
        Self {
            gamma_ratios: gamma_ratios(gamma),
            grayscale_enhanced_contrast,
            subpixel_enhanced_contrast,
            is_bgr: PackedSubpixelParameters::new(system.is_bgr, clear_type_level),
            windows_hwnd: None,
            windows_monitor: None,
        }
    }
}

#[derive(Clone, Copy)]
struct SystemRenderingParameters {
    gamma: f32,
    grayscale_enhanced_contrast: f32,
    subpixel_enhanced_contrast: f32,
    clear_type_level: f32,
    is_bgr: bool,
}

impl Default for SystemRenderingParameters {
    fn default() -> Self {
        Self {
            gamma: 1.45,
            grayscale_enhanced_contrast: 0.35,
            subpixel_enhanced_contrast: 0.5,
            clear_type_level: 1.0,
            is_bgr: false,
        }
    }
}

#[cfg(target_os = "windows")]
fn monitor_for_window(hwnd: isize) -> Option<isize> {
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow},
    };

    let hwnd = HWND(hwnd as *mut _);
    if hwnd.is_invalid() {
        return None;
    }
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    (!monitor.is_invalid()).then_some(monitor.0 as isize)
}

#[cfg(target_os = "windows")]
fn system_rendering_parameters() -> SystemRenderingParameters {
    create_system_rendering_parameters(None).unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn system_rendering_parameters_for_monitor(monitor: isize) -> Option<SystemRenderingParameters> {
    create_system_rendering_parameters(Some(monitor))
}

#[cfg(target_os = "windows")]
fn create_system_rendering_parameters(monitor: Option<isize>) -> Option<SystemRenderingParameters> {
    use windows::{
        Win32::Graphics::{
            DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_PIXEL_GEOMETRY_BGR, DWriteCreateFactory,
                IDWriteFactory5, IDWriteRenderingParams1,
            },
            Gdi::HMONITOR,
        },
        core::Interface,
    };

    let factory: IDWriteFactory5 = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()? };
    let render_params: IDWriteRenderingParams1 = match monitor {
        Some(monitor) => unsafe {
            factory
                .CreateMonitorRenderingParams(HMONITOR(monitor as *mut _))
                .ok()?
        },
        None => unsafe { factory.CreateRenderingParams().ok()? },
    }
    .cast()
    .ok()?;
    Some(SystemRenderingParameters {
        gamma: unsafe { render_params.GetGamma() },
        grayscale_enhanced_contrast: unsafe { render_params.GetGrayscaleEnhancedContrast() },
        subpixel_enhanced_contrast: unsafe { render_params.GetEnhancedContrast() },
        clear_type_level: unsafe { render_params.GetClearTypeLevel() },
        is_bgr: unsafe { render_params.GetPixelGeometry() } == DWRITE_PIXEL_GEOMETRY_BGR,
    })
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

#[cfg(test)]
mod tests {
    use super::PackedSubpixelParameters;

    fn unpack(value: PackedSubpixelParameters) -> (bool, f32) {
        let packed = u32::from(value);
        let is_bgr = packed & PackedSubpixelParameters::BGR_BIT != 0;
        let clear_type_level =
            f32::from_bits(packed & PackedSubpixelParameters::VALUE_BITS);
        (is_bgr, clear_type_level)
    }

    #[test]
    fn packed_subpixel_parameters_preserve_geometry_and_level_exactly() {
        for (is_bgr, clear_type_level) in [
            (false, 0.0_f32),
            (false, 0.375_f32),
            (false, 1.0_f32),
            (true, 0.0_f32),
            (true, 0.625_f32),
            (true, 1.0_f32),
        ] {
            let (decoded_bgr, decoded_level) =
                unpack(PackedSubpixelParameters::new(is_bgr, clear_type_level));
            assert_eq!(decoded_bgr, is_bgr);
            assert_eq!(decoded_level.to_bits(), clear_type_level.to_bits());
        }
    }

    #[test]
    fn packed_subpixel_parameters_normalize_invalid_levels() {
        assert_eq!(unpack(PackedSubpixelParameters::new(false, -1.0)).1, 0.0);
        assert_eq!(unpack(PackedSubpixelParameters::new(false, 2.0)).1, 1.0);
        assert_eq!(
            unpack(PackedSubpixelParameters::new(false, f32::NAN)).1,
            1.0
        );
        assert_eq!(
            unpack(PackedSubpixelParameters::new(true, f32::INFINITY)),
            (true, 1.0)
        );
    }
}
