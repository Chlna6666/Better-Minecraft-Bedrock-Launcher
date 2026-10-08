use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf, str::FromStr};

/// Maximum frame rate GPUI allows for continuous window composition.
pub const MAX_WINDOW_COMPOSITION_FPS: f32 = 240.0;
const MIN_WINDOW_COMPOSITION_FPS: f32 = 1.0;

/// Runtime renderer backend preference for GPUI.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum RendererBackend {
    /// Use GPUI's platform default renderer.
    #[default]
    Auto,
    /// Prefer the nova-gfx Vulkan renderer.
    NovaVulkan,
    /// Prefer the native OpenGL 4.5 renderer on Windows or Linux.
    NovaOpenGl,
    /// Prefer the native nova-gfx Direct3D 11 renderer.
    NovaDx11,
    /// Prefer the nova-gfx DX12 renderer.
    NovaDx12,
    /// Prefer the nova-gfx Metal renderer.
    NovaMetal,
    /// Use the test/headless renderer.
    HeadlessTest,
}

/// Text coverage formats a renderer can composite correctly.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum TextRasterizationMode {
    /// Composite one coverage value equally across all color channels.
    #[default]
    Grayscale,
    /// Composite independent red, green, and blue coverage values.
    RgbSubpixel,
}

/// Rendering capabilities that affect platform resource generation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RendererCapabilities {
    /// Text coverage format supported by the renderer's atlas and blend pipeline.
    pub text_rasterization: TextRasterizationMode,
}

/// GPU adapter power preference for renderers that can choose an adapter.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum GpuPowerPreference {
    /// Prefer low idle power and let the backend pick the most efficient adapter.
    #[default]
    AutoLowPower,
    /// Prefer a high-performance adapter for animation-heavy or 3D-heavy windows.
    HighPerformance,
}

/// Swap-chain present mode preference.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum PresentModePreference {
    /// Prefer vblank-paced presentation.
    #[default]
    AutoVsync,
    /// Prefer low-latency mailbox presentation where available.
    Mailbox,
    /// Present immediately where the backend supports it.
    Immediate,
}

/// GPU submission policy for renderers with explicit submission handles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum GpuSubmissionMode {
    /// Submit work and let the renderer defer completion waits where the backend can track fences.
    #[default]
    Deferred,
    /// Wait for frame GPU work before returning from the renderer draw call.
    Synchronous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GpuiMemoryTrimLevel {
    Light,
    Moderate,
    Aggressive,
}

/// Default rendering policy for application windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum RenderPolicy {
    /// Render only in response to invalidation, presentation, or animation requests.
    #[default]
    EventDriven,
    /// Render continuously at the requested maximum frame rate.
    Continuous {
        /// Maximum frame rate while continuous rendering is active.
        max_fps: f32,
    },
    /// Render only when explicitly requested by the application.
    OnDemand,
}

impl RenderPolicy {
    /// Returns a policy with continuous composition bounded to GPUI's supported range.
    pub fn clamped(self) -> Self {
        match self {
            Self::Continuous { max_fps } => Self::Continuous {
                max_fps: max_fps.clamp(MIN_WINDOW_COMPOSITION_FPS, MAX_WINDOW_COMPOSITION_FPS),
            },
            Self::EventDriven | Self::OnDemand => self,
        }
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by renderer policy tests and diagnostics")
    )]
    pub(crate) fn continuous_frame_interval_ms(self) -> Option<u32> {
        let Self::Continuous { max_fps } = self.clamped() else {
            return None;
        };
        Some((1_000.0 / max_fps).ceil().max(1.0) as u32)
    }
}

/// Renderer startup options.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum RendererFallback {
    /// Try other compiled platform backends after complete initialization fails.
    /// A pinned adapter remains strict regardless of this policy.
    #[default]
    Available,
    /// Initialize only the requested backend (or the platform default for `Auto`).
    Disabled,
}

/// Renderer startup options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RendererOptions {
    /// Backend preference for platform startup.
    pub backend: RendererBackend,
    /// Controls initialization fallback. Existing serialized options allow fallback.
    #[serde(default)]
    pub fallback: RendererFallback,
    /// Exact adapter name to prefer when the backend can enumerate GPU adapters.
    pub adapter_name: Option<String>,
    /// GPU adapter preference when the backend can choose between adapters.
    pub power_preference: GpuPowerPreference,
    /// Swap-chain present mode preference.
    pub present_mode: PresentModePreference,
    /// GPU submission mode for supported renderers.
    pub submission_mode: GpuSubmissionMode,
    /// Default rendering policy for new windows.
    pub render_policy: RenderPolicy,
    /// Enables extra frame metrics for debugging and profiling.
    pub frame_metrics: bool,
    /// Optional persistent nova-gfx pipeline-cache root.
    #[serde(default)]
    pub pipeline_cache_dir: Option<PathBuf>,
}

impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            backend: RendererBackend::Auto,
            fallback: RendererFallback::Available,
            adapter_name: None,
            power_preference: GpuPowerPreference::AutoLowPower,
            present_mode: PresentModePreference::AutoVsync,
            submission_mode: GpuSubmissionMode::Deferred,
            render_policy: RenderPolicy::EventDriven,
            frame_metrics: false,
            pipeline_cache_dir: None,
        }
    }
}

impl RendererOptions {
    /// Returns options using the supplied backend and default low-idle renderer policy.
    pub fn with_backend(backend: RendererBackend) -> Self {
        Self {
            backend,
            ..Self::default()
        }
    }

    /// Ordered initialization candidates. Explicit adapter selection forbids fallback.
    pub(crate) fn candidates(&self, resolved: RendererBackend) -> Vec<RendererBackend> {
        let resolved = if resolved == RendererBackend::Auto {
            RendererBackend::platform_default()
        } else {
            resolved
        };
        let mut candidates = vec![resolved];
        if self.fallback == RendererFallback::Available && self.adapter_name.is_none() {
            for backend in RendererBackend::available_backends() {
                if !candidates.contains(backend) {
                    candidates.push(*backend);
                }
            }
        }
        candidates
    }

    /// Resolves the backend against the environment override.
    pub fn resolve(mut self) -> Self {
        self.backend = self.backend.resolve();
        self.adapter_name = self.adapter_name.and_then(|name| match name.trim() {
            "" => None,
            trimmed => Some(trimmed.to_string()),
        });
        self.render_policy = self.render_policy.clamped();
        self
    }
}

/// Device type reported by the renderer backend for a GPU adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GpuAdapterDeviceType {
    /// Unknown or backend-specific device type.
    Other,
    /// Integrated GPU sharing memory with the CPU.
    IntegratedGpu,
    /// Discrete GPU with dedicated graphics memory.
    DiscreteGpu,
    /// Virtual or hosted GPU.
    VirtualGpu,
    /// CPU or software renderer.
    Cpu,
}

/// Information about an available GPU adapter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GpuAdapterInfo {
    /// Adapter name reported by the backend.
    pub name: String,
    /// Backend that exposed this adapter.
    pub backend: RendererBackend,
    /// Adapter device type.
    pub device_type: GpuAdapterDeviceType,
    /// Backend-specific vendor ID.
    pub vendor: u32,
    /// Backend-specific device ID.
    pub device: u32,
    /// Driver name reported by the backend.
    pub driver: String,
    /// Driver details reported by the backend.
    pub driver_info: String,
}

impl RendererBackend {
    /// Environment variable used to override the renderer backend.
    pub const ENV_VAR: &'static str = "GPUI_RENDERER";

    /// Returns the resource-generation capabilities of this renderer backend.
    ///
    /// `Auto` reports conservative capabilities because its concrete backend is
    /// not known until platform startup resolves it.
    pub const fn capabilities(self) -> RendererCapabilities {
        let text_rasterization = match self {
            #[cfg(target_os = "windows")]
            Self::NovaOpenGl | Self::NovaVulkan | Self::NovaDx11 | Self::NovaDx12 => {
                TextRasterizationMode::RgbSubpixel
            }
            #[cfg(not(target_os = "windows"))]
            Self::NovaOpenGl | Self::NovaVulkan | Self::NovaDx11 | Self::NovaDx12 => {
                TextRasterizationMode::Grayscale
            }
            Self::Auto | Self::NovaMetal | Self::HeadlessTest => TextRasterizationMode::Grayscale,
        };
        RendererCapabilities { text_rasterization }
    }

    /// Returns GPUI's platform default renderer backend.
    pub fn platform_default() -> Self {
        Self::available_backends()
            .first()
            .copied()
            .unwrap_or(Self::Auto)
    }

    /// Compiled native backends, in platform fallback order.
    pub(crate) fn available_backends() -> &'static [Self] {
        &[
            #[cfg(all(target_os = "windows", feature = "nova-gfx-dx12"))]
            Self::NovaDx12,
            #[cfg(all(target_os = "windows", feature = "nova-gfx-dx11"))]
            Self::NovaDx11,
            #[cfg(all(
                any(target_os = "windows", target_os = "linux", target_os = "freebsd"),
                feature = "nova-gfx-vulkan"
            ))]
            Self::NovaVulkan,
            #[cfg(all(
                any(target_os = "windows", target_os = "linux"),
                feature = "nova-gfx-opengl"
            ))]
            Self::NovaOpenGl,
            #[cfg(all(target_os = "macos", feature = "nova-gfx-metal"))]
            Self::NovaMetal,
        ]
    }

    /// Reads [`Self::ENV_VAR`] and returns a parsed backend preference.
    pub fn from_env() -> Option<Self> {
        std::env::var(Self::ENV_VAR)
            .ok()
            .and_then(|value| value.parse().ok())
    }

    /// Resolves a builder preference against the environment override.
    pub fn resolve(self) -> Self {
        Self::from_env().unwrap_or(self)
    }

    /// Returns the environment string for this backend.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::NovaVulkan => "nova-vulkan",
            Self::NovaOpenGl => "nova-opengl",
            Self::NovaDx11 => "nova-dx11",
            Self::NovaDx12 => "nova-dx12",
            Self::NovaMetal => "nova-metal",
            Self::HeadlessTest => "headless",
        }
    }
}

impl FromStr for RendererBackend {
    type Err = RendererBackendParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "auto" | "default" => Ok(Self::Auto),
            "nova" | "blade" | "vk" | "vulkan" | "nova-vulkan" | "nova_vulkan" | "nova-vk"
            | "nova_vk" => Ok(Self::NovaVulkan),
            "gl" | "opengl" | "nova-opengl" | "nova_opengl" => Ok(Self::NovaOpenGl),
            "dx11" | "directx11" | "d3d11" | "nova-dx11" | "nova_dx11" => Ok(Self::NovaDx11),
            "dx12" | "directx" | "directx12" | "d3d12" | "nova-dx12" | "nova_dx12"
            | "nova-directx12" | "nova-d3d12" => Ok(Self::NovaDx12),
            "metal" | "mtl" | "nova-metal" | "nova_metal" | "nova-mtl" | "nova_mtl" => {
                Ok(Self::NovaMetal)
            }
            "headless" | "headless-test" | "test" => Ok(Self::HeadlessTest),
            other => Err(RendererBackendParseError {
                value: other.to_string(),
            }),
        }
    }
}

impl fmt::Display for RendererBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing a renderer backend preference fails.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RendererBackendParseError {
    value: String,
}

impl fmt::Display for RendererBackendParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown GPUI renderer backend '{}'", self.value)
    }
}

impl std::error::Error for RendererBackendParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opengl_aliases_and_disabled_fallback_are_unambiguous() {
        for alias in ["gl", "opengl", "nova-opengl", "nova_opengl"] {
            assert_eq!(
                alias.parse::<RendererBackend>().unwrap(),
                RendererBackend::NovaOpenGl
            );
        }
        let options = RendererOptions {
            fallback: RendererFallback::Disabled,
            ..RendererOptions::with_backend(RendererBackend::NovaOpenGl)
        };
        assert_eq!(
            options.candidates(RendererBackend::NovaOpenGl),
            [RendererBackend::NovaOpenGl]
        );
    }

    #[test]
    fn preferred_backend_is_first_and_fallbacks_are_compiled_and_unique() {
        let options = RendererOptions::with_backend(RendererBackend::NovaDx11);
        let candidates = options.candidates(RendererBackend::NovaDx11);
        assert_eq!(candidates[0], RendererBackend::NovaDx11);
        for backend in &candidates[1..] {
            assert!(RendererBackend::available_backends().contains(backend));
        }
        let mut unique = candidates.clone();
        unique.sort_by_key(|backend| backend.as_str());
        unique.dedup();
        assert_eq!(unique.len(), candidates.len());
        let pinned = RendererOptions {
            adapter_name: Some("adapter".into()),
            ..options
        };
        assert_eq!(
            pinned.candidates(RendererBackend::NovaDx11),
            [RendererBackend::NovaDx11]
        );
    }
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_and_display() {
        for alias in ["dx11", "d3d11", "directx11", "nova-dx11", "nova_dx11"] {
            assert_eq!(
                alias.parse::<RendererBackend>().unwrap(),
                RendererBackend::NovaDx11
            );
        }
        assert_eq!(RendererBackend::NovaDx11.to_string(), "nova-dx11");
        assert_eq!(
            "auto".parse::<RendererBackend>().unwrap(),
            RendererBackend::Auto
        );
        assert_eq!(
            "vulkan".parse::<RendererBackend>().unwrap(),
            RendererBackend::NovaVulkan
        );
        assert_eq!(
            "dx12".parse::<RendererBackend>().unwrap(),
            RendererBackend::NovaDx12
        );
        assert_eq!(
            "nova-vulkan".parse::<RendererBackend>().unwrap(),
            RendererBackend::NovaVulkan
        );
        assert_eq!(
            "nova-dx12".parse::<RendererBackend>().unwrap(),
            RendererBackend::NovaDx12
        );
        assert_eq!(
            "nova-metal".parse::<RendererBackend>().unwrap(),
            RendererBackend::NovaMetal
        );
        assert_eq!(RendererBackend::NovaVulkan.to_string(), "nova-vulkan");
        assert_eq!(RendererBackend::NovaDx12.to_string(), "nova-dx12");
        assert_eq!(RendererBackend::NovaMetal.to_string(), "nova-metal");
        assert_eq!(RendererBackend::HeadlessTest.to_string(), "headless");
    }

    #[test]
    fn renderer_options_default_to_event_driven_low_power() {
        let options = RendererOptions::default();

        assert_eq!(options.backend, RendererBackend::Auto);
        assert_eq!(options.adapter_name, None);
        assert_eq!(options.power_preference, GpuPowerPreference::AutoLowPower);
        assert_eq!(options.present_mode, PresentModePreference::AutoVsync);
        assert_eq!(options.submission_mode, GpuSubmissionMode::Deferred);
        assert_eq!(options.render_policy, RenderPolicy::EventDriven);
        assert!(!options.frame_metrics);
    }

    #[test]
    fn nova_backends_report_platform_text_rasterization() {
        #[cfg(target_os = "windows")]
        for backend in [
            RendererBackend::NovaOpenGl,
            RendererBackend::NovaVulkan,
            RendererBackend::NovaDx11,
            RendererBackend::NovaDx12,
        ] {
            assert_eq!(
                backend.capabilities().text_rasterization,
                TextRasterizationMode::RgbSubpixel
            );
        }
        #[cfg(not(target_os = "windows"))]
        for backend in [
            RendererBackend::NovaOpenGl,
            RendererBackend::NovaVulkan,
            RendererBackend::NovaDx11,
            RendererBackend::NovaDx12,
        ] {
            assert_eq!(
                backend.capabilities().text_rasterization,
                TextRasterizationMode::Grayscale
            );
        }
        assert_eq!(
            RendererBackend::NovaMetal.capabilities().text_rasterization,
            TextRasterizationMode::Grayscale
        );
    }

    #[test]
    fn gpu_submission_mode_defaults_to_deferred() {
        assert_eq!(
            RendererOptions::default().submission_mode,
            GpuSubmissionMode::Deferred
        );
    }

    #[test]
    fn platform_default_backend_is_expected_for_target() {
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::available_backends()
                .first()
                .copied()
                .unwrap_or(RendererBackend::Auto)
        );
        #[cfg(all(target_os = "windows", feature = "nova-gfx-dx12"))]
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::NovaDx12
        );

        #[cfg(all(
            target_os = "windows",
            not(feature = "nova-gfx-dx12"),
            feature = "nova-gfx-dx11"
        ))]
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::NovaDx11
        );

        #[cfg(all(
            target_os = "windows",
            not(feature = "nova-gfx-dx12"),
            not(feature = "nova-gfx-dx11"),
            feature = "nova-gfx-vulkan"
        ))]
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::NovaVulkan
        );

        #[cfg(all(
            target_os = "windows",
            not(feature = "nova-gfx-dx12"),
            not(feature = "nova-gfx-dx11"),
            not(feature = "nova-gfx-vulkan"),
            not(feature = "nova-gfx-opengl")
        ))]
        assert_eq!(RendererBackend::platform_default(), RendererBackend::Auto);

        #[cfg(all(
            any(target_os = "linux", target_os = "freebsd"),
            feature = "nova-gfx-vulkan"
        ))]
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::NovaVulkan
        );

        #[cfg(all(target_os = "macos", feature = "nova-gfx-metal"))]
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::NovaMetal
        );
    }

    #[test]
    fn continuous_render_policy_is_capped_to_window_composition_limit() {
        let options = RendererOptions {
            render_policy: RenderPolicy::Continuous { max_fps: 360.0 },
            ..RendererOptions::default()
        }
        .resolve();

        assert_eq!(
            options.render_policy,
            RenderPolicy::Continuous {
                max_fps: MAX_WINDOW_COMPOSITION_FPS
            }
        );
        assert_eq!(
            options.render_policy.continuous_frame_interval_ms(),
            Some(5)
        );
    }

    #[test]
    #[expect(
        unsafe_code,
        reason = "the test serializes process environment mutation with ENV_LOCK"
    )]
    fn environment_override_takes_precedence() {
        let _lock = ENV_LOCK.lock().unwrap();
        // SAFETY: This test holds a process-wide mutex while mutating the environment.
        unsafe { std::env::set_var(RendererBackend::ENV_VAR, "nova-dx12") };
        assert_eq!(RendererBackend::Auto.resolve(), RendererBackend::NovaDx12);
        // SAFETY: This test holds a process-wide mutex while mutating the environment.
        unsafe { std::env::remove_var(RendererBackend::ENV_VAR) };
    }
}
