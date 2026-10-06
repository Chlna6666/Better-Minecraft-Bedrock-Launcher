use crate::{NodeId, Quat, Transform, Vec3};
use std::{collections::HashMap, time::Duration};

/// Reusable storage for evaluating repeated scene animation samples.
///
/// Pass one scratch value to [`crate::Scene::evaluate_with`] across samples. The returned scene
/// borrows it until that snapshot is dropped, so the transform map can retain its allocation.
#[derive(Debug, Default)]
pub struct AnimationScratch {
    pub(crate) transforms: HashMap<NodeId, Transform>,
}

impl AnimationScratch {
    /// Creates empty reusable animation storage.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Releases retained transform-map capacity after a large clip is no longer sampled.
    pub fn trim(&mut self) {
        self.transforms = HashMap::new();
    }
}

/// Interpolation used between a key and the next key in a track.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Interpolation {
    /// Keep the key value until the next key.
    Step,
    /// Interpolate linearly; rotations use shortest-arc spherical interpolation.
    #[default]
    Linear,
}

/// A value sampled at an absolute clip time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keyframe<T> {
    time: Duration,
    value: T,
    interpolation: Interpolation,
}

impl<T> Keyframe<T> {
    /// Creates a linearly interpolated keyframe.
    pub const fn new(time: Duration, value: T) -> Self {
        Self {
            time,
            value,
            interpolation: Interpolation::Linear,
        }
    }

    /// Selects how this value blends toward the next key.
    pub const fn with_interpolation(mut self, interpolation: Interpolation) -> Self {
        self.interpolation = interpolation;
        self
    }

    /// Absolute time from the start of the clip.
    pub const fn time(&self) -> Duration {
        self.time
    }

    /// Keyed value.
    pub const fn value(&self) -> T
    where
        T: Copy,
    {
        self.value
    }

    /// Interpolation toward the next key.
    pub const fn interpolation(&self) -> Interpolation {
        self.interpolation
    }
}

/// Errors in transform animation data or evaluation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AnimationError {
    /// A channel contains no keyframes.
    #[error("animation channel has no keyframes")]
    EmptyTrack,
    /// Keyframe times are not strictly increasing.
    #[error("animation keyframe times must be strictly increasing")]
    InvalidKeyTimes,
    /// A translation or scale key contains a non-finite value.
    #[error("animation vector key contains a non-finite value")]
    NonFiniteVector,
    /// A rotation key is zero, non-finite, or cannot be normalized.
    #[error("animation rotation key is invalid")]
    InvalidRotation,
    /// The base transform contains an invalid translation, rotation, or scale.
    #[error("animation base transform is invalid")]
    InvalidBaseTransform,
    /// A scene track refers to a node handle that is no longer live.
    #[error("animation track refers to a stale scene node")]
    StaleNode,
    /// More than one transform track targets the same scene node.
    #[error("multiple animation tracks target the same scene node")]
    DuplicateNodeTrack,
}

/// Absolute-time keyframes for a vector-valued transform channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Vec3Track {
    keys: Box<[Keyframe<Vec3>]>,
}

impl Vec3Track {
    /// Creates a vector channel with strictly increasing times and finite values.
    ///
    /// # Errors
    ///
    /// Returns [`AnimationError::EmptyTrack`] for no keys, [`AnimationError::InvalidKeyTimes`]
    /// for duplicate or decreasing times, or [`AnimationError::NonFiniteVector`] for invalid
    /// translations or scales.
    pub fn new(keys: impl Into<Box<[Keyframe<Vec3>]>>) -> Result<Self, AnimationError> {
        let keys = keys.into();
        validate_keys(&keys)?;
        if keys.iter().any(|key| !key.value.is_finite()) {
            return Err(AnimationError::NonFiniteVector);
        }
        Ok(Self { keys })
    }

    /// Keyframes in ascending clip-time order.
    pub fn keys(&self) -> &[Keyframe<Vec3>] {
        &self.keys
    }

    /// Samples the channel at an absolute clip time, clamping outside its key range.
    pub fn sample(&self, time: Duration) -> Vec3 {
        sample_pair(&self.keys, time, |left, right, amount| {
            left + (right - left) * amount
        })
    }
}

/// Absolute-time keyframes for a quaternion rotation channel.
#[derive(Clone, Debug, PartialEq)]
pub struct RotationTrack {
    keys: Box<[Keyframe<Quat>]>,
}

impl RotationTrack {
    /// Creates a rotation channel and normalizes each valid quaternion key.
    ///
    /// # Errors
    ///
    /// Returns [`AnimationError::EmptyTrack`] for no keys, [`AnimationError::InvalidKeyTimes`]
    /// for duplicate or decreasing times, or [`AnimationError::InvalidRotation`] for a key that
    /// cannot be normalized.
    pub fn new(keys: impl Into<Box<[Keyframe<Quat>]>>) -> Result<Self, AnimationError> {
        let mut keys = keys.into();
        validate_keys(&keys)?;
        for key in &mut keys {
            key.value = key
                .value
                .normalized()
                .ok_or(AnimationError::InvalidRotation)?;
        }
        Ok(Self { keys })
    }

    /// Keyframes in ascending clip-time order.
    pub fn keys(&self) -> &[Keyframe<Quat>] {
        &self.keys
    }

    /// Samples the channel at an absolute clip time, clamping outside its key range.
    pub fn sample(&self, time: Duration) -> Quat {
        sample_pair(&self.keys, time, |left, right, amount| {
            left.slerp(right, amount).unwrap_or(left)
        })
    }
}

/// Transform channels for one scene node.
///
/// Channels are stateless functions of absolute clip time. Missing channels preserve the node's
/// base transform, and sampling does not change the source scene or depend on previous samples.
#[derive(Clone, Debug, PartialEq)]
pub struct TransformTrack {
    node: NodeId,
    translation: Option<Vec3Track>,
    rotation: Option<RotationTrack>,
    scale: Option<Vec3Track>,
}

impl TransformTrack {
    /// Creates an empty track targeting one generational scene-node handle.
    pub const fn new(node: NodeId) -> Self {
        Self {
            node,
            translation: None,
            rotation: None,
            scale: None,
        }
    }

    /// Target scene node.
    pub const fn node(&self) -> NodeId {
        self.node
    }

    /// Sets the translation channel.
    pub fn with_translation(mut self, track: Vec3Track) -> Self {
        self.translation = Some(track);
        self
    }

    /// Sets the rotation channel.
    pub fn with_rotation(mut self, track: RotationTrack) -> Self {
        self.rotation = Some(track);
        self
    }

    /// Sets the scale channel.
    pub fn with_scale(mut self, track: Vec3Track) -> Self {
        self.scale = Some(track);
        self
    }

    /// Samples this node's local transform from its authored base pose.
    ///
    /// # Errors
    ///
    /// Returns [`AnimationError::InvalidBaseTransform`] when the authored pose contains
    /// non-finite values or an invalid rotation.
    pub fn sample(&self, base: Transform, time: Duration) -> Result<Transform, AnimationError> {
        if !base.translation.is_finite()
            || !base.scale.is_finite()
            || !base.rotation.x.is_finite()
            || !base.rotation.y.is_finite()
            || !base.rotation.z.is_finite()
            || !base.rotation.w.is_finite()
        {
            return Err(AnimationError::InvalidBaseTransform);
        }
        let rotation = base
            .rotation
            .normalized()
            .ok_or(AnimationError::InvalidBaseTransform)?;
        let transform = Transform {
            translation: self
                .translation
                .as_ref()
                .map_or(base.translation, |track| track.sample(time)),
            rotation: self
                .rotation
                .as_ref()
                .map_or(rotation, |track| track.sample(time)),
            scale: self
                .scale
                .as_ref()
                .map_or(base.scale, |track| track.sample(time)),
        };
        if !transform.translation.is_finite() || !transform.scale.is_finite() {
            return Err(AnimationError::InvalidBaseTransform);
        }
        Ok(transform)
    }
}

fn validate_keys<T>(keys: &[Keyframe<T>]) -> Result<(), AnimationError> {
    if keys.is_empty() {
        return Err(AnimationError::EmptyTrack);
    }
    if keys.windows(2).any(|pair| pair[0].time >= pair[1].time) {
        return Err(AnimationError::InvalidKeyTimes);
    }
    Ok(())
}

fn sample_pair<T: Copy>(
    keys: &[Keyframe<T>],
    time: Duration,
    interpolate: impl FnOnce(T, T, f32) -> T,
) -> T {
    if time <= keys[0].time {
        return keys[0].value;
    }
    let right_index = keys.partition_point(|key| key.time <= time);
    if right_index == keys.len() {
        return keys[keys.len() - 1].value;
    }
    let left = keys[right_index - 1];
    if left.interpolation == Interpolation::Step {
        return left.value;
    }
    let right = keys[right_index];
    let elapsed = (time - left.time).as_secs_f64();
    let span = (right.time - left.time).as_secs_f64();
    interpolate(left.value, right.value, (elapsed / span) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, PI};

    #[test]
    fn vector_track_clamps_endpoints_and_is_independent_of_sample_order() {
        let track = Vec3Track::new([
            Keyframe::new(Duration::from_secs(1), Vec3::ZERO),
            Keyframe::new(Duration::from_secs(3), Vec3::X * 2.0),
        ])
        .unwrap();
        assert_eq!(track.sample(Duration::ZERO), Vec3::ZERO);
        assert_eq!(track.sample(Duration::from_secs(2)), Vec3::X);
        assert_eq!(track.sample(Duration::from_secs(4)), Vec3::X * 2.0);
        let _ = track.sample(Duration::from_secs(1));
        assert_eq!(track.sample(Duration::from_secs(2)), Vec3::X);
    }

    #[test]
    fn step_track_holds_the_left_key_until_the_next_key() {
        let track = Vec3Track::new([
            Keyframe::new(Duration::ZERO, Vec3::X).with_interpolation(Interpolation::Step),
            Keyframe::new(Duration::from_secs(1), Vec3::Y),
        ])
        .unwrap();
        assert_eq!(track.sample(Duration::from_millis(999)), Vec3::X);
        assert_eq!(track.sample(Duration::from_secs(1)), Vec3::Y);
    }

    #[test]
    fn rotation_track_uses_shortest_arc_and_normalized_keys() {
        let track = RotationTrack::new([
            Keyframe::new(
                Duration::ZERO,
                Quat::from_axis_angle(Vec3::Y, 350.0 * PI / 180.0).unwrap(),
            ),
            Keyframe::new(
                Duration::from_secs(1),
                Quat::from_axis_angle(Vec3::Y, 10.0 * PI / 180.0).unwrap(),
            ),
        ])
        .unwrap();
        let halfway = track.sample(Duration::from_millis(500));
        let expected = Quat::from_axis_angle(Vec3::Y, 2.0 * PI).unwrap();
        let alignment = halfway.x * expected.x
            + halfway.y * expected.y
            + halfway.z * expected.z
            + halfway.w * expected.w;
        assert!((alignment.abs() - 1.0).abs() < 1.0e-5);

        let half_turn = RotationTrack::new([
            Keyframe::new(Duration::ZERO, Quat::IDENTITY),
            Keyframe::new(
                Duration::from_secs(1),
                Quat::from_axis_angle(Vec3::Y, FRAC_PI_2).unwrap(),
            ),
        ])
        .unwrap();
        assert!(
            half_turn
                .sample(Duration::from_millis(500))
                .normalized()
                .is_some()
        );
    }

    #[test]
    fn tracks_reject_empty_duplicate_and_invalid_values() {
        assert_eq!(Vec3Track::new([]), Err(AnimationError::EmptyTrack));
        assert_eq!(
            Vec3Track::new([
                Keyframe::new(Duration::ZERO, Vec3::ZERO),
                Keyframe::new(Duration::ZERO, Vec3::X),
            ]),
            Err(AnimationError::InvalidKeyTimes)
        );
        assert_eq!(
            Vec3Track::new([Keyframe::new(Duration::ZERO, Vec3::new(f32::NAN, 0.0, 0.0),)]),
            Err(AnimationError::NonFiniteVector)
        );
        assert_eq!(
            RotationTrack::new([Keyframe::new(Duration::ZERO, Quat::new(0.0, 0.0, 0.0, 0.0))]),
            Err(AnimationError::InvalidRotation)
        );
    }
}
