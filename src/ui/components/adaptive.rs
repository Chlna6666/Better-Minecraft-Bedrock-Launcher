use gpui::{Pixels, px};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdaptiveSizeClass {
    Compact,
    Regular,
    Spacious,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowMetrics {
    pub width: Pixels,
    pub height: Pixels,
    pub width_class: AdaptiveSizeClass,
    pub height_class: AdaptiveSizeClass,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveModalSpec {
    pub min_width: f32,
    pub max_width: f32,
    pub min_height: f32,
    pub max_height: f32,
    pub margin: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdaptiveModalSize {
    pub width: Pixels,
    pub height: Pixels,
    pub available_height: Pixels,
}

impl WindowMetrics {
    #[must_use]
    pub fn new(width: Pixels, height: Pixels) -> Self {
        let width_px = width / px(1.0);
        let height_px = height / px(1.0);

        Self {
            width,
            height,
            width_class: size_class(width_px),
            height_class: size_class(height_px),
        }
    }
}

#[must_use]
pub fn adaptive_modal_size(metrics: WindowMetrics, spec: AdaptiveModalSpec) -> AdaptiveModalSize {
    adaptive_modal_size_from_px(metrics.width / px(1.0), metrics.height / px(1.0), spec)
}

fn adaptive_modal_size_from_px(
    window_width: f32,
    window_height: f32,
    spec: AdaptiveModalSpec,
) -> AdaptiveModalSize {
    let margin = spec.margin.max(0.0);
    let available_width = (window_width - margin).max(0.0);
    let available_height = (window_height - margin).max(0.0);
    let width = axis_size(available_width, spec.min_width, spec.max_width);
    let height = axis_size(available_height, spec.min_height, spec.max_height);

    AdaptiveModalSize {
        width: px(width),
        height: px(height),
        available_height: px(available_height),
    }
}

fn axis_size(available: f32, minimum: f32, maximum: f32) -> f32 {
    if available <= minimum {
        available
    } else {
        available.min(maximum)
    }
}

fn size_class(value: f32) -> AdaptiveSizeClass {
    if value < 640.0 {
        AdaptiveSizeClass::Compact
    } else if value < 1040.0 {
        AdaptiveSizeClass::Regular
    } else {
        AdaptiveSizeClass::Spacious
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: AdaptiveModalSpec = AdaptiveModalSpec {
        min_width: 320.0,
        max_width: 520.0,
        min_height: 360.0,
        max_height: 640.0,
        margin: 40.0,
    };

    #[test]
    fn adaptive_modal_size_does_not_overflow_small_window() {
        let metrics = WindowMetrics::new(px(300.0), px(340.0));
        let size = adaptive_modal_size(metrics, SPEC);

        assert_eq!(size.width, px(260.0));
        assert_eq!(size.height, px(300.0));
        assert_eq!(size.available_height, px(300.0));
    }

    #[test]
    fn adaptive_modal_size_caps_large_window_height() {
        let metrics = WindowMetrics::new(px(1920.0), px(1032.0));
        let size = adaptive_modal_size(metrics, SPEC);

        assert_eq!(size.width, px(520.0));
        assert_eq!(size.height, px(640.0));
    }

    #[test]
    fn adaptive_modal_size_clamps_regular_window_between_bounds() {
        let metrics = WindowMetrics::new(px(720.0), px(560.0));
        let size = adaptive_modal_size(metrics, SPEC);

        assert_eq!(size.width, px(520.0));
        assert_eq!(size.height, px(520.0));
    }

    #[test]
    fn window_metrics_assigns_size_classes() {
        let compact = WindowMetrics::new(px(520.0), px(500.0));
        let regular = WindowMetrics::new(px(800.0), px(760.0));
        let spacious = WindowMetrics::new(px(1280.0), px(1100.0));

        assert_eq!(compact.width_class, AdaptiveSizeClass::Compact);
        assert_eq!(regular.width_class, AdaptiveSizeClass::Regular);
        assert_eq!(spacious.height_class, AdaptiveSizeClass::Spacious);
    }
}
