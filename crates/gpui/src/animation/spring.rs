use super::physics::PhysicsConfig;

/// Sampled spring state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringSample {
    /// Normalized progress toward the target.
    pub progress: f32,
    /// Normalized velocity.
    pub velocity: f32,
    /// True when position and velocity are both below settle thresholds.
    pub done: bool,
}

/// Spring animation parameters.
#[derive(
    Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct Spring {
    /// Physics parameters.
    pub physics: PhysicsConfig,
    /// Position delta threshold used to mark the spring settled.
    pub settle_position: f32,
    /// Velocity threshold used to mark the spring settled.
    pub settle_velocity: f32,
}

impl Default for Spring {
    fn default() -> Self {
        Self {
            physics: PhysicsConfig::default(),
            settle_position: 0.001,
            settle_velocity: 0.001,
        }
    }
}

impl Spring {
    /// Estimate normalized progress for the given elapsed time.
    pub fn sample(&self, elapsed_seconds: f32) -> f32 {
        self.sample_with_velocity(elapsed_seconds, 0.0).progress
    }

    /// Sample normalized spring progress using an initial velocity.
    pub fn sample_with_velocity(
        &self,
        elapsed_seconds: f32,
        initial_velocity: f32,
    ) -> SpringSample {
        let motion = self
            .physics
            .position_velocity(elapsed_seconds, initial_velocity);
        SpringSample {
            progress: 1.0 + motion.displacement,
            velocity: motion.velocity,
            done: motion.displacement.abs() <= self.settle_position.max(0.0)
                && motion.velocity.abs() <= self.settle_velocity.max(0.0),
        }
    }
}
