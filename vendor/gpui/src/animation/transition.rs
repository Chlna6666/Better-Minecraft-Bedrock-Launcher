use super::{
    easing::Easing,
    scheduler::AnimationDriver,
    timeline::{AnimationSpec, TransitionSpec},
};
use smallvec::{SmallVec, smallvec};
use std::time::Duration;

/// Properties supported by the transition API.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
pub enum TransitionProperty {
    /// Element opacity.
    Opacity,
    /// Full transform matrix or transform-related properties.
    Transform,
    /// Translation.
    Translation,
    /// Scale.
    Scale,
    /// Rotation.
    Rotation,
    /// Background or foreground color.
    Color,
    /// Blur radius.
    Blur,
    /// Shadow fields.
    Shadow,
    /// Width.
    Width,
    /// Height.
    Height,
    /// Absolute/relative inset.
    Inset,
    /// Margin.
    Margin,
    /// Padding.
    Padding,
    /// Flex/grid gap.
    Gap,
    /// Border width.
    BorderWidth,
}

impl TransitionProperty {
    /// Returns true if this property requires layout recomputation.
    pub fn affects_layout(self) -> bool {
        matches!(
            self,
            Self::Width
                | Self::Height
                | Self::Inset
                | Self::Margin
                | Self::Padding
                | Self::Gap
                | Self::BorderWidth
        )
    }

    /// Resolve `AnimationDriver::Auto` for this property.
    pub fn preferred_driver(self) -> AnimationDriver {
        if self.affects_layout() {
            AnimationDriver::Layout
        } else if self.supports_gpu_driver() {
            AnimationDriver::Gpu
        } else {
            AnimationDriver::Paint
        }
    }

    /// Returns true if this property can be sampled by the current generic GPU
    /// animation path without primitive-specific fallback checks.
    pub fn supports_gpu_driver(self) -> bool {
        matches!(
            self,
            Self::Opacity | Self::Transform | Self::Translation | Self::Scale | Self::Rotation
        )
    }
}

/// Builder for state-change animations declared on styled elements.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    /// Timing and driver policy.
    pub spec: AnimationSpec,
    /// Animated properties.
    pub properties: SmallVec<[TransitionProperty; 4]>,
}

impl Transition {
    /// Create a transition with the supplied duration.
    pub fn new(duration: Duration) -> Self {
        Self {
            spec: AnimationSpec::new(duration),
            properties: smallvec![TransitionProperty::Opacity, TransitionProperty::Transform],
        }
    }

    /// Set easing.
    pub fn ease(mut self, easing: Easing) -> Self {
        self.spec = self.spec.ease(easing);
        self
    }

    /// Set start delay.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.spec = self.spec.delay(delay);
        self
    }

    /// Set repeat behavior.
    pub fn repeat(mut self, repeat: super::timeline::RepeatMode) -> Self {
        self.spec = self.spec.repeat(repeat);
        self
    }

    /// Set direction.
    pub fn direction(mut self, direction: super::timeline::AnimationDirection) -> Self {
        self.spec = self.spec.direction(direction);
        self
    }

    /// Set fill mode.
    pub fn fill_mode(mut self, fill_mode: super::timeline::FillMode) -> Self {
        self.spec = self.spec.fill_mode(fill_mode);
        self
    }

    /// Set preferred driver.
    pub fn driver(mut self, driver: AnimationDriver) -> Self {
        self.spec = self.spec.driver(driver);
        self
    }

    /// Set animated properties.
    pub fn properties<I>(mut self, properties: I) -> Self
    where
        I: IntoIterator<Item = TransitionProperty>,
    {
        self.properties = properties.into_iter().collect();
        self
    }

    /// Convert this runtime transition to serializable style metadata.
    pub fn into_style(self) -> TransitionStyle {
        let mut spec = self.spec.to_style_spec();
        spec.driver = resolve_driver_with_cpu_policy(
            spec.driver,
            self.properties.iter().copied(),
            self.spec.easing.requires_cpu_driver(),
        );

        TransitionStyle {
            spec,
            properties: self.properties.into_iter().collect(),
        }
    }

    /// Returns the driver required by this transition after property classification.
    pub fn resolved_driver(&self) -> AnimationDriver {
        resolve_driver_with_cpu_policy(
            self.spec.driver,
            self.properties.iter().copied(),
            self.spec.easing.requires_cpu_driver(),
        )
    }
}

/// Serializable transition metadata attached to styles.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TransitionStyle {
    /// Timing and driver policy.
    pub spec: TransitionSpec,
    /// Animated properties.
    pub properties: Vec<TransitionProperty>,
}

impl TransitionStyle {
    /// Returns the driver required by this transition after property classification.
    pub fn resolved_driver(&self) -> AnimationDriver {
        resolve_driver_with_cpu_policy(
            self.spec.driver,
            self.properties.iter().copied(),
            self.spec.easing.requires_cpu_driver(),
        )
    }
}

impl From<TransitionStyle> for Transition {
    fn from(style: TransitionStyle) -> Self {
        let requires_cpu_driver = style.spec.easing.requires_cpu_driver();
        let properties = style.properties.into_iter().collect::<SmallVec<[_; 4]>>();
        let mut spec = AnimationSpec::from(style.spec);
        spec.driver = resolve_driver_with_cpu_policy(
            spec.driver,
            properties.iter().copied(),
            requires_cpu_driver,
        );

        Self { spec, properties }
    }
}

pub(crate) fn resolve_driver(
    requested: AnimationDriver,
    properties: impl IntoIterator<Item = TransitionProperty>,
) -> AnimationDriver {
    resolve_driver_with_cpu_policy(requested, properties, false)
}

pub(crate) fn resolve_driver_with_cpu_policy(
    requested: AnimationDriver,
    properties: impl IntoIterator<Item = TransitionProperty>,
    requires_cpu_driver: bool,
) -> AnimationDriver {
    if matches!(requested, AnimationDriver::Layout) {
        return AnimationDriver::Layout;
    }

    let mut has_property = false;
    let mut all_properties_support_gpu = true;
    for property in properties {
        has_property = true;
        if property.affects_layout() {
            return AnimationDriver::Layout;
        }
        if !property.supports_gpu_driver() {
            all_properties_support_gpu = false;
        }
    }

    if requires_cpu_driver {
        return AnimationDriver::Paint;
    }

    match requested {
        AnimationDriver::Gpu if all_properties_support_gpu => AnimationDriver::Gpu,
        AnimationDriver::Gpu => AnimationDriver::Paint,
        AnimationDriver::Auto if has_property && all_properties_support_gpu => AnimationDriver::Gpu,
        AnimationDriver::Auto | AnimationDriver::Paint => AnimationDriver::Paint,
        AnimationDriver::Layout => AnimationDriver::Layout,
    }
}
