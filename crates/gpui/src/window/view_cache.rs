use super::*;

impl Window {
    // Translation is deliberately narrower than stationary replay: only
    // static quads and glyphs under an unchanged rectangular ancestor clip, with all
    // pixels inside both slot positions. The paint list must keep its exact
    // cardinality, so a newly clipped or newly visible primitive is a miss.
    pub(super) fn can_translate_static_range(
        &self,
        range: &super::frame::RetainedElementRange,
        bounds: Bounds<Pixels>,
        delta: Point<Pixels>,
    ) -> bool {
        let context = range.paint_context.as_ref();
        let scale = context.device_scale_factor;
        let dx = delta.x.0 * scale;
        let dy = delta.y.0 * scale;
        if context.scale != 1.0 || context.translation != Point::default()
            || !dx.is_finite() || !dy.is_finite()
            || dx != dx.round() || dy != dy.round()
            || context.content_mask != context.visual_content_mask
            || context.content_mask.corner_radii != Corners::default()
            || range.bounds.intersect(&context.content_mask.bounds) != range.bounds
            || bounds.intersect(&context.content_mask.bounds) != bounds
        {
            return false;
        }
        let clip = context.content_mask.scale(scale);
        let old_slot = range.bounds.scale(scale);
        let new_slot = bounds.scale(scale);
        let scaled_delta = point(ScaledPixels(dx), ScaledPixels(dy));
        let Some(operations) = self.rendered_frame.scene().paint_operations.get_range(
            range.paint_range.start.scene_index..range.paint_range.end.scene_index
        ) else { return false; };
        if !operations.iter().all(|operation| {
            let crate::PaintOperation::Primitive(primitive) = operation else { return false; };
            let Some(translated) = primitive.translated_static(scaled_delta) else { return false; };
            let original = *primitive.bounds();
            let moved = *translated.bounds();
            // Glyph raster bounds can extend slightly outside the text
            // element (overhang/antialiasing). Require full visibility in
            // the unchanged clip rather than forcing glyphs inside the slot.
            let inside_slots = matches!(primitive, crate::Primitive::MonochromeSprite(_) | crate::Primitive::Underline(_))
                || (original.intersect(&old_slot) == original && moved.intersect(&new_slot) == moved);
            *primitive.content_mask() == clip
                && !original.is_empty()
                && inside_slots
                && original.intersect(&clip.bounds) == original
                && moved.intersect(&clip.bounds) == moved
        }) {
            return false;
        }
        let hitboxes = &self.rendered_frame.hitboxes[
            range.prepaint_range.start.hitboxes_index..range.prepaint_range.end.hitboxes_index
        ];
        if !hitboxes.iter().all(|hitbox| {
            hitbox.content_mask == context.content_mask
                && hitbox.bounds.intersect(&range.bounds) == hitbox.bounds
        }) {
            return false;
        }
        // Internal scroll clips or transformed descendants require their own
        // coordinate spaces. Keep the initial translation implementation flat.
        self.rendered_frame.retained_element_order[range.metadata_range.clone()].iter().all(|key| {
            self.rendered_frame.retained_element_ranges.get(key).is_some_and(|child| {
                child.paint_context.as_ref() == context
                    && child.bounds.intersect(&range.bounds) == child.bounds
            })
        })
    }

    pub(crate) fn reuse_prepaint_at(
        &mut self,
        range: Range<PrepaintStateIndex>,
        delta: Point<Pixels>,
    ) -> bool {
        let start = self.next_frame.hitboxes.len();
        if !self.reuse_prepaint(range) {
            return false;
        }
        if delta != Point::default() {
            for hitbox in &mut self.next_frame.hitboxes[start..] {
                hitbox.bounds.origin += delta;
            }
        }
        true
    }

}
