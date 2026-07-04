use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Selects the subsystem responsible for ticking and applying an animation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum AnimationDriver {
    /// Select the fastest supported path for the requested properties.
    #[default]
    Auto,
    /// Use GPU-side interpolation for supported visual primitives.
    Gpu,
    /// Repaint only the affected visual region on the CPU.
    Paint,
    /// Recompute layout and rerender the owning view.
    Layout,
}

pub(crate) fn next_visual_driver(driver: AnimationDriver) -> AnimationDriver {
    match driver {
        AnimationDriver::Gpu => AnimationDriver::Gpu,
        AnimationDriver::Auto | AnimationDriver::Paint | AnimationDriver::Layout => {
            AnimationDriver::Paint
        }
    }
}

pub(crate) fn merge_requested_drivers(
    current: Option<AnimationDriver>,
    requested: AnimationDriver,
) -> AnimationDriver {
    let requested = if matches!(requested, AnimationDriver::Layout) {
        AnimationDriver::Layout
    } else {
        next_visual_driver(requested)
    };
    match current {
        None => requested,
        Some(AnimationDriver::Auto) => AnimationDriver::Auto,
        Some(current)
            if matches!(current, AnimationDriver::Layout)
                != matches!(requested, AnimationDriver::Layout) =>
        {
            AnimationDriver::Auto
        }
        Some(AnimationDriver::Gpu) | Some(AnimationDriver::Paint) => {
            if matches!(requested, AnimationDriver::Gpu) {
                AnimationDriver::Gpu
            } else {
                AnimationDriver::Paint
            }
        }
        Some(AnimationDriver::Layout) => AnimationDriver::Layout,
    }
}
