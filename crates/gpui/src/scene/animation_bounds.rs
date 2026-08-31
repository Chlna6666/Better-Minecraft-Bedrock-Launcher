use super::{PaintOperation, Scene, SceneAnimationId};
use crate::{Bounds, ScaledPixels};

impl Scene {
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
