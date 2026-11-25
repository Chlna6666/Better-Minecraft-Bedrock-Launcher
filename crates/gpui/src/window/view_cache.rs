use super::*;

impl Window {
    /// The explicit GPUI cached API supplies layout independently of Render.
    /// Its unchanged stationary subtree may include listeners and state, but
    /// not nested frame-local cache owners or deferred/tooltip transfers.
    pub(crate) fn explicit_cached_view_replay(
        &self,
        owner: EntityId,
        key: &GlobalElementId,
        bounds: Bounds<Pixels>,
        fingerprint: Option<u64>,
        cx: &App,
    ) -> Option<(super::frame::RetainedReplayRanges, Point<Pixels>)> {
        if cfg!(gpui_hot_reload)
            || self.force_view_cache_refresh()
            || self.view_dirty_scope(owner).is_some()
            || cx.has_active_drag()
            || self.is_inspector_picking(cx)
            || fingerprint.is_none()
            || self.invalidator.retained_route_path_is_dirty(key, &|_| true)
            || self.invalidator.requires_reconciliation_for_route(key, &|_| true)
        {
            return None;
        }
        let range = self.rendered_frame.retained_element_ranges.get(key)?;
        if !range.explicit_cached_view || !range.identity_stable
            || range.bounds.size != bounds.size
            || range.layout_fingerprint != fingerprint
            || !self.retained_paint_context_matches(range.paint_context.as_ref())
            || self.rendered_frame.dispatch_tree.view_path_reversed(owner).next().is_none()
            || range.prepaint_range.start.deferred_draws_index != range.prepaint_range.end.deferred_draws_index
            || range.prepaint_range.start.tooltips_index != range.prepaint_range.end.tooltips_index
            || !self.can_reuse_prepaint(&range.prepaint_range)
            || !self.can_reuse_paint(&range.paint_range)
        {
            return None;
        }
        let (root, descendants) = self.rendered_frame.retained_element_order
            .get(range.metadata_range.clone())?.split_last()?;
        if self.rendered_frame.retained_element_ranges.get(root)?.metadata_range != range.metadata_range
            || !descendants.iter().enumerate().all(|(i, key)| {
                self.rendered_frame.retained_element_ranges.get(key).is_some_and(|child| {
                    !child.replay_barrier && child.metadata_range.end == range.metadata_range.start + i + 1
                })
            })
        {
            return None;
        }
        let delta = bounds.origin - range.bounds.origin;
        if delta != Point::default()
            && (super::draw_reuse::retained_range_has_stateful_effects(range)
                || !self.can_translate_static_range(range, bounds, delta))
        {
            return None;
        }
        Some((super::frame::RetainedReplayRanges {
            prepaint_range: range.prepaint_range.clone(),
            paint_range: range.paint_range.clone(),
            metadata_range: range.metadata_range.clone(),
        }, delta))
    }

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
