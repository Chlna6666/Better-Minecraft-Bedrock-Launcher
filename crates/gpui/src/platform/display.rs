use std::fmt::{self, Debug};

use anyhow::Result;
use uuid::Uuid;

use crate::{Bounds, DEFAULT_WINDOW_SIZE, Pixels, point};

/// Converts a nominal refresh rate to an interval, rejecting unknown or unrepresentable rates.
pub fn refresh_interval_from_hz(hertz: f64) -> Option<std::time::Duration> {
    if !hertz.is_finite() || hertz <= 0.0 {
        return None;
    }
    std::time::Duration::try_from_secs_f64(1.0 / hertz)
        .ok()
        .filter(|interval| !interval.is_zero())
}

/// A change to connected displays, reported by [`crate::App::observe_displays`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DisplayEvent {
    /// A display was connected.
    Added(DisplayId),
    /// A display was disconnected.
    Removed(DisplayId),
    /// A connected display's properties changed.
    Changed {
        /// The display that changed.
        id: DisplayId,
        /// The properties that changed.
        changes: DisplayChanges,
    },
}

bitflags::bitflags! {
    /// Properties reported by [`DisplayEvent::Changed`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct DisplayChanges: u8 {
        /// Display bounds changed.
        const BOUNDS = 1 << 0;
        /// The nominal refresh interval changed.
        const REFRESH_INTERVAL = 1 << 1;
    }
}

/// A handle to a platform's display, e.g. a monitor or laptop screen.
pub trait PlatformDisplay: Send + Sync + Debug {
    /// Nominal refresh period reported by the display's current mode, if available.
    /// This is a hardware timing target, not a measurement of application presentation or VRR.
    fn refresh_interval(&self) -> Option<std::time::Duration> {
        None
    }

    /// Get the ID for this display
    fn id(&self) -> DisplayId;

    /// Returns a stable identifier for this display that can be persisted and used
    /// across system restarts.
    fn uuid(&self) -> Result<Uuid>;

    /// Get the bounds for this display
    fn bounds(&self) -> Bounds<Pixels>;

    /// Get the visible work area for this display, excluding taskbars, docks, and menu bars.
    ///
    /// Platforms without a distinct work area may use the full display bounds.
    fn visible_bounds(&self) -> Bounds<Pixels> {
        self.bounds()
    }

    /// Get the default bounds for this display to place a window
    fn default_bounds(&self) -> Bounds<Pixels> {
        let bounds = self.bounds();
        let center = bounds.center();
        let clipped_window_size = DEFAULT_WINDOW_SIZE.min(&bounds.size);

        let offset = clipped_window_size / 2.0;
        let origin = point(center.x - offset.width, center.y - offset.height);
        Bounds::new(origin, clipped_window_size)
    }
}

/// An opaque identifier for a hardware display
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub struct DisplayId(pub(crate) u32);

impl From<DisplayId> for u32 {
    fn from(id: DisplayId) -> Self {
        id.0
    }
}

impl Debug for DisplayId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DisplayId({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_intervals_reject_unknown_and_unrepresentable_rates() {
        assert_eq!(
            refresh_interval_from_hz(50.),
            Some(std::time::Duration::from_millis(20))
        );
        assert_eq!(
            refresh_interval_from_hz(120.),
            std::time::Duration::try_from_secs_f64(1. / 120.).ok()
        );
        for rate in [0., -60., f64::NAN, f64::INFINITY, 1e-300, 1e300] {
            assert_eq!(refresh_interval_from_hz(rate), None);
        }
    }
}
