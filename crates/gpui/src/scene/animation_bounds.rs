use super::{PaintOperation, Scene, SceneAnimationId, SceneAnimationValue};
use crate::{Bounds, ScaledPixels, TransitionProperty};

impl Scene {
    pub(crate) fn animation_sample_is_identity(value: &SceneAnimationValue) -> bool {
        let progress = if value.progress.is_finite() {
            value.progress
        } else {
            0.
        };
        let sampled: [f32; 4] = std::array::from_fn(|index| {
            value.from[index] + (value.to[index] - value.from[index]) * progress
        });
        match value.property {
            TransitionProperty::Opacity => sampled[0].clamp(0., 1.) == 1.,
            TransitionProperty::Translation => {
                sampled[0] == 0.
                    && sampled[1] == 0.
                    && (sampled[3] <= 0.5 || sampled[2].clamp(0., 1.) == 1.)
            }
            TransitionProperty::Scale => sampled[0] == 1.,
            TransitionProperty::Transform => sampled[0] == 1. && sampled[1].clamp(0., 1.) == 1.,
            TransitionProperty::Rotation => sampled[0] == 0.,
            TransitionProperty::VisualState => {
                sampled[0] == 0.
                    && sampled[1] == 0.
                    && sampled[2] == 1.
                    && sampled[3].clamp(0., 1.) == 1.
            }
            _ => false,
        }
    }

    /// A zero-radius capture with an identity coordinate mapping can propagate
    /// child damage without repainting its entire output. Other mappings and
    /// filters keep the conservative full-capture damage path.
    pub(crate) fn capture_preserves_source_coordinates<'a>(
        radius: ScaledPixels,
        animation_id: Option<SceneAnimationId>,
        values: impl IntoIterator<Item = &'a SceneAnimationValue>,
    ) -> bool {
        if radius.0 != 0. {
            return false;
        }
        let Some(animation_id) = animation_id else {
            return true;
        };
        let mut seen = smallvec::SmallVec::<[TransitionProperty; 4]>::new();
        for value in values
            .into_iter()
            .filter(|value| value.animation_id == animation_id)
        {
            if seen.contains(&value.property) {
                continue;
            }
            seen.push(value.property);
            let sampled: [f32; 4] = std::array::from_fn(|index| {
                value.from[index] + (value.to[index] - value.from[index]) * value.progress
            });
            let identity = match value.property {
                TransitionProperty::Opacity | TransitionProperty::ClipReveal => true,
                TransitionProperty::Translation => sampled[0] == 0. && sampled[1] == 0.,
                TransitionProperty::Scale | TransitionProperty::Transform => sampled[0] == 1.,
                TransitionProperty::Rotation => sampled[0] == 0.,
                TransitionProperty::VisualState => {
                    sampled[0] == 0. && sampled[1] == 0. && sampled[2] == 1.
                }
                _ => false,
            };
            if !identity {
                return false;
            }
        }
        // Missing values disable the indexed GPU slot and leave the base
        // geometry unchanged; CPU animation fallback has the same semantics.
        true
    }

    /// Returns the tight visual bounds of root-scene paint operations owned by one retained
    /// animation id. Structural layout layers are intentionally ignored because they do not
    /// produce pixels and may cover the whole viewport while animated content is local.
    pub(crate) fn animation_visual_bounds(
        &self,
        animation_id: SceneAnimationId,
    ) -> Option<Bounds<ScaledPixels>> {
        self.paint_operations
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::Primitive(primitive)
                    if primitive.animation_id() == Some(animation_id) =>
                {
                    operation.visual_bounds()
                }
                PaintOperation::StartBlur(capture)
                    if capture.animation_id == Some(animation_id) =>
                {
                    operation.visual_bounds()
                }
                PaintOperation::Primitive(_)
                | PaintOperation::StartLayer(_)
                | PaintOperation::EndLayer
                | PaintOperation::StartBlur(_)
                | PaintOperation::EndBlur => None,
            })
            .reduce(|left, right| left.union(&right))
    }
}
