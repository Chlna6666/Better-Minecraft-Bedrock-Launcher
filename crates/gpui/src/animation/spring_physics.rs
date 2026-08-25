/// Physics-based motion helpers.
#[derive(
    Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct PhysicsConfig {
    /// Spring stiffness.
    pub stiffness: f32,
    /// Spring damping.
    pub damping: f32,
    /// Mass.
    pub mass: f32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            stiffness: 1.0,
            damping: 1.0,
            mass: 1.0,
        }
    }
}

impl PhysicsConfig {
    /// Returns a stable decay factor for the current configuration.
    pub fn decay_factor(&self, elapsed_seconds: f32) -> f32 {
        self.position_velocity(elapsed_seconds, 0.0)
            .displacement
            .abs()
    }

    /// Solve the normalized spring displacement and velocity at `elapsed_seconds`.
    pub fn position_velocity(&self, elapsed_seconds: f32, initial_velocity: f32) -> SpringMotion {
        let elapsed_seconds = finite_non_negative(elapsed_seconds);
        let initial_velocity = if initial_velocity.is_finite() {
            initial_velocity
        } else {
            0.0
        };
        let mass = positive_or_default(self.mass, 1.0);
        let stiffness = positive_or_default(self.stiffness, 1.0);
        let damping = finite_non_negative(self.damping);
        let omega = (stiffness / mass).sqrt();
        let damping_ratio = damping / (2.0 * (stiffness * mass).sqrt());
        let initial_displacement = -1.0;

        if damping_ratio < 1.0 - 0.000_1 {
            underdamped_motion(
                elapsed_seconds,
                omega,
                damping_ratio,
                initial_displacement,
                initial_velocity,
            )
        } else if damping_ratio > 1.0 + 0.000_1 {
            overdamped_motion(
                elapsed_seconds,
                omega,
                damping_ratio,
                initial_displacement,
                initial_velocity,
            )
        } else {
            critically_damped_motion(
                elapsed_seconds,
                omega,
                initial_displacement,
                initial_velocity,
            )
        }
    }
}

/// Normalized spring displacement and velocity relative to the target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringMotion {
    /// Displacement from the target, where 0 is settled.
    pub displacement: f32,
    /// Velocity in normalized units per second.
    pub velocity: f32,
}

fn underdamped_motion(
    time: f32,
    omega: f32,
    damping_ratio: f32,
    initial_displacement: f32,
    initial_velocity: f32,
) -> SpringMotion {
    let damped_omega = omega * (1.0 - damping_ratio * damping_ratio).sqrt();
    let decay = (-damping_ratio * omega * time).exp();
    let cos = (damped_omega * time).cos();
    let sin = (damped_omega * time).sin();
    let coefficient = (initial_velocity + damping_ratio * omega * initial_displacement)
        / damped_omega.max(f32::MIN_POSITIVE);
    let displacement = decay * (initial_displacement * cos + coefficient * sin);
    let velocity = decay
        * ((coefficient * damped_omega - damping_ratio * omega * initial_displacement) * cos
            + (-initial_displacement * damped_omega - damping_ratio * omega * coefficient) * sin);
    SpringMotion {
        displacement,
        velocity,
    }
}

fn critically_damped_motion(
    time: f32,
    omega: f32,
    initial_displacement: f32,
    initial_velocity: f32,
) -> SpringMotion {
    let coefficient = initial_velocity + omega * initial_displacement;
    let decay = (-omega * time).exp();
    let displacement = (initial_displacement + coefficient * time) * decay;
    let velocity = (coefficient - omega * (initial_displacement + coefficient * time)) * decay;
    SpringMotion {
        displacement,
        velocity,
    }
}

fn overdamped_motion(
    time: f32,
    omega: f32,
    damping_ratio: f32,
    initial_displacement: f32,
    initial_velocity: f32,
) -> SpringMotion {
    let root = (damping_ratio * damping_ratio - 1.0).sqrt();
    let root_a = -omega * (damping_ratio - root);
    let root_b = -omega * (damping_ratio + root);
    let coefficient_a = (initial_velocity - root_b * initial_displacement) / (root_a - root_b);
    let coefficient_b = initial_displacement - coefficient_a;
    let exp_a = (root_a * time).exp();
    let exp_b = (root_b * time).exp();
    SpringMotion {
        displacement: coefficient_a * exp_a + coefficient_b * exp_b,
        velocity: coefficient_a * root_a * exp_a + coefficient_b * root_b * exp_b,
    }
}

fn finite_non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn positive_or_default(value: f32, default: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        default
    }
}
