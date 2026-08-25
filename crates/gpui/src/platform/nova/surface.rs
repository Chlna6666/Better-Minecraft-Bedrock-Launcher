use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SurfaceAlphaState {
    pub(super) swapchain_mode: CompositeAlphaMode,
    pub(super) output_mode: SurfaceOutputMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SurfaceOutputMode {
    Straight,
    Premultiplied,
}

pub(super) fn clear_color() -> ClearColor {
    ClearColor {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 0.0,
    }
}

impl SurfaceAlphaState {
    #[cfg(any(test, feature = "nova-gfx-vulkan", target_os = "macos"))]
    pub(super) fn new(swapchain_mode: CompositeAlphaMode) -> Self {
        let output_mode = if matches!(
            swapchain_mode,
            CompositeAlphaMode::Premultiplied | CompositeAlphaMode::Inherit
        ) {
            SurfaceOutputMode::Premultiplied
        } else {
            SurfaceOutputMode::Straight
        };
        Self {
            swapchain_mode,
            output_mode,
        }
    }

    pub(super) fn for_window_transparency(is_transparent: bool) -> Self {
        if is_transparent {
            Self {
                swapchain_mode: CompositeAlphaMode::Premultiplied,
                output_mode: SurfaceOutputMode::Premultiplied,
            }
        } else {
            Self {
                swapchain_mode: CompositeAlphaMode::Opaque,
                output_mode: SurfaceOutputMode::Straight,
            }
        }
    }

    pub(super) fn outputs_premultiplied_alpha(self) -> bool {
        matches!(self.output_mode, SurfaceOutputMode::Premultiplied)
    }
}
