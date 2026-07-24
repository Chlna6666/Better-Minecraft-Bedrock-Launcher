use super::{animatable::Animatable, easing::Easing};

/// A simple tween descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct Tween<T> {
    /// Starting value.
    pub from: T,
    /// Ending value.
    pub to: T,
    /// Easing curve.
    pub easing: Easing,
}

impl<T> Tween<T> {
    /// Create a linear tween.
    pub fn new(from: T, to: T) -> Self {
        Self {
            from,
            to,
            easing: Easing::Linear,
        }
    }

    /// Set the easing curve.
    pub fn ease(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }
}

impl<T: Animatable> Tween<T> {
    /// Sample the tween at progress `progress`.
    pub fn sample(&self, progress: f32) -> T {
        T::interpolate(&self.from, &self.to, self.easing.sample(progress))
    }
}
