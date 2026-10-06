use crate::Vec3;
use std::fmt;

/// Invalid spotlight configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LightError {
    /// Position, direction, color, intensity, range, or cone angles are invalid.
    InvalidSpotParameters,
}

impl fmt::Display for LightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("spotlight parameters must be finite and within their valid ranges")
    }
}

impl std::error::Error for LightError {}

/// Soft spotlight cone angles, measured from the light's forward axis in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpotCone {
    inner_angle: f32,
    outer_angle: f32,
}

impl SpotCone {
    /// Creates a cone with full intensity inside `inner_angle` and zero outside `outer_angle`.
    ///
    /// Both angles are measured in radians and must satisfy `0 <= inner < outer < PI`.
    ///
    /// # Errors
    ///
    /// Returns [`LightError::InvalidSpotParameters`] when either angle is non-finite or outside
    /// the valid cone range.
    pub fn new(inner_angle: f32, outer_angle: f32) -> Result<Self, LightError> {
        if !inner_angle.is_finite()
            || !outer_angle.is_finite()
            || inner_angle < 0.0
            || inner_angle >= outer_angle
            || outer_angle >= std::f32::consts::PI
        {
            return Err(LightError::InvalidSpotParameters);
        }
        Ok(Self {
            inner_angle,
            outer_angle,
        })
    }

    /// Inner full-intensity cone angle in radians.
    #[must_use]
    pub const fn inner_angle(self) -> f32 {
        self.inner_angle
    }

    /// Outer zero-intensity cone angle in radians.
    #[must_use]
    pub const fn outer_angle(self) -> f32 {
        self.outer_angle
    }
}

/// Local-space spotlight attached to a scene node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpotLight {
    position: Vec3,
    direction: Vec3,
    color: [f32; 3],
    intensity: f32,
    range: f32,
    cone: SpotCone,
}

impl SpotLight {
    /// Creates a white spotlight with unit intensity.
    ///
    /// The direction is normalized. The range must be positive and finite; the cone uses radians.
    /// The node's world transform later moves the position and direction and scales the range by
    /// the largest world-space axis scale.
    ///
    /// # Errors
    ///
    /// Returns [`LightError::InvalidSpotParameters`] for a non-finite position, zero or
    /// non-finite direction, non-positive range, or invalid cone.
    pub fn new(
        position: Vec3,
        direction: Vec3,
        range: f32,
        cone: SpotCone,
    ) -> Result<Self, LightError> {
        if !position.is_finite() || !range.is_finite() || range <= 0.0 {
            return Err(LightError::InvalidSpotParameters);
        }
        let direction = direction
            .normalized()
            .ok_or(LightError::InvalidSpotParameters)?;
        Ok(Self {
            position,
            direction,
            color: [1.0; 3],
            intensity: 1.0,
            range,
            cone,
        })
    }

    /// Sets the linear RGB color.
    ///
    /// # Errors
    ///
    /// Returns [`LightError::InvalidSpotParameters`] for a negative or non-finite component.
    pub fn with_color(mut self, color: [f32; 3]) -> Result<Self, LightError> {
        if color
            .iter()
            .any(|component| !component.is_finite() || *component < 0.0)
        {
            return Err(LightError::InvalidSpotParameters);
        }
        self.color = color;
        Ok(self)
    }

    /// Sets the non-negative radiance multiplier.
    ///
    /// # Errors
    ///
    /// Returns [`LightError::InvalidSpotParameters`] for a negative or non-finite intensity.
    pub fn with_intensity(mut self, intensity: f32) -> Result<Self, LightError> {
        if !intensity.is_finite() || intensity < 0.0 {
            return Err(LightError::InvalidSpotParameters);
        }
        self.intensity = intensity;
        Ok(self)
    }

    /// Local-space light position.
    #[must_use]
    pub const fn position(self) -> Vec3 {
        self.position
    }

    /// Normalized local-space direction the light rays travel in.
    #[must_use]
    pub const fn direction(self) -> Vec3 {
        self.direction
    }

    /// Linear RGB color.
    #[must_use]
    pub const fn color(self) -> [f32; 3] {
        self.color
    }

    /// Radiance multiplier.
    #[must_use]
    pub const fn intensity(self) -> f32 {
        self.intensity
    }

    /// Influence radius in node-local units.
    #[must_use]
    pub const fn range(self) -> f32 {
        self.range
    }

    /// Soft cone angle configuration.
    #[must_use]
    pub const fn cone(self) -> SpotCone {
        self.cone
    }
}

/// Light attached to a scene node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    /// Directional light with linear RGB intensity.
    Directional {
        /// Local-space direction rays travel in.
        direction: Vec3,
        /// Linear RGB color.
        color: [f32; 3],
        /// Non-negative intensity.
        intensity: f32,
    },
    /// Point light with linear RGB intensity and finite range.
    Point {
        /// Local-space light position.
        position: Vec3,
        /// Linear RGB color.
        color: [f32; 3],
        /// Non-negative intensity.
        intensity: f32,
        /// Influence radius in node-local units.
        range: f32,
    },
    /// Soft-cone spotlight with node-local position and direction.
    Spot(SpotLight),
    /// Ambient scene illumination.
    Ambient {
        /// Linear RGB color.
        color: [f32; 3],
        /// Non-negative intensity.
        intensity: f32,
    },
}

/// Light with its node transform applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PreparedLight {
    /// Directional light in world space.
    Directional {
        /// World-space direction rays travel in.
        direction: Vec3,
        /// Linear RGB color.
        color: [f32; 3],
        /// Non-negative intensity.
        intensity: f32,
    },
    /// Point light in world space.
    Point {
        /// World-space light position.
        position: Vec3,
        /// Linear RGB color.
        color: [f32; 3],
        /// Non-negative intensity.
        intensity: f32,
        /// Influence radius in world units.
        range: f32,
    },
    /// Soft-cone spotlight in world space.
    Spot {
        /// World-space light position.
        position: Vec3,
        /// Normalized world-space direction rays travel in.
        direction: Vec3,
        /// Linear RGB color.
        color: [f32; 3],
        /// Non-negative intensity.
        intensity: f32,
        /// Influence radius in world units.
        range: f32,
        /// Inner full-intensity cone angle in radians.
        inner_angle: f32,
        /// Outer zero-intensity cone angle in radians.
        outer_angle: f32,
    },
    /// Ambient scene illumination.
    Ambient {
        /// Linear RGB color.
        color: [f32; 3],
        /// Non-negative intensity.
        intensity: f32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_cone_and_light_reject_invalid_inputs() {
        assert_eq!(
            SpotCone::new(0.5, 0.5),
            Err(LightError::InvalidSpotParameters)
        );
        assert_eq!(
            SpotCone::new(0.0, std::f32::consts::PI),
            Err(LightError::InvalidSpotParameters)
        );
        let cone = SpotCone::new(0.2, 0.5).unwrap();
        assert!(SpotLight::new(Vec3::ZERO, Vec3::ZERO, 1.0, cone).is_err());
        assert!(SpotLight::new(Vec3::ZERO, -Vec3::Z, 0.0, cone).is_err());
        assert!(
            SpotLight::new(Vec3::ZERO, -Vec3::Z, 1.0, cone)
                .unwrap()
                .with_color([1.0, f32::NAN, 1.0])
                .is_err()
        );
    }

    #[test]
    fn spot_light_normalizes_direction_and_accepts_linear_radiance() {
        let cone = SpotCone::new(0.2, 0.5).unwrap();
        let light = SpotLight::new(Vec3::new(1.0, 2.0, 3.0), -Vec3::Z * 4.0, 8.0, cone)
            .unwrap()
            .with_color([0.8, 0.7, 0.6])
            .unwrap()
            .with_intensity(12.0)
            .unwrap();

        assert_eq!(light.direction(), -Vec3::Z);
        assert_eq!(light.color(), [0.8, 0.7, 0.6]);
        assert_eq!(light.intensity(), 12.0);
        assert_eq!(light.range(), 8.0);
    }
}
