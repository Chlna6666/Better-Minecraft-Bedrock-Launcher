use super::*;
use crate::{RetainedViewRecipe, window::frame::RetainedPaintContext};

#[derive(Clone)]
pub(super) struct CommittedViewRecipe {
    owner: EntityId,
    recipe: RetainedViewRecipe,
    context: RetainedPaintContext,
    viewport: Size<Pixels>,
    reused: bool,
}

impl Window {
    /// Admission happens before layout. Only explicit repeatable recipes are
    /// retained; a miss executes the original Render and normal lifecycle.
    pub(crate) fn reuse_view_recipe(&mut self, owner: EntityId) -> Option<RetainedViewRecipe> {
        if self.view_recipes.is_empty()
            || cfg!(gpui_hot_reload)
            || self.force_view_cache_refresh()
            || self.view_dirty_scope(owner).is_some()
            || !self.retained_replay_is_targeted()
        {
            return None;
        }
        let key = self.current_stable_layout_element_id()?;
        if self.invalidator.retained_route_path_is_dirty(&key, &|_| true)
            || self.invalidator.requires_reconciliation_for_route(&key, &|_| true)
        {
            return None;
        }
        let previous = self.view_recipes.get(&key)?;
        if previous.owner != owner
            || previous.viewport != self.viewport_size
            || previous.context != self.current_retained_paint_context()
            || self.rendered_frame.dispatch_tree.view_path_reversed(owner).next().is_none()
        {
            return None;
        }
        let mut previous = previous.clone();
        previous.reused = true;
        let recipe = previous.recipe.clone();
        self.next_view_recipes.insert(key, previous);
        // Render did not repeat its dependency reads. The existing committed
        // replay preservation path must retain those still-mounted edges.
        self.replayed_dependency_ranges = true;
        Some(recipe)
    }

    pub(crate) fn record_view_recipe(&mut self, owner: EntityId, recipe: RetainedViewRecipe) {
        let Some(key) = self.current_stable_layout_element_id() else { return; };
        let entry = CommittedViewRecipe {
            owner,
            recipe,
            context: self.current_retained_paint_context(),
            viewport: self.viewport_size,
            reused: false,
        };
        self.next_view_recipes.insert(key, entry);
    }

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
    ) -> Option<super::frame::RetainedReplayRanges> {
        if cfg!(gpui_hot_reload)
            || self.force_view_cache_refresh()
            || !self.invalidator.active_targeted_replay()
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
            || range.bounds != bounds
            || range.layout_fingerprint != fingerprint
            || range.paint_context.as_ref() != &self.current_retained_paint_context()
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
        Some(super::frame::RetainedReplayRanges {
            prepaint_range: range.prepaint_range.clone(),
            paint_range: range.paint_range.clone(),
            metadata_range: range.metadata_range.clone(),
        })
    }

    /// Fixed slots have an independent layout root. Reuse the whole committed
    /// lifecycle only if the slot's actual geometry and inherited paint state
    /// match and all descendants have a known static replay contract.
    pub(crate) fn fixed_view_replay(
        &self,
        key: &GlobalElementId,
        bounds: Bounds<Pixels>,
        fingerprint: Option<u64>,
    ) -> Option<(EntityId, super::frame::RetainedReplayRanges, Point<Pixels>)> {
        let entry = self.next_view_recipes.get(key)?;
        if !entry.reused || entry.recipe.fixed_size().is_none()
            || self.force_view_cache_refresh()
            || fingerprint.is_none()
        {
            return None;
        }
        let range = self.rendered_frame.retained_element_ranges.get(key)?;
        if !range.identity_stable
            || range.bounds.size != bounds.size
            || range.layout_fingerprint != fingerprint
            || range.paint_context.as_ref() != &self.current_retained_paint_context()
            || range.prepaint_range.start.deferred_draws_index != range.prepaint_range.end.deferred_draws_index
            || range.prepaint_range.start.tooltips_index != range.prepaint_range.end.tooltips_index
            || range.prepaint_range.start.accessed_element_states_index != range.prepaint_range.end.accessed_element_states_index
            || range.paint_range.start.accessed_element_states_index != range.paint_range.end.accessed_element_states_index
            || range.paint_range.start.mouse_listeners_index != range.paint_range.end.mouse_listeners_index
            || range.paint_range.start.input_handlers_index != range.paint_range.end.input_handlers_index
            || range.paint_range.start.tab_handle_index != range.paint_range.end.tab_handle_index
            || range.paint_range.start.cursor_styles_index != range.paint_range.end.cursor_styles_index
            || range.paint_range.start.window_control_hitboxes_index != range.paint_range.end.window_control_hitboxes_index
        {
            return None;
        }
        let descendants = self.rendered_frame.retained_element_order.get(range.metadata_range.clone())?;
        let (root, descendants) = descendants.split_last()?;
        if self.rendered_frame.retained_element_ranges.get(root)?.metadata_range != range.metadata_range
            || descendants.is_empty()
            || !descendants.iter().all(|key| self.rendered_frame.retained_element_ranges.get(key).is_some_and(|child| {
                child.identity_stable && child.subtree_stable && child.semantic_descriptor.is_some()
            }))
            || !self.can_reuse_prepaint(&range.prepaint_range)
            || !self.can_reuse_paint(&range.paint_range)
        {
            return None;
        }
        let delta = bounds.origin - range.bounds.origin;
        if delta != Point::default() && !self.can_translate_static_range(range, bounds, delta) {
            return None;
        }
        Some((entry.owner, super::frame::RetainedReplayRanges {
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

    pub(super) fn commit_view_recipes(&mut self) {
        // Remove unmounted entries and entries with ambiguous/uncommitted
        // lifecycle metadata. The candidate table is bounded by this frame.
        self.next_view_recipes.retain(|key, entry| {
            self.rendered_frame.retained_element_ranges.get(key)
                .is_some_and(|range| range.identity_stable)
                && self.rendered_frame.dispatch_tree.view_path_reversed(entry.owner).next().is_some()
        });
        mem::swap(&mut self.view_recipes, &mut self.next_view_recipes);
        self.next_view_recipes.clear();
    }
}
