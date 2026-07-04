use crate::{
    AbsoluteLength, DefiniteLength, Edges, Hsla, Length, Pixels, Point, Size, TransformationMatrix,
    style::BoxShadow,
};
use std::f32::consts::{PI, TAU};

const TRANSFORM_EPSILON: f32 = 0.000_01;
const TRANSFORM_SHEAR_EPSILON: f32 = 0.01;

/// Interpolates animation values.
pub trait Animatable: Clone {
    /// Interpolate between `from` and `to` at progress `progress`.
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self;
}

fn interpolate_f32(from: f32, to: f32, progress: f32) -> f32 {
    let progress = if progress.is_finite() { progress } else { 0.0 };
    from + (to - from) * progress
}

impl Animatable for f32 {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        interpolate_f32(*from, *to, progress)
    }
}

impl Animatable for Pixels {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        Self::from(interpolate_f32((*from).into(), (*to).into(), progress))
    }
}

impl Animatable for Hsla {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        Self {
            h: interpolate_hue(from.h, to.h, progress),
            s: interpolate_f32(from.s, to.s, progress),
            l: interpolate_f32(from.l, to.l, progress),
            a: interpolate_f32(from.a, to.a, progress),
        }
    }
}

fn interpolate_hue(from: f32, to: f32, progress: f32) -> f32 {
    let from = from.rem_euclid(1.0);
    let to = to.rem_euclid(1.0);
    let delta = (to - from + 0.5).rem_euclid(1.0) - 0.5;
    (from + delta * progress).rem_euclid(1.0)
}

impl Animatable for Point<Pixels> {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        Point {
            x: Pixels::interpolate(&from.x, &to.x, progress),
            y: Pixels::interpolate(&from.y, &to.y, progress),
        }
    }
}

impl Animatable for Size<Pixels> {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        Size {
            width: Pixels::interpolate(&from.width, &to.width, progress),
            height: Pixels::interpolate(&from.height, &to.height, progress),
        }
    }
}

impl<T: Animatable> Animatable for Vec<T> {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        if from.len() != to.len() {
            return if progress >= 1.0 {
                to.clone()
            } else {
                from.clone()
            };
        }

        from.iter()
            .zip(to)
            .map(|(from, to)| T::interpolate(from, to, progress))
            .collect()
    }
}

impl Animatable for Edges<Pixels> {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        Self {
            top: Pixels::interpolate(&from.top, &to.top, progress),
            right: Pixels::interpolate(&from.right, &to.right, progress),
            bottom: Pixels::interpolate(&from.bottom, &to.bottom, progress),
            left: Pixels::interpolate(&from.left, &to.left, progress),
        }
    }
}

impl Animatable for TransformationMatrix {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        let rotation_scale =
            interpolate_rotation_scale(from.rotation_scale, to.rotation_scale, progress)
                .unwrap_or_else(|| {
                    [
                        [
                            interpolate_f32(
                                from.rotation_scale[0][0],
                                to.rotation_scale[0][0],
                                progress,
                            ),
                            interpolate_f32(
                                from.rotation_scale[0][1],
                                to.rotation_scale[0][1],
                                progress,
                            ),
                        ],
                        [
                            interpolate_f32(
                                from.rotation_scale[1][0],
                                to.rotation_scale[1][0],
                                progress,
                            ),
                            interpolate_f32(
                                from.rotation_scale[1][1],
                                to.rotation_scale[1][1],
                                progress,
                            ),
                        ],
                    ]
                });
        Self {
            rotation_scale,
            translation: [
                interpolate_f32(from.translation[0], to.translation[0], progress),
                interpolate_f32(from.translation[1], to.translation[1], progress),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct DecomposedTransform {
    rotation: f32,
    scale_x: f32,
    scale_y: f32,
}

fn interpolate_rotation_scale(
    from: [[f32; 2]; 2],
    to: [[f32; 2]; 2],
    progress: f32,
) -> Option<[[f32; 2]; 2]> {
    let from = decompose_rotation_scale(from)?;
    let to = decompose_rotation_scale(to)?;
    Some(compose_rotation_scale(DecomposedTransform {
        rotation: interpolate_angle(from.rotation, to.rotation, progress),
        scale_x: interpolate_f32(from.scale_x, to.scale_x, progress),
        scale_y: interpolate_f32(from.scale_y, to.scale_y, progress),
    }))
}

fn decompose_rotation_scale(matrix: [[f32; 2]; 2]) -> Option<DecomposedTransform> {
    let [[m00, m01], [m10, m11]] = matrix;
    if !m00.is_finite() || !m01.is_finite() || !m10.is_finite() || !m11.is_finite() {
        return None;
    }

    let scale_x = m00.hypot(m10);
    let scale_y = m01.hypot(m11);
    if scale_x <= TRANSFORM_EPSILON || scale_y <= TRANSFORM_EPSILON {
        return None;
    }

    let normalized_dot = (m00 * m01 + m10 * m11) / (scale_x * scale_y);
    if normalized_dot.abs() > TRANSFORM_SHEAR_EPSILON {
        return None;
    }

    let determinant = m00 * m11 - m01 * m10;
    let scale_y = if determinant < 0.0 { -scale_y } else { scale_y };
    Some(DecomposedTransform {
        rotation: m10.atan2(m00),
        scale_x,
        scale_y,
    })
}

fn compose_rotation_scale(transform: DecomposedTransform) -> [[f32; 2]; 2] {
    let (sin, cos) = transform.rotation.sin_cos();
    [
        [cos * transform.scale_x, -sin * transform.scale_y],
        [sin * transform.scale_x, cos * transform.scale_y],
    ]
}

fn interpolate_angle(from: f32, to: f32, progress: f32) -> f32 {
    from + ((to - from + PI).rem_euclid(TAU) - PI) * progress
}

impl Animatable for AbsoluteLength {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        match (*from, *to) {
            (Self::Pixels(from), Self::Pixels(to)) => {
                Self::Pixels(Pixels::interpolate(&from, &to, progress))
            }
            (Self::Rems(from), Self::Rems(to)) => {
                Self::Rems(crate::Rems(interpolate_f32(from.0, to.0, progress)))
            }
            _ if progress >= 1.0 => *to,
            _ => *from,
        }
    }
}

impl Animatable for DefiniteLength {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        match (*from, *to) {
            (Self::Absolute(from), Self::Absolute(to)) => {
                Self::Absolute(AbsoluteLength::interpolate(&from, &to, progress))
            }
            (Self::Fraction(from), Self::Fraction(to)) => {
                Self::Fraction(interpolate_f32(from, to, progress))
            }
            _ if progress >= 1.0 => *to,
            _ => *from,
        }
    }
}

impl Animatable for Length {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        match (*from, *to) {
            (Self::Definite(from), Self::Definite(to)) => {
                Self::Definite(DefiniteLength::interpolate(&from, &to, progress))
            }
            _ if progress >= 1.0 => *to,
            _ => *from,
        }
    }
}

impl Animatable for BoxShadow {
    fn interpolate(from: &Self, to: &Self, progress: f32) -> Self {
        Self {
            color: Hsla::interpolate(&from.color, &to.color, progress),
            offset: Point::<Pixels>::interpolate(&from.offset, &to.offset, progress),
            blur_radius: Pixels::interpolate(&from.blur_radius, &to.blur_radius, progress),
            spread_radius: Pixels::interpolate(&from.spread_radius, &to.spread_radius, progress),
        }
    }
}
