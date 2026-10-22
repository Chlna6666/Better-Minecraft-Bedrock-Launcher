use super::*;
use crate::div;

fn record_draw_phase_metrics(metrics: FramePhaseMetrics) {
    record_first_frame_build_time(metrics.build);
    record_first_frame_layout_time(metrics.layout);
    record_first_frame_prepaint_time(metrics.prepaint);
    record_first_frame_paint_time(metrics.paint);
    record_first_frame_scene_finish_time(metrics.scene_finish);
    record_frame_phase_metrics(metrics);
}

pub(super) fn deadline_remaining_micros(deadline: Option<Instant>) -> Option<i64> {
    let deadline = deadline?;
    let now = Instant::now();
    let remaining = if deadline >= now {
        deadline
            .duration_since(now)
            .as_micros()
            .min(i64::MAX as u128) as i64
    } else {
        -(now
            .duration_since(deadline)
            .as_micros()
            .min(i64::MAX as u128) as i64)
    };
    Some(remaining)
}

/// When a view gains or loses retained paint fragments, keep visually identical
/// prefix/suffix fragments out of the damage set. Retained View bounds are often
/// the entire page even when the inserted fragment is a small popup.
fn for_each_changed_fragment_count_bounds(
    previous_scene: &Scene,
    previous_segments: &[&RetainedSceneSegment],
    current_scene: &Scene,
    current_segments: &[&RetainedSceneSegment],
    mut visit: impl FnMut(Bounds<ScaledPixels>),
) {
    fn unchanged(
        previous_scene: &Scene,
        previous: &RetainedSceneSegment,
        current_scene: &Scene,
        current: &RetainedSceneSegment,
    ) -> bool {
        if previous.bounds != current.bounds {
            return false;
        }
        let mut changed = false;
        crate::paint::for_each_changed_bounds(
            current_scene,
            current.scene_range.clone(),
            previous_scene,
            previous.scene_range.clone(),
            |_| changed = true,
        ) && !changed
    }

    let common = previous_segments.len().min(current_segments.len());
    let mut prefix = 0;
    while prefix < common
        && unchanged(
            previous_scene,
            previous_segments[prefix],
            current_scene,
            current_segments[prefix],
        )
    {
        prefix += 1;
    }

    let mut suffix = 0;
    while suffix < common - prefix
        && unchanged(
            previous_scene,
            previous_segments[previous_segments.len() - suffix - 1],
            current_scene,
            current_segments[current_segments.len() - suffix - 1],
        )
    {
        suffix += 1;
    }

    // Use actual painter bounds, not the enclosing View/layout rectangle.
    // Scene::bounds_for_range includes isolated blur groups and filter margins;
    // empty paint fragments do not contribute pixel damage.
    for fragment in &previous_segments[prefix..previous_segments.len() - suffix] {
        if let Some(bounds) = previous_scene.bounds_for_range(fragment.scene_range.clone()) {
            visit(bounds);
        }
    }
    for fragment in &current_segments[prefix..current_segments.len() - suffix] {
        if let Some(bounds) = current_scene.bounds_for_range(fragment.scene_range.clone()) {
            visit(bounds);
        }
    }
}

impl Window {
    /// Produces a new frame and assigns it to `rendered_frame`. To actually show
    /// the contents of the new [`Scene`], use [`Self::present`].
    #[profiling::function]
    pub fn draw(&mut self, cx: &mut App) -> ArenaClearNeeded {
        let _element_arena_scope = ElementArenaScope::enter();
        let frame_started_at = Instant::now();
        // The geometry this frame is laid out with is fixed for its whole
        // lifetime; a native resize arriving later is a new generation.
        self.next_frame_epoch = self.viewport_epoch;
        let previous_scene_was_empty = self.rendered_frame.scene().len() == 0;
        let debug_force_full_redraw = self.begin_debug_visualization_frame(cx);
        let force_full_redraw = self.force_full_redraw.get() || debug_force_full_redraw;
        let (restored_input_handler_index, directly_dirty_views) = self.begin_draw_cycle(cx);
        let mut phase_metrics = self.draw_roots(cx);
        self.next_frame.window_active = self.active.get();

        if self.draw_was_degraded && self.has_completed_rendered_frame {
            let result = self.finish_degraded_draw(restored_input_handler_index);
            phase_metrics.build = frame_started_at.elapsed();
            record_draw_phase_metrics(phase_metrics);
            return result;
        }

        // Register requested input handler with the platform window.
        if let Some(input_handler) = self
            .next_frame
            .input_handlers
            .iter_mut()
            .rev()
            .find_map(|handler| handler.take())
        {
            self.platform_window.set_input_handler(input_handler);
        }

        phase_metrics.scene_finish = self.finish_completed_draw(
            previous_scene_was_empty,
            force_full_redraw,
            &directly_dirty_views,
            cx,
        );
        phase_metrics.build = frame_started_at.elapsed();
        self.record_debug_frame_time(phase_metrics.build, cx);
        record_draw_phase_metrics(phase_metrics);
        ArenaClearNeeded::new()
    }

    fn begin_draw_cycle(&mut self, cx: &mut App) -> (Option<usize>, SmallVec<[EntityId; 8]>) {
        let was_dirty = self.invalidator.is_dirty();
        let frame_budget = self.frame_throttle.frame_budget();
        self.dirty_frame_scheduled = false;
        self.draw_deadline = Some(Instant::now() + frame_budget);
        self.draw_was_degraded = false;
        self.last_generation_stats.prepaint_breakdown = state::PrepaintWorkBreakdown::default();
        self.last_generation_stats
            .deadline_remaining_at_prepaint_start_us = None;
        self.last_generation_stats
            .deadline_remaining_at_layout_start_us = None;
        self.last_generation_stats
            .deadline_remaining_at_paint_start_us = None;
        record_window_layout_recompute(self.handle.window_id().as_u64());
        if let Some(started_at) = self.invalidator.take_dirty_to_present_start() {
            self.active_dirty_to_present_started_at = Some(
                self.active_dirty_to_present_started_at
                    .map_or(started_at, |active| active.min(started_at)),
            );
        }
        let directly_dirty_views = self.invalidate_entities();
        self.pending_list_measured_items = 0;
        cx.entities.clear_accessed();
        debug_assert!(self.rendered_entity_stack.is_empty());
        debug_assert!(self.view_bounds_stack.is_empty());
        // Defensive: an unwound draw could leave stale frames behind.
        self.view_bounds_stack.clear();
        self.invalidator.set_dirty(false);
        // Window-level damage without retained provenance must still execute the root.
        // A clean draw or a targeted child update can reuse it, but an unnamed dirty edge cannot.
        if was_dirty && !self.invalidator.active_targeted_replay() {
            self.force_view_cache_refresh = true;
        }
        // Establish abort eligibility exactly once, before any Prepaint/Paint
        // handler can move state from rendered_frame. The per-element snapshot
        // decision and the eventual deadline-abort decision share this latch.
        // Time passing may clear the recent-input guard, but it must NEVER
        // retroactively make an already-running frame abortable.
        self.frame_can_degrade =
            !self.live_resize_active && self.progressive_degradation_conditions();
        self.requested_autoscroll = None;
        (self.restore_previous_input_handler(), directly_dirty_views)
    }

    fn restore_previous_input_handler(&mut self) -> Option<usize> {
        let Some(input_handler) = self.platform_window.take_input_handler() else {
            return None;
        };

        if let Some((index, slot)) = self
            .rendered_frame
            .input_handlers
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, handler)| handler.is_none())
        {
            *slot = Some(input_handler);
            Some(index)
        } else {
            let index = self.rendered_frame.input_handlers.len();
            self.rendered_frame.input_handlers.push(Some(input_handler));
            Some(index)
        }
    }

    fn finish_degraded_draw(
        &mut self,
        restored_input_handler_index: Option<usize>,
    ) -> ArenaClearNeeded {
        self.degraded_draw_count = self.degraded_draw_count.saturating_add(1);
        self.restore_input_handler_after_degraded_draw(restored_input_handler_index);
        self.finish_layout_and_text_frame();
        // An aborted frame never replaces the committed retained tree.
        // Drop only newly allocated/unmounted Taffy nodes, not cached views.
        if let Some(layout_engine) = self.layout_engine.as_mut() {
            layout_engine.retain_committed_elements(
                |id| self.rendered_frame.retained_element_ranges.contains_key(id),
                false,
            );
        }
        let frame_retained_capacity = self.next_frame.retained_capacity();
        let scene_metrics = self.next_frame.scene().frame_metrics();
        self.last_generation_stats.scene = scene_metrics;
        self.last_generation_stats.frame_retained_capacity = frame_retained_capacity;
        self.last_generation_stats.list_measured_items = self.pending_list_measured_items;
        record_frame_retained_capacity(frame_retained_capacity);
        record_scene_frame_metrics(scene_metrics);
        self.rendered_frame
            .rollback_replayed_input_handlers(&mut self.next_frame);
        self.rendered_frame
            .rollback_element_states(&mut self.next_frame);
        self.next_frame.clear();
        let targeted_retry = self.invalidator.retry_aborted_frame();
        self.refreshing = false;
        self.invalidator.set_phase(DrawPhase::None);
        // The committed retained ranges and dispatch listeners remain intact
        // after the speculative rollback. A targeted generation can retry
        // with its original dirty dependency scope, leaving unrelated
        // committed cache ranges replayable. Unknown/global invalidation or
        // an intervening notification still forces the previous safe barrier.
        self.force_view_cache_refresh |= !targeted_retry;
        self.recovering_degraded_draw = true;
        self.frame_can_degrade = false;
        self.draw_deadline = None;
        ArenaClearNeeded::new()
    }

    fn finish_completed_draw(
        &mut self,
        previous_scene_was_empty: bool,
        force_full_redraw: bool,
        directly_dirty_views: &[EntityId],
        cx: &mut App,
    ) -> Duration {
        let finalize_started_at = Instant::now();
        self.finish_layout_and_text_frame();
        let scene_finish_started_at = Instant::now();
        self.next_frame.finish(&mut self.rendered_frame);
        let scene_finish_time = scene_finish_started_at.elapsed();
        let scene_animation_values = self
            .animation_engine
            .borrow()
            .scene_values(self.animation_time());
        let accumulate_unpresented_damage = self.needs_present.get();
        let mut backdrop_blur_damage_plan = crate::paint::backdrop_blur_damage_plan(
            self.next_frame.scene(),
            self.rendered_frame.scene(),
            &scene_animation_values,
            self.presentation_state
                .animation_values_snapshot(self.rendered_frame.scene()),
        );
        if accumulate_unpresented_damage {
            backdrop_blur_damage_plan.merge_from(&self.backdrop_blur_damage_plan);
        }
        self.backdrop_blur_damage_plan = backdrop_blur_damage_plan;
        self.prepare_render_plan_for_next_frame(
            previous_scene_was_empty || force_full_redraw || self.draw_was_degraded,
            directly_dirty_views,
            accumulate_unpresented_damage,
        );
        let committed_layout_extent = (
            (self.viewport_size.width.0 * self.scale_factor)
                .round()
                .max(0.0) as u32,
            (self.viewport_size.height.0 * self.scale_factor)
                .round()
                .max(0.0) as u32,
        );
        let paint = self.next_frame.publish(PaintGeometry {
            layout_extent: Some(committed_layout_extent),
            layout_epoch: self.next_frame_epoch,
            scale_factor: self.scale_factor,
        });
        self.presentation_state
            .publish_pending(paint, scene_animation_values);
        let frame_retained_capacity = self.next_frame.retained_capacity();
        let scene_metrics = self.next_frame.scene().frame_metrics();
        self.last_generation_stats.scene = scene_metrics;
        self.last_generation_stats.frame_retained_capacity = frame_retained_capacity;
        self.last_generation_stats.list_measured_items = self.pending_list_measured_items;
        record_frame_retained_capacity(frame_retained_capacity);
        record_scene_frame_metrics(scene_metrics);

        self.invalidator.set_phase(DrawPhase::Focus);
        let previous_focus_path = self.rendered_frame.focus_path();
        let previous_window_active = self.rendered_frame.window_active;
        // Hitboxes, hit-test geometry and the scene become committed together:
        // pointer input must be interpreted in this generation, not in the
        // newest native one, or a fast edge drag tests live coordinates against
        // the previous layout.
        self.committed_epoch = self.next_frame_epoch;
        self.committed_scale_factor = self.scale_factor;
        self.platform_window.set_committed_geometry(
            self.committed_epoch,
            self.viewport_size,
            self.scale_factor,
        );
        mem::swap(&mut self.rendered_frame, &mut self.next_frame);
        #[cfg(target_os = "windows")]
        if force_full_redraw && crate::diagnostics::gpu_owner::resize_trace_enabled() {
            // Resizing a swapchain is not proof that the new Scene covers that extent.
            // A missing root cache invalidation can replay quads clipped to the old
            // viewport. Record the committed CPU Scene bounds, without a GPU readback.
            // Quads and sprites are measured separately because image content and its clip can
            // retain stale bounds even when the chrome quads track the new viewport.
            let scene = self.rendered_frame.scene();
            let (quad_right, quad_bottom) =
                scene
                    .quads
                    .iter()
                    .fold((0.0_f32, 0.0_f32), |(right, bottom), quad| {
                        (
                            right.max(quad.bounds.right().0),
                            bottom.max(quad.bounds.bottom().0),
                        )
                    });
            let (poly_right, poly_bottom, poly_clip_right, poly_clip_bottom) =
                scene.polychrome_sprites.iter().fold(
                    (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32),
                    |(right, bottom, clip_right, clip_bottom), sprite| {
                        (
                            right.max(sprite.bounds.right().0),
                            bottom.max(sprite.bounds.bottom().0),
                            clip_right.max(sprite.content_mask.bounds.right().0),
                            clip_bottom.max(sprite.content_mask.bounds.bottom().0),
                        )
                    },
                );
            let (mono_right, mono_bottom, mono_clip_right, mono_clip_bottom) =
                scene.monochrome_sprites.iter().fold(
                    (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32),
                    |(right, bottom, clip_right, clip_bottom), sprite| {
                        (
                            right.max(sprite.bounds.right().0),
                            bottom.max(sprite.bounds.bottom().0),
                            clip_right.max(sprite.content_mask.bounds.right().0),
                            clip_bottom.max(sprite.content_mask.bounds.bottom().0),
                        )
                    },
                );
            log::info!(
                target: "gpui_resize_trace",
                "gpui_resize stage=scene window_id={} physical_px={}x{} scene_primitives={} quads={} quad_right_px={:.1} quad_bottom_px={:.1} poly_sprites={} poly_right_px={:.1} poly_bottom_px={:.1} poly_clip_right_px={:.1} poly_clip_bottom_px={:.1} mono_sprites={} mono_right_px={:.1} mono_bottom_px={:.1} mono_clip_right_px={:.1} mono_clip_bottom_px={:.1} force_full_redraw=true epoch={}",
                self.handle.window_id().as_u64(),
                (self.viewport_size.width.0 * self.scale_factor).round() as u32,
                (self.viewport_size.height.0 * self.scale_factor).round() as u32,
                self.rendered_frame.scene().len(),
                scene.quads.len(),
                quad_right,
                quad_bottom,
                scene.polychrome_sprites.len(),
                poly_right,
                poly_bottom,
                poly_clip_right,
                poly_clip_bottom,
                scene.monochrome_sprites.len(),
                mono_right,
                mono_bottom,
                mono_clip_right,
                mono_clip_bottom,
                self.next_frame_epoch,
            );
        }
        // GPU/CPU retained fragments may skip the layout traversal entirely,
        // so native Taffy liveness follows the committed retained identities.
        if let Some(layout_engine) = self.layout_engine.as_mut() {
            layout_engine.retain_committed_elements(
                |id| self.rendered_frame.retained_element_ranges.contains_key(id),
                true,
            );
        }
        // Keep UI generations plus active/pending presentation snapshots live before the previous
        // frame becomes scratch. A still-active older publication keeps its payload while this frame starts a new builder.
        self.prune_static_image_atlas_residency();
        self.next_frame.clear_for_reuse(&self.rendered_frame);
        self.viewport_dependent_views
            .borrow_mut()
            .retain(|view_id| {
                !self
                    .rendered_frame
                    .dispatch_tree
                    .view_path_reversed(*view_id)
                    .next()
                    .is_none()
            });
        if self
            .animation_engine
            .borrow_mut()
            .retain_scene_animation_cache(self.rendered_frame.scene())
        {
            self.scene_animation_needs_commit.set(true);
        }
        let current_focus_path = self.rendered_frame.focus_path();
        let current_window_active = self.rendered_frame.window_active;

        self.emit_focus_change_events(
            previous_focus_path,
            previous_window_active,
            current_focus_path,
            current_window_active,
            cx,
        );

        debug_assert!(self.rendered_entity_stack.is_empty());
        self.record_entities_accessed(cx);
        self.reset_cursor_style(cx);
        self.refreshing = false;
        self.invalidator.set_phase(DrawPhase::None);
        self.force_full_redraw.set(false);
        self.force_view_cache_refresh = false;
        self.recovering_degraded_draw = false;
        self.frame_can_degrade = false;
        self.has_completed_rendered_frame = true;
        // A layout RAF may notify and rebuild a view after its sampled value has already
        // converged. Retain the newly built CPU scene, but do not encode, upload, or present an
        // identical framebuffer. Platform exposure and explicit presentation requests still use
        // the presentation-only path without coming through this flag.
        // A new animation generation is a compositor update even if the retained primitives
        // have identical pixels. Hand it off here before native presentation resumes the old packet.
        self.needs_present
            .set(self.scene_animation_needs_commit.get() || !self.render_dirty_region.is_empty());
        if self.draw_was_degraded {
            self.invalidator.set_dirty(true);
        } else {
            self.dirty_views.clear();
            self.direct_dirty_views.clear();
            self.animation_dirty_region = DirtyRegion::empty();
        }
        self.finish_debug_visualization_frame(cx);
        self.draw_deadline = None;
        self.last_generation_stats.scene_finish_elapsed = scene_finish_time;
        self.last_generation_stats.finalize_elapsed = finalize_started_at.elapsed();

        scene_finish_time
    }

    fn finish_layout_and_text_frame(&mut self) {
        let mut layout_metrics = LayoutFrameMetrics::default();
        let mut layout_cache_metrics = LayoutCacheFrameMetrics::default();
        if let Some(layout_engine) = self.layout_engine.as_mut() {
            let (layout_cache_hits, layout_cache_misses) = layout_engine.layout_cache_metrics();
            layout_metrics = layout_engine.frame_metrics();
            layout_cache_metrics = LayoutCacheFrameMetrics {
                hits: layout_cache_hits,
                misses: layout_cache_misses,
            };
            record_layout_frame_metrics(layout_metrics);
            record_layout_cache_metrics(layout_cache_hits, layout_cache_misses);
            layout_engine.clear();
        }
        let text_layout_metrics = self.text_system().finish_frame();
        self.last_generation_stats.layout = layout_metrics;
        self.last_generation_stats.layout_cache = layout_cache_metrics;
        self.last_generation_stats.text_layout = text_layout_metrics;
    }

    fn emit_focus_change_events(
        &mut self,
        previous_focus_path: SmallVec<[FocusId; 8]>,
        previous_window_active: bool,
        current_focus_path: SmallVec<[FocusId; 8]>,
        current_window_active: bool,
        cx: &mut App,
    ) {
        if previous_focus_path != current_focus_path
            || previous_window_active != current_window_active
        {
            if !previous_focus_path.is_empty() && current_focus_path.is_empty() {
                self.focus_lost_listeners
                    .clone()
                    .retain(&(), |listener| listener(self, cx));
            }

            let event = WindowFocusEvent {
                previous_focus_path: if previous_window_active {
                    previous_focus_path
                } else {
                    Default::default()
                },
                current_focus_path: if current_window_active {
                    current_focus_path
                } else {
                    Default::default()
                },
            };
            self.focus_listeners
                .clone()
                .retain(&(), |listener| listener(&event, self, cx));
        }
    }

    fn restore_input_handler_after_degraded_draw(&mut self, restored_index: Option<usize>) {
        let restored_input_handler = restored_index
            .and_then(|index| self.rendered_frame.input_handlers.get_mut(index))
            .and_then(Option::take)
            .or_else(|| {
                self.next_frame
                    .input_handlers
                    .iter_mut()
                    .rev()
                    .find_map(|handler| handler.take())
            });

        if let Some(input_handler) = restored_input_handler {
            self.platform_window.set_input_handler(input_handler);
        }
    }

    fn prepare_render_plan_for_next_frame(
        &mut self,
        force_full_redraw: bool,
        directly_dirty_views: &[EntityId],
        accumulate_unpresented_damage: bool,
    ) {
        let viewport = Bounds::new(Point::default(), self.viewport_size).scale(self.scale_factor);
        let mut dirty_region = if accumulate_unpresented_damage {
            mem::take(&mut self.render_dirty_region)
        } else {
            DirtyRegion::empty()
        };
        let accumulated_full = dirty_region.is_full();

        let scene_requires_full_redraw =
            crate::paint::requires_full_redraw_fallback(self.next_frame.scene());
        // Backdrop-filter output depends on pixels rendered before it. When a blur primitive is
        // added or removed (modal open/close is the common case), there is no current primitive on
        // one side of the transition whose spatial damage can represent all stale filtered pixels
        // from the other side. Pay for one full surface redraw on this low-frequency topology
        // transition; once the blur set is stable, ordinary source/hover damage remains spatial.
        let backdrop_blur_topology_changed = self.next_frame.scene().backdrop_blurs.len()
            != self.rendered_frame.scene().backdrop_blurs.len()
            || self.next_frame.scene().has_backdrop_blurs()
                != self.rendered_frame.scene().has_backdrop_blurs();
        // Root backdrop blurs (for example titlebar glass) have a well-defined
        // output clip in both scenes. A newly added/removed one only changes
        // pixels inside those clips. Nested element-blur captures can change
        // their composition dependencies, so they retain the full fallback.
        let local_root_blur_topology = backdrop_blur_topology_changed
            // Keep the full redraw fallback when the final backdrop disappears:
            // a transition to a blur-free scene may enable native partial present
            // before every rotating swapchain image contains initialized pixels.
            && self.next_frame.scene().has_backdrop_blurs()
            && self.next_frame.scene().blurs.is_empty()
            && self.rendered_frame.scene().blurs.is_empty();
        // Retained fragments are allowed to partition the Scene. Requiring a
        // *single* fragment to span every paint operation incorrectly forces
        // full damage on normal multi-view pages after one degraded frame.
        //
        // The committed previous Scene was never replaced by the aborted
        // generation. Diff the two completed painter streams directly; this
        // covers every visual operation regardless of the ownership partition.
        // A failed paint-range proof remains a conservative full redraw.
        let mut recovery_scene_damage = DirtyRegion::empty();
        let recovery_diff_failed = self.recovering_degraded_draw
            && !force_full_redraw
            && !scene_requires_full_redraw
            && !crate::paint::for_each_changed_bounds(
                self.next_frame.scene(),
                0..self.next_frame.scene().len(),
                self.rendered_frame.scene(),
                0..self.rendered_frame.scene().len(),
                |bounds| recovery_scene_damage.push(bounds),
            );
        let requires_full_redraw = force_full_redraw
            || scene_requires_full_redraw
            || (backdrop_blur_topology_changed && !local_root_blur_topology)
            || recovery_diff_failed;
        let mut spatial_area = dirty_region.area();
        let mut empty_damage_fallback = false;

        if requires_full_redraw {
            if scene_requires_full_redraw {
                crate::diagnostics::performance_metrics::record_full_redraw_fallback();
            }
            dirty_region.mark_full(viewport);
        } else {
            if local_root_blur_topology {
                // Preserve the union of OLD and NEW output clips. Removal needs
                // the old pixels restored; addition needs the new pixels filled.
                // The Gaussian source halo remains the filter cache's separate
                // responsibility via backdrop_blur_damage_plan.
                for blur in self
                    .rendered_frame
                    .scene()
                    .backdrop_blurs
                    .iter()
                    .chain(&self.next_frame.scene().backdrop_blurs)
                {
                    dirty_region.push(blur.bounds.intersect(&blur.content_mask.bounds));
                }
            }
            // `dirty_views` contains the ancestor path needed to reach dirty descendants during
            // retained reconciliation. That is a traversal/cache semantic, not a pixel-damage
            // semantic: treating every ancestor RetainedSceneSegment as changed makes a dirty root
            // segment turn a local child update into a full-window redraw. Compute damage from the
            // directly invalidated views instead and separately account for layout-induced moves.
            if self.recovering_degraded_draw {
                // One ordered painter diff already includes every changed
                // view/fragment and moved bound. Avoid repeating per-View
                // operation comparisons over the same freshly rebuilt Scene.
                for rect in recovery_scene_damage.rects() {
                    dirty_region.push(rect.bounds);
                }
            } else {
                self.add_direct_view_damage(&mut dirty_region, directly_dirty_views);
            }
            for rect in self.animation_dirty_region.rects() {
                dirty_region.push(rect.bounds);
            }

            for bounds in crate::paint::backdrop_blur_output_damage(
                self.next_frame.scene(),
                &self.backdrop_blur_damage_plan,
            ) {
                dirty_region.push(bounds);
            }

            spatial_area = dirty_region.area();
            if dirty_region.is_empty() && self.next_frame.retained_scene_segments.is_empty() {
                empty_damage_fallback = true;
                dirty_region.mark_full(viewport);
            } else if !dirty_region.is_empty() {
                dirty_region.coalesce_if_large(viewport, DIRTY_REGION_FULL_REDRAW_RATIO);
            }
        }

        log::trace!(target: "gpui::diagnostics",
            "GPUI damage decision: window_id={} scene_revision={} full={} forced={} scene_fallback={} blur_topology={} local_blur_topology={} recovery={} accumulated_full={} empty_fallback={} spatial_area={} direct_views={:?}",
            self.handle.window_id().as_u64(), self.next_frame.scene().revision,
            dirty_region.is_full(), force_full_redraw, scene_requires_full_redraw,
            backdrop_blur_topology_changed, local_root_blur_topology,
            recovery_diff_failed, accumulated_full, empty_damage_fallback,
            spatial_area, directly_dirty_views);

        if self.recovering_degraded_draw && dirty_region.is_full() {
            self.recovery_full_redraw_count = self.recovery_full_redraw_count.saturating_add(1);
        }
        self.render_present_mode = if dirty_region.is_full()
            || (dirty_region.is_empty() && self.next_frame.retained_scene_segments.is_empty())
        {
            PartialPresentMode::FullRedraw
        } else {
            PartialPresentMode::Partial
        };
        record_dirty_region_metrics(dirty_region.rect_count(), dirty_region.area() as usize);
        self.render_dirty_region = dirty_region;
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
    }

    fn add_direct_view_damage(
        &self,
        dirty_region: &mut DirtyRegion,
        directly_dirty_views: &[EntityId],
    ) {
        if directly_dirty_views.is_empty() {
            return;
        }

        let previous_entities = &self.rendered_frame.retained_scene_index;
        let current_entities = &self.next_frame.retained_scene_index;

        // Only directly invalidated views need operation comparisons here.
        // Degraded recovery uses a single whole-Scene ordered diff above.
        let mut diff_entity = |entity_id: &EntityId| {
            let previous = previous_entities.get(entity_id);
            let current = current_entities.get(entity_id);
            let diffed = previous.zip(current).is_some_and(|(previous, current)| {
                if previous.len() != current.len() {
                    // The fragment-count change is handled by the ordered painter diff
                    // below. Falling back to Entry::bounds here damages the entire
                    // containing View (often the full window).
                    return true;
                }
                // A View may own more than one independently retained scene
                // fragment (e.g. a page body plus stable chrome). The old
                // single-fragment requirement incorrectly marked the ENTIRE
                // View dirty when just one fragment changed. Compare each
                // paired fragment's painter operations and invalidate only
                // the old/new pixels produced by that fragment.
                //
                // If any fragment cannot be compared, the caller retains the
                // conservative old+new View bounds fallback below.
                previous
                    .segments(&self.rendered_frame.retained_scene_segments)
                    .zip(current.segments(&self.next_frame.retained_scene_segments))
                    .all(|(previous_segment, current_segment)| {
                        crate::paint::for_each_changed_bounds(
                            self.next_frame.scene(),
                            current_segment.scene_range.clone(),
                            self.rendered_frame.scene(),
                            previous_segment.scene_range.clone(),
                            |bounds| dirty_region.push(bounds),
                        )
                    })
            });
            if !diffed {
                if let Some(previous) = previous {
                    dirty_region.push(previous.bounds);
                }
                if let Some(current) = current {
                    dirty_region.push(current.bounds);
                }
            }
        };
        // A layout-driven spring may live inside a full-page view even
        // though only a small button changed. Compare the directly notified
        // view rather than damaging its entire containing region.
        for entity_id in directly_dirty_views {
            diff_entity(entity_id);
        }

        // Layout changes can move siblings even though those siblings were not directly notified.
        // Compare retained bounds across frames and damage only entities whose visual extent moved,
        // appeared, or disappeared. Stable ancestors such as MainWindow therefore remain clean.
        for (entity_id, current) in current_entities {
            match previous_entities.get(entity_id) {
                Some(previous) if previous.len() != current.len() => {
                    let old = previous
                        .segments(&self.rendered_frame.retained_scene_segments)
                        .collect::<SmallVec<[_; 4]>>();
                    let new = current
                        .segments(&self.next_frame.retained_scene_segments)
                        .collect::<SmallVec<[_; 4]>>();
                    for_each_changed_fragment_count_bounds(
                        self.rendered_frame.scene(),
                        &old,
                        self.next_frame.scene(),
                        &new,
                        |bounds| dirty_region.push(bounds),
                    );
                }
                Some(previous) => {
                    // A single moving fragment must not damage the other fragments
                    // of the same View, which may include an entire static background.
                    for (old, new) in previous
                        .segments(&self.rendered_frame.retained_scene_segments)
                        .zip(current.segments(&self.next_frame.retained_scene_segments))
                    {
                        if old.bounds != new.bounds {
                            dirty_region.push(old.bounds);
                            dirty_region.push(new.bounds);
                        }
                    }
                }
                None => {
                    for segment in current.segments(&self.next_frame.retained_scene_segments) {
                        dirty_region.push(segment.bounds);
                    }
                }
            }
        }
        for (entity_id, previous) in previous_entities {
            if !current_entities.contains_key(entity_id) {
                for segment in previous.segments(&self.rendered_frame.retained_scene_segments) {
                    dirty_region.push(segment.bounds);
                }
            }
        }
    }

    fn presentation_packet(&self) -> PresentationPacket {
        let paint = self
            .presentation_state
            .active_paint()
            .unwrap_or_else(|| self.rendered_frame.published_paint());
        let animation_engine = self.animation_engine.borrow();
        let timelines = animation_engine.presentation_timelines();
        drop(animation_engine);
        let mut packet = PresentationPacket::new(
            paint,
            self.presentation_state
                .active_engine_animation_values()
                .iter()
                .copied(),
            timelines,
            self.animation_time(),
            self.render_dirty_region.clone(),
            self.backdrop_blur_damage_plan.clone(),
            self.render_present_mode,
        );
        packet.window_id = self.handle.window_id().as_u64();
        packet.set_frame_interval(self.frame_clock.interval());
        packet
    }

    fn record_entities_accessed(&mut self, cx: &mut App) {
        let mut entities = mem::take(cx.entities.accessed_entities.get_mut());
        let handle = self.handle;
        cx.record_entities_accessed(
            handle,
            // Try moving window invalidator into the Window
            self.invalidator.clone(),
            &entities,
        );
        mem::swap(&mut entities, cx.entities.accessed_entities.get_mut());
    }

    fn invalidate_entities(&mut self) -> SmallVec<[EntityId; 8]> {
        let mut views = self.invalidator.take_views();
        for entity in views.drain() {
            self.mark_view_dirty(entity);
        }
        self.invalidator.replace_views(views);
        self.direct_dirty_views.iter().copied().collect()
    }

    /// Acknowledge a drawn frame in element tests using the real presentation path.
    #[cfg(test)]
    pub(crate) fn test_present(&mut self) -> PlatformFrameResult {
        self.present()
    }

    #[profiling::function]
    pub(super) fn present(&mut self) -> PlatformFrameResult {
        #[cfg(feature = "profiler")]
        let _profile = crate::diagnostics::foreground_profiler::ForegroundWorkSpan::submit(
            self.handle.window_id().as_u64(),
        );
        self.presentation_state.activate_pending();
        let result = self.platform_window.draw(self.presentation_packet());
        if result.is_accepted() {
            self.needs_present.set(false);
            self.scene_animation_needs_commit.set(false);
        }
        profiling::finish_frame!();
        result
    }

    pub(super) fn present_framebuffer_only(&mut self) -> PlatformFrameResult {
        #[cfg(feature = "profiler")]
        let _profile = crate::diagnostics::foreground_profiler::ForegroundWorkSpan::submit(
            self.handle.window_id().as_u64(),
        );
        self.presentation_state.activate_pending();
        let result = self
            .platform_window
            .present_framebuffer_only(self.presentation_packet());
        if result.is_accepted() {
            self.needs_present.set(false);
            self.scene_animation_needs_commit.set(false);
        }
        profiling::finish_frame!();
        result
    }

    fn draw_roots(&mut self, cx: &mut App) -> FramePhaseMetrics {
        self.last_generation_stats
            .deadline_remaining_at_prepaint_start_us =
            deadline_remaining_micros(self.draw_deadline);
        let prepaint_started_at = Instant::now();
        self.invalidator.set_phase(DrawPhase::Prepaint);
        self.tooltip_bounds.take();
        super::element_work::reset();

        let _inspector_width: Pixels = rems(30.0).to_pixels(self.rem_size());
        let root_size = {
            #[cfg(any(feature = "inspector", debug_assertions))]
            {
                self.viewport_size
            }
            #[cfg(not(any(feature = "inspector", debug_assertions)))]
            {
                self.viewport_size
            }
        };

        let root_prepaint_started_at = Instant::now();
        // Render the root view as an ordinary element subtree. Unchanged children reconcile
        // through the regular element retained replay proofs; there is no separate view cache
        // boundary wrapping the whole window.
        let mut root_element = div()
            .size_full()
            .child(self.root.as_ref().unwrap().clone())
            .into_any_element();
        let root_layout_elapsed = self.with_critical_draw(|window| {
            let layout_started_at = Instant::now(); // Profiling; visual sampling keeps the frame clock.
            root_element.layout_as_root(root_size.into(), window, cx);
            let layout_elapsed = layout_started_at.elapsed();
            root_element.prepaint_at(Point::default(), window, cx);
            layout_elapsed
        });
        let root_prepaint_elapsed = root_prepaint_started_at.elapsed();

        let deferred_prepaint_started_at = Instant::now();
        #[cfg(any(feature = "inspector", debug_assertions))]
        let inspector_element = if self.inspector.is_some() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
            None
        } else {
            self.prepaint_inspector(_inspector_width, cx)
        };

        if !self.next_frame.deferred_draws.is_empty() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else {
            self.prepaint_deferred_draws(cx);
        }
        let sorted_deferred_draws = if self.draw_was_degraded() {
            SmallVec::new()
        } else {
            self.deferred_draw_traversal_order()
        };

        let deferred_prepaint_elapsed = deferred_prepaint_started_at.elapsed();
        let overlay_prepaint_started_at = Instant::now();
        let mut prompt_element = None;
        let mut active_drag_element = None;
        let mut tooltip_element = None;
        let has_overlay_work = self.prompt.is_some()
            || cx.active_drag.is_some()
            || !self.next_frame.tooltip_requests.is_empty();
        if has_overlay_work && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else if let Some(prompt) = self.prompt.take() {
            let mut element = prompt.view.any_view().into_any_element();
            element.prepaint_as_root(Point::default(), root_size.into(), self, cx);
            prompt_element = Some(element);
            self.prompt = Some(prompt);
        } else if let Some(active_drag) = cx.active_drag.take() {
            let mut element = active_drag.view.clone().into_any_element();
            let offset = self.mouse_position() - active_drag.cursor_offset;
            element.prepaint_as_root(offset, AvailableSpace::min_size(), self, cx);
            active_drag_element = Some(element);
            cx.active_drag = Some(active_drag);
        } else {
            tooltip_element = self.prepaint_tooltip(cx);
        }

        let overlay_prepaint_elapsed = overlay_prepaint_started_at.elapsed();
        let hit_test_started_at = Instant::now();
        self.mouse_hit_test = self.next_frame.hit_test(self.mouse_position);
        let hit_test_elapsed = hit_test_started_at.elapsed();
        let prepaint_time = prepaint_started_at.elapsed();
        self.last_generation_stats.prepaint_breakdown = state::PrepaintWorkBreakdown {
            root: root_prepaint_elapsed,
            root_layout: root_layout_elapsed,
            deferred: deferred_prepaint_elapsed,
            overlay: overlay_prepaint_elapsed,
            hit_test: hit_test_elapsed,
        };
        // Detailed Trace is opt-in. No per-element scans, allocations or
        // synchronous logging are added to the normal 1ms hot path.
        // Match the actual event target when testing the Trace filter.
        // The module's own log level can be lower even when
        // gpui::diagnostics=trace is explicitly enabled, which previously
        // suppressed every per-frame Prepaint sample.
        if log::log_enabled!(target: "gpui::diagnostics", log::Level::Trace) {
            log::trace!(
                target: "gpui::diagnostics",
                "gpui prepaint work window={} root_us={} root_layout_us={} root_non_layout_us={} deferred_us={} overlay_us={} hit_test_us={} other_us={} total_us={}",
                self.handle.window_id().as_u64(),
                root_prepaint_elapsed.as_micros(),
                root_layout_elapsed.as_micros(),
                root_prepaint_elapsed.saturating_sub(root_layout_elapsed).as_micros(),
                deferred_prepaint_elapsed.as_micros(),
                overlay_prepaint_elapsed.as_micros(),
                hit_test_elapsed.as_micros(),
                self.last_generation_stats
                    .prepaint_breakdown
                    .other(prepaint_time)
                    .as_micros(),
                prepaint_time.as_micros(),
            );
        }

        let paint_started_at = Instant::now();
        self.last_generation_stats
            .deadline_remaining_at_paint_start_us = deadline_remaining_micros(self.draw_deadline);
        self.invalidator.set_phase(DrawPhase::Paint);
        self.with_critical_draw(|window| root_element.paint(window, cx));

        #[cfg(any(feature = "inspector", debug_assertions))]
        if inspector_element.is_some() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else {
            self.paint_inspector(inspector_element, cx);
        }

        if !sorted_deferred_draws.is_empty() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else {
            self.paint_deferred_draws(&sorted_deferred_draws, cx);
        }

        let has_overlay_element =
            prompt_element.is_some() || active_drag_element.is_some() || tooltip_element.is_some();
        if has_overlay_element && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else if let Some(mut prompt_element) = prompt_element {
            prompt_element.paint(self, cx);
        } else if let Some(mut drag_element) = active_drag_element {
            drag_element.paint(self, cx);
        } else if let Some(mut tooltip_element) = tooltip_element {
            tooltip_element.paint(self, cx);
        }

        #[cfg(any(feature = "inspector", debug_assertions))]
        self.paint_inspector_hitbox(cx);

        self.paint_debug_surface_update_flash(cx);
        self.paint_debug_frame_time_overlay(cx);
        self.last_generation_stats.prepaint_elapsed = prepaint_time;
        self.last_generation_stats.paint_elapsed = paint_started_at.elapsed();
        if super::element_work::enabled() {
            let element_work = super::element_work::take();
            self.last_generation_stats.element_prepaint = format!(
                "elements={} self_total_us={} top={:?} view_render={:?} root_layout={:?}",
                element_work.prepaint_elements,
                element_work
                    .prepaint
                    .iter()
                    .map(|(_, elapsed)| elapsed.as_micros())
                    .sum::<u128>(),
                element_work.prepaint,
                element_work.render,
                element_work.layout,
            );
            self.last_generation_stats.element_paint = format!(
                "elements={} self_total_us={} top={:?} range_replay={:?}",
                element_work.paint_elements,
                element_work
                    .paint
                    .iter()
                    .map(|(_, elapsed)| elapsed.as_micros())
                    .sum::<u128>(),
                element_work.paint,
                element_work.replay,
            );
        } else {
            // Avoid per-frame heap allocations and sorting when the invasive
            // element-level profiler has not been explicitly requested.
            self.last_generation_stats.element_prepaint.clear();
            self.last_generation_stats.element_paint.clear();
        }
        FramePhaseMetrics {
            layout: self
                .layout_engine
                .as_ref()
                .map_or(Duration::ZERO, TaffyLayoutEngine::frame_layout_time),
            prepaint: prepaint_time,
            paint: paint_started_at.elapsed(),
            ..FramePhaseMetrics::default()
        }
    }

    fn prepaint_tooltip(&mut self, cx: &mut App) -> Option<AnyElement> {
        for tooltip_request_index in (0..self.next_frame.tooltip_requests.len()).rev() {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let Some(Some(tooltip_request)) = self
                .next_frame
                .tooltip_requests
                .get(tooltip_request_index)
                .cloned()
            else {
                log::error!("Unexpectedly absent TooltipRequest");
                continue;
            };
            let mut element = tooltip_request.tooltip.view.clone().into_any_element();
            let mouse_position = tooltip_request.tooltip.mouse_position;
            let tooltip_size = element.layout_as_root(AvailableSpace::min_size(), self, cx);

            let mut tooltip_bounds =
                Bounds::new(mouse_position + point(px(1.), px(1.)), tooltip_size);
            let window_bounds = Bounds {
                origin: Point::default(),
                size: self.viewport_size(),
            };

            if tooltip_bounds.right() > window_bounds.right() {
                let new_x = mouse_position.x - tooltip_bounds.size.width - px(1.);
                if new_x >= Pixels::ZERO {
                    tooltip_bounds.origin.x = new_x;
                } else {
                    tooltip_bounds.origin.x = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.x - tooltip_bounds.right() - window_bounds.right(),
                    );
                }
            }

            if tooltip_bounds.bottom() > window_bounds.bottom() {
                let new_y = mouse_position.y - tooltip_bounds.size.height - px(1.);
                if new_y >= Pixels::ZERO {
                    tooltip_bounds.origin.y = new_y;
                } else {
                    tooltip_bounds.origin.y = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.y - tooltip_bounds.bottom() - window_bounds.bottom(),
                    );
                }
            }

            let is_visible =
                (tooltip_request.tooltip.check_visible_and_update)(tooltip_bounds, self, cx);
            if !is_visible {
                continue;
            }

            self.with_absolute_element_offset(tooltip_bounds.origin, |window| {
                element.prepaint(window, cx)
            });

            self.tooltip_bounds = Some(TooltipBounds {
                id: tooltip_request.id,
                bounds: tooltip_bounds,
            });
            return Some(element);
        }
        None
    }
}

#[cfg(test)]
mod fragment_count_damage_tests {
    use super::*;

    fn append_quad(scene: &mut crate::scene::Painter, x: f32, width: f32) -> RetainedSceneSegment {
        let bounds = Bounds::new(
            point(ScaledPixels(x), ScaledPixels(0.0)),
            size(ScaledPixels(width), ScaledPixels(10.0)),
        );
        let start = scene.len();
        scene.insert_primitive(Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            ..Quad::default()
        });
        RetainedSceneSegment {
            entity_id: EntityId::from(1_u64),
            bounds,
            scene_range: start..scene.len(),
            prepaint_range: Default::default(),
            paint_range: Default::default(),
        }
    }

    #[test]
    fn inserting_popup_fragment_preserves_large_static_background() {
        let mut previous = crate::scene::Painter::default();
        let old_background = append_quad(&mut previous, 0.0, 900.0);
        let old_control = append_quad(&mut previous, 700.0, 20.0);

        let mut current = crate::scene::Painter::default();
        let new_background = append_quad(&mut current, 0.0, 900.0);
        let popup = append_quad(&mut current, 150.0, 90.0);
        let new_control = append_quad(&mut current, 700.0, 20.0);

        let mut damage = Vec::new();
        for_each_changed_fragment_count_bounds(
            &previous,
            &[&old_background, &old_control],
            &current,
            &[&new_background, &popup, &new_control],
            |bounds| damage.push(bounds),
        );
        assert!(damage.contains(&popup.bounds));
        assert!(!damage.contains(&old_background.bounds));
        assert!(!damage.contains(&new_background.bounds));
    }

    #[test]
    fn removing_popup_fragment_preserves_large_static_background() {
        let mut previous = crate::scene::Painter::default();
        let old_background = append_quad(&mut previous, 0.0, 900.0);
        let popup = append_quad(&mut previous, 150.0, 90.0);
        let old_control = append_quad(&mut previous, 700.0, 20.0);

        let mut current = crate::scene::Painter::default();
        let new_background = append_quad(&mut current, 0.0, 900.0);
        let new_control = append_quad(&mut current, 700.0, 20.0);

        let mut damage = Vec::new();
        for_each_changed_fragment_count_bounds(
            &previous,
            &[&old_background, &popup, &old_control],
            &current,
            &[&new_background, &new_control],
            |bounds| damage.push(bounds),
        );
        assert!(damage.contains(&popup.bounds));
        assert!(!damage.contains(&old_background.bounds));
        assert!(!damage.contains(&new_background.bounds));
    }
}

#[cfg(test)]
mod degraded_recovery_damage_tests {
    use super::*;

    #[test]
    fn partitioned_retained_scene_recovery_can_damage_only_changed_operations() {
        let viewport = Bounds::new(
            crate::point(ScaledPixels(0.), ScaledPixels(0.)),
            crate::size(ScaledPixels(900.), ScaledPixels(600.)),
        );
        let build = |x| {
            let mut scene = crate::scene::Painter::default();
            // These are independent static visual fragments. No single
            // retained fragment need cover the entire painter stream.
            for (left, top) in [(30., 20.), (x, 160.)] {
                scene.insert_primitive(Quad {
                    bounds: Bounds::new(
                        crate::point(ScaledPixels(left), ScaledPixels(top)),
                        crate::size(ScaledPixels(40.), ScaledPixels(26.)),
                    ),
                    content_mask: crate::ContentMask::new(viewport),
                    ..Default::default()
                });
            }
            scene.finish();
            scene
        };
        let old = build(220.);
        let updated = build(246.);
        let mut dirty = DirtyRegion::empty();
        assert!(crate::paint::for_each_changed_bounds(
            &updated,
            0..updated.len(),
            &old,
            0..old.len(),
            |bounds| dirty.push(bounds),
        ));
        assert!(!dirty.is_full());
        assert!(dirty.rect_count() >= 1);
        assert!(dirty.area() < (900. * 600.) as f32);
    }
}
