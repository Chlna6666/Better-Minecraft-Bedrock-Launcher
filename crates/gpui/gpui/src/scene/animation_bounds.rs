use super::{PaintOperationRef, Scene, SceneAnimationId};
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
            .views()
            .filter_map(|operation| match operation {
                PaintOperationRef::Primitive(primitive)
                    if primitive.animation_id() == Some(animation_id) =>
                {
                    operation.visual_bounds()
                }
                PaintOperationRef::StartBlur(capture)
                    if capture.animation_id == Some(animation_id) =>
                {
                    operation.visual_bounds()
                }
                PaintOperationRef::Primitive(_)
                | PaintOperationRef::StartLayer(_)
                | PaintOperationRef::EndLayer
                | PaintOperationRef::StartBlur(_)
                | PaintOperationRef::EndBlur
                | PaintOperationRef::DeferredScroll(_) => None,
            })
            .reduce(|left, right| left.union(&right))
    }
}
