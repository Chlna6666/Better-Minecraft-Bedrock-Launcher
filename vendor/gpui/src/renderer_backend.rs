use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Maximum frame rate GPUI allows for continuous window composition.
pub const MAX_WINDOW_COMPOSITION_FPS: f32 = 240.0;
const MIN_WINDOW_COMPOSITION_FPS: f32 = 1.0;

/// Runtime renderer backend preference for GPUI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum RendererBackend {
    /// Use GPUI's platform default renderer.
    #[default]
    Auto,
    /// Prefer the nova-gfx Vulkan renderer.
    NovaVulkan,
    /// Prefer the nova-gfx DX12 renderer.
    NovaDx12,
    /// Prefer the nova-gfx Metal renderer.
    NovaMetal,
    /// Use the test/headless renderer.
    HeadlessTest,
}

/// GPU adapter power preference for renderers that can choose an adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RendererOptions {
    /// Backend preference for platform startup.
    pub backend: RendererBackend,
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
}

impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            backend: RendererBackend::Auto,
            adapter_name: None,
            power_preference: GpuPowerPreference::AutoLowPower,
            present_mode: PresentModePreference::AutoVsync,
            submission_mode: GpuSubmissionMode::Deferred,
            render_policy: RenderPolicy::EventDriven,
            frame_metrics: false,
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

    /// Returns GPUI's platform default renderer backend.
    pub fn platform_default() -> Self {
        #[cfg(all(target_os = "windows", feature = "nova-gfx-dx12"))]
        {
            Self::NovaDx12
        }
        #[cfg(all(
            target_os = "windows",
            not(feature = "nova-gfx-dx12"),
            feature = "nova-gfx-vulkan"
        ))]
        {
            Self::NovaVulkan
        }
        #[cfg(all(
            target_os = "windows",
            not(feature = "nova-gfx-dx12"),
            not(feature = "nova-gfx-vulkan")
        ))]
        {
            Self::Auto
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            Self::NovaVulkan
        }
        #[cfg(target_os = "macos")]
        {
            Self::NovaMetal
        }
        #[cfg(all(
            not(target_os = "windows"),
            not(target_os = "linux"),
            not(target_os = "freebsd"),
            not(target_os = "macos")
        ))]
        {
            Self::Auto
        }
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
            "dx12" | "directx" | "directx12" | "d3d12" | "dx11" | "directx11" | "d3d11"
            | "nova-dx12" | "nova_dx12" | "nova-directx12" | "nova-d3d12" => Ok(Self::NovaDx12),
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_and_display() {
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
    fn gpu_submission_mode_defaults_to_deferred() {
        assert_eq!(
            RendererOptions::default().submission_mode,
            GpuSubmissionMode::Deferred
        );
    }

    #[test]
    fn platform_default_backend_is_expected_for_target() {
        #[cfg(all(target_os = "windows", feature = "nova-gfx-dx12"))]
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::NovaDx12
        );

        #[cfg(all(
            target_os = "windows",
            not(feature = "nova-gfx-dx12"),
            feature = "nova-gfx-vulkan"
        ))]
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::NovaVulkan
        );

        #[cfg(all(
            target_os = "windows",
            not(feature = "nova-gfx-dx12"),
            not(feature = "nova-gfx-vulkan")
        ))]
        assert_eq!(RendererBackend::platform_default(), RendererBackend::Auto);

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        assert_eq!(
            RendererBackend::platform_default(),
            RendererBackend::NovaVulkan
        );

        #[cfg(target_os = "macos")]
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
    fn environment_override_takes_precedence() {
        let _lock = ENV_LOCK.lock().unwrap();
        // SAFETY: This test holds a process-wide mutex while mutating the environment.
        unsafe { std::env::set_var(RendererBackend::ENV_VAR, "nova-dx12") };
        assert_eq!(RendererBackend::Auto.resolve(), RendererBackend::NovaDx12);
        // SAFETY: This test holds a process-wide mutex while mutating the environment.
        unsafe { std::env::remove_var(RendererBackend::ENV_VAR) };
    }
}
