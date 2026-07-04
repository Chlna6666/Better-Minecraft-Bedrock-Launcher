use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NovaSurfaceAlphaState {
    pub(super) swapchain_mode: CompositeAlphaMode,
    pub(super) output_mode: NovaSurfaceOutputMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NovaSurfaceOutputMode {
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

impl NovaSurfaceAlphaState {
    #[cfg(test)]
    pub(super) fn new(swapchain_mode: CompositeAlphaMode) -> Self {
        let output_mode = if matches!(swapchain_mode, CompositeAlphaMode::Premultiplied) {
            NovaSurfaceOutputMode::Premultiplied
        } else {
            NovaSurfaceOutputMode::Straight
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
                output_mode: NovaSurfaceOutputMode::Premultiplied,
            }
        } else {
            Self {
                swapchain_mode: CompositeAlphaMode::Opaque,
                output_mode: NovaSurfaceOutputMode::Straight,
            }
        }
    }

    pub(super) fn outputs_premultiplied_alpha(self) -> bool {
        matches!(self.output_mode, NovaSurfaceOutputMode::Premultiplied)
    }
}
