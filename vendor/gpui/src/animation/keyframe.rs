use super::{animatable::Animatable, easing::Easing};

/// A single keyframe in a keyframe animation.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe<T> {
    /// Keyframe progress.
    pub offset: f32,
    /// Keyframe value.
    pub value: T,
    /// Easing used for the segment that starts at this keyframe.
    pub easing: Option<Easing>,
}

impl<T> Keyframe<T> {
    /// Create a keyframe.
    pub fn new(offset: f32, value: T) -> Self {
        Self {
            offset: normalize_offset(offset),
            value,
            easing: None,
        }
    }

    /// Set the easing used for the segment that starts at this keyframe.
    pub fn ease(mut self, easing: Easing) -> Self {
        self.easing = Some(easing);
        self
    }
}

/// A minimal keyframe track.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyframeTrack<T> {
    keyframes: Vec<Keyframe<T>>,
}

impl<T> KeyframeTrack<T> {
    /// Create a track from a set of keyframes.
    pub fn new(mut keyframes: Vec<Keyframe<T>>) -> Self {
        for keyframe in &mut keyframes {
            keyframe.offset = normalize_offset(keyframe.offset);
        }
        keyframes.sort_by(|left, right| left.offset.total_cmp(&right.offset));
        Self { keyframes }
    }
}

impl<T: Animatable> KeyframeTrack<T> {
    /// Sample the track at progress `progress`.
    pub fn sample(&self, progress: f32) -> Option<T> {
        let progress = normalize_offset(progress);
        let first = self.keyframes.first()?;
        let last = self.keyframes.last()?;
        if progress <= first.offset {
            return Some(first.value.clone());
        }
        if progress >= last.offset {
            return Some(last.value.clone());
        }

        let right_index = self.upper_bound(progress);
        let left = &self.keyframes[right_index.saturating_sub(1)];
        let right = &self.keyframes[right_index];
        let span = (right.offset - left.offset).max(f32::MIN_POSITIVE);
        let local = (progress - left.offset) / span;
        let eased = left
            .easing
            .as_ref()
            .map_or(local, |easing| easing.sample(local));
        Some(T::interpolate(&left.value, &right.value, eased))
    }

    fn upper_bound(&self, progress: f32) -> usize {
        let mut lower = 0;
        let mut upper = self.keyframes.len();
        while lower < upper {
            let middle = lower + (upper - lower) / 2;
            if self.keyframes[middle].offset <= progress {
                lower = middle + 1;
            } else {
                upper = middle;
            }
        }
        lower
    }
}

fn normalize_offset(offset: f32) -> f32 {
    if offset.is_nan() {
        0.0
    } else {
        offset.clamp(0.0, 1.0)
    }
}
