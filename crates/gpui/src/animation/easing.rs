use super::spring::Spring;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{fmt, rc::Rc};

/// Step easing edge behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum StepPosition {
    /// Jump at the end of each step.
    #[default]
    End,
    /// Jump at the start of each step.
    Start,
}

/// Built-in easing functions plus a compatibility escape hatch.
#[derive(Clone)]
pub enum Easing {
    /// Linear interpolation.
    Linear,
    /// Cubic ease-in.
    InCubic,
    /// Cubic ease-out.
    OutCubic,
    /// Cubic ease-in-out.
    InOutCubic,
    /// Overshooting ease-out back curve.
    OutBack,
    /// Quintic ease-out.
    OutQuint,
    /// Elastic ease-out curve.
    OutElastic,
    /// Physics spring sampled over normalized elapsed seconds.
    Spring(Spring),
    /// CSS-compatible cubic bezier easing.
    CubicBezier {
        /// First control point x coordinate.
        x1: f32,
        /// First control point y coordinate.
        y1: f32,
        /// Second control point x coordinate.
        x2: f32,
        /// Second control point y coordinate.
        y2: f32,
    },
    /// Discrete stepped easing.
    Steps {
        /// Number of steps.
        count: u32,
        /// Step edge behavior.
        position: StepPosition,
    },
    /// Caller-provided easing function.
    Custom(Rc<dyn Fn(f32) -> f32>),
}

impl Default for Easing {
    fn default() -> Self {
        Self::Linear
    }
}

impl fmt::Debug for Easing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Linear => f.write_str("Linear"),
            Self::InCubic => f.write_str("InCubic"),
            Self::OutCubic => f.write_str("OutCubic"),
            Self::InOutCubic => f.write_str("InOutCubic"),
            Self::OutBack => f.write_str("OutBack"),
            Self::OutQuint => f.write_str("OutQuint"),
            Self::OutElastic => f.write_str("OutElastic"),
            Self::Spring(spring) => f.debug_tuple("Spring").field(spring).finish(),
            Self::CubicBezier { x1, y1, x2, y2 } => f
                .debug_struct("CubicBezier")
                .field("x1", x1)
                .field("y1", y1)
                .field("x2", x2)
                .field("y2", y2)
                .finish(),
            Self::Steps { count, position } => f
                .debug_struct("Steps")
                .field("count", count)
                .field("position", position)
                .finish(),
            Self::Custom(_) => f.write_str("Custom"),
        }
    }
}

impl PartialEq for Easing {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Linear, Self::Linear)
            | (Self::InCubic, Self::InCubic)
            | (Self::OutCubic, Self::OutCubic)
            | (Self::InOutCubic, Self::InOutCubic)
            | (Self::OutBack, Self::OutBack)
            | (Self::OutQuint, Self::OutQuint)
            | (Self::OutElastic, Self::OutElastic) => true,
            (Self::Spring(left), Self::Spring(right)) => left == right,
            (
                Self::CubicBezier {
                    x1: left_x1,
                    y1: left_y1,
                    x2: left_x2,
                    y2: left_y2,
                },
                Self::CubicBezier {
                    x1: right_x1,
                    y1: right_y1,
                    x2: right_x2,
                    y2: right_y2,
                },
            ) => {
                left_x1.to_bits() == right_x1.to_bits()
                    && left_y1.to_bits() == right_y1.to_bits()
                    && left_x2.to_bits() == right_x2.to_bits()
                    && left_y2.to_bits() == right_y2.to_bits()
            }
            (
                Self::Steps {
                    count: left_count,
                    position: left_position,
                },
                Self::Steps {
                    count: right_count,
                    position: right_position,
                },
            ) => left_count == right_count && left_position == right_position,
            (Self::Custom(left), Self::Custom(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Easing {
    /// Sample this easing curve. Input is clamped to the `[0, 1]` range, while
    /// finite output is preserved so curves such as [`Easing::OutBack`] can
    /// overshoot.
    pub fn sample(&self, progress: f32) -> f32 {
        let progress = normalize_progress(progress);
        let sampled = match self {
            Self::Linear => progress,
            Self::InCubic => progress * progress * progress,
            Self::OutCubic => 1.0 - (1.0 - progress).powi(3),
            Self::InOutCubic => {
                if progress < 0.5 {
                    4.0 * progress * progress * progress
                } else {
                    1.0 - (-2.0 * progress + 2.0).powi(3) / 2.0
                }
            }
            Self::OutBack => {
                const C1: f32 = 1.701_58;
                const C3: f32 = C1 + 1.0;
                1.0 + C3 * (progress - 1.0).powi(3) + C1 * (progress - 1.0).powi(2)
            }
            Self::OutQuint => 1.0 - (1.0 - progress).powi(5),
            Self::OutElastic => sample_out_elastic(progress),
            Self::Spring(spring) => spring.sample(progress),
            Self::CubicBezier { x1, y1, x2, y2 } => {
                sample_cubic_bezier(*x1, *y1, *x2, *y2, progress)
            }
            Self::Steps { count, position } => sample_steps(*count, *position, progress),
            Self::Custom(function) => function(progress),
        };
        finite_sample_or_progress(sampled, progress)
    }

    pub(crate) fn sample_bounded(&self, progress: f32) -> f32 {
        self.sample(progress).clamp(0.0, 1.0)
    }

    pub(crate) fn requires_cpu_driver(&self) -> bool {
        matches!(self, Self::Custom(_) | Self::Spring(_))
    }

    pub(crate) fn to_style_easing(&self) -> TransitionEasing {
        match self {
            Self::Linear => TransitionEasing::Linear,
            Self::InCubic => TransitionEasing::InCubic,
            Self::OutCubic => TransitionEasing::OutCubic,
            Self::InOutCubic => TransitionEasing::InOutCubic,
            Self::OutBack => TransitionEasing::OutBack,
            Self::OutQuint => TransitionEasing::OutQuint,
            Self::OutElastic => TransitionEasing::OutElastic,
            Self::Spring(spring) => TransitionEasing::Spring(*spring),
            Self::CubicBezier { x1, y1, x2, y2 } => TransitionEasing::CubicBezier {
                x1: *x1,
                y1: *y1,
                x2: *x2,
                y2: *y2,
            },
            Self::Steps { count, position } => TransitionEasing::Steps {
                count: *count,
                position: *position,
            },
            Self::Custom(_) => TransitionEasing::Custom,
        }
    }
}

fn sample_cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, progress: f32) -> f32 {
    let x1 = normalize_progress(x1);
    let x2 = normalize_progress(x2);
    let y1 = if y1.is_finite() { y1 } else { 0.0 };
    let y2 = if y2.is_finite() { y2 } else { 1.0 };
    let t = cubic_bezier_t_for_x(x1, x2, progress);
    cubic_bezier_axis(y1, y2, t)
}

fn cubic_bezier_t_for_x(x1: f32, x2: f32, progress: f32) -> f32 {
    let mut lower = 0.0;
    let mut upper = 1.0;
    let mut t = progress;
    for _ in 0..10 {
        let x = cubic_bezier_axis(x1, x2, t);
        let derivative = cubic_bezier_derivative(x1, x2, t);
        let error = x - progress;
        if error.abs() <= 0.000_01 {
            return t;
        }
        if derivative.abs() >= 0.000_001 {
            let next = t - error / derivative;
            if (lower..=upper).contains(&next) {
                t = next;
                continue;
            }
        }
        if x < progress {
            lower = t;
        } else {
            upper = t;
        }
        t = (lower + upper) * 0.5;
    }
    t
}

fn cubic_bezier_axis(control1: f32, control2: f32, t: f32) -> f32 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * t * control1 + 3.0 * inverse * t * t * control2 + t * t * t
}

fn cubic_bezier_derivative(control1: f32, control2: f32, t: f32) -> f32 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * control1
        + 6.0 * inverse * t * (control2 - control1)
        + 3.0 * t * t * (1.0 - control2)
}

fn sample_out_elastic(progress: f32) -> f32 {
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }
    let c4 = (2.0 * std::f32::consts::PI) / 3.0;
    2.0f32.powf(-10.0 * progress) * ((progress * 10.0 - 0.75) * c4).sin() + 1.0
}

fn sample_steps(count: u32, position: StepPosition, progress: f32) -> f32 {
    let count = count.max(1) as f32;
    match position {
        StepPosition::End => (progress * count).floor() / count,
        StepPosition::Start => ((progress * count).floor() + 1.0) / count,
    }
    .clamp(0.0, 1.0)
}

pub(crate) fn normalize_progress(progress: f32) -> f32 {
    if progress.is_nan() {
        0.0
    } else {
        progress.clamp(0.0, 1.0)
    }
}

fn finite_sample_or_progress(sampled: f32, progress: f32) -> f32 {
    if sampled.is_finite() {
        sampled
    } else {
        progress
    }
}

pub(crate) fn sample_legacy_easing_bounded(easing: &dyn Fn(f32) -> f32, progress: f32) -> f32 {
    let progress = normalize_progress(progress);
    finite_sample_or_progress(easing(progress), progress).clamp(0.0, 1.0)
}

/// Serializable easing metadata stored in styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum TransitionEasing {
    /// Linear interpolation.
    #[default]
    Linear,
    /// Cubic ease-in.
    InCubic,
    /// Cubic ease-out.
    OutCubic,
    /// Cubic ease-in-out.
    InOutCubic,
    /// Overshooting ease-out back curve.
    OutBack,
    /// Quintic ease-out.
    OutQuint,
    /// Elastic ease-out curve.
    OutElastic,
    /// Physics spring sampled over normalized elapsed seconds.
    Spring(Spring),
    /// CSS-compatible cubic bezier easing.
    CubicBezier {
        /// First control point x coordinate.
        x1: f32,
        /// First control point y coordinate.
        y1: f32,
        /// Second control point x coordinate.
        x2: f32,
        /// Second control point y coordinate.
        y2: f32,
    },
    /// Discrete stepped easing.
    Steps {
        /// Number of steps.
        count: u32,
        /// Step edge behavior.
        position: StepPosition,
    },
    /// Runtime-only custom easing. Style metadata keeps the declaration but
    /// drivers that cannot access the closure fall back to layout sampling.
    Custom,
}

impl TransitionEasing {
    pub(crate) fn requires_cpu_driver(self) -> bool {
        matches!(self, Self::Custom | Self::Spring(_))
    }
}

impl From<TransitionEasing> for Easing {
    fn from(easing: TransitionEasing) -> Self {
        match easing {
            TransitionEasing::Linear => Self::Linear,
            TransitionEasing::Custom => Self::Custom(Rc::new(|progress| progress)),
            TransitionEasing::InCubic => Self::InCubic,
            TransitionEasing::OutCubic => Self::OutCubic,
            TransitionEasing::InOutCubic => Self::InOutCubic,
            TransitionEasing::OutBack => Self::OutBack,
            TransitionEasing::OutQuint => Self::OutQuint,
            TransitionEasing::OutElastic => Self::OutElastic,
            TransitionEasing::Spring(spring) => Self::Spring(spring),
            TransitionEasing::CubicBezier { x1, y1, x2, y2 } => {
                Self::CubicBezier { x1, y1, x2, y2 }
            }
            TransitionEasing::Steps { count, position } => Self::Steps { count, position },
        }
    }
}
