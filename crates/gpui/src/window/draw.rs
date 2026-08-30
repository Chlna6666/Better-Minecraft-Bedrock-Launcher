use super::*;

struct RetainedEntitySegments<'a> {
    bounds: Bounds<ScaledPixels>,
    segments: SmallVec<[&'a RetainedSceneSegment; 1]>,
}

impl<'a> RetainedEntitySegments<'a> {
    fn new(segment: &'a RetainedSceneSegment) -> Self {
        Self {
            bounds: segment.bounds,
            segments: SmallVec::from_elem(segment, 1),
        }
    }

    fn push(&mut self, segment: &'a RetainedSceneSegment) {
        self.bounds = self.bounds.union(&segment.bounds);
        self.segments.push(segment);
    }
}

impl Window {
    /// Produces a new frame and assigns it to `rendered_frame`. To actually show
    /// the contents of the new [`Scene`], use [`Self::present`].
    #[profiling::function]
    pub fn draw(&mut self, cx: &mut App) -> ArenaClearNeeded {
        let previous_scene_was_empty = self.rendered_frame.scene.len() == 0;
        let debug_force_full_redraw = self.begin_debug_visualization_frame(cx);
        let force_full_redraw = self.force_full_redraw.get() || debug_force_full_redraw;
        let (restored_input_handler_index, directly_dirty_views) = self.begin_draw_cycle(cx);
        self.draw_roots(cx);
        self.next_frame.window_active = self.active.get();

        if self.draw_was_degraded && self.has_completed_rendered_frame {
            return self.finish_degraded_draw(restored_input_handler_index);
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

        self.finish_completed_draw(
            previous_scene_was_empty,
            force_full_redraw,
            &directly_dirty_views,
            cx,
        )
    }

    fn begin_draw_cycle(&mut self, cx: &mut App) -> (Option<usize>, SmallVec<[EntityId; 8]>) {
        let frame_budget = DIRTY_FRAME_BACKPRESSURE_BUDGET;
        self.dirty_frame_scheduled = false;
        self.draw_deadline = Some(Instant::now() + frame_budget);
        self.draw_was_degraded = false;
        record_window_layout_recompute(self.handle.window_id().as_u64());
        let directly_dirty_views = self.invalidate_entities();
        self.pending_list_measured_items = 0;
        cx.entities.clear_accessed();
        debug_assert!(self.rendered_entity_stack.is_empty());
        debug_assert!(self.view_bounds_stack.is_empty());
        // Defensive: an unwound draw could leave stale frames behind.
        self.view_bounds_stack.clear();
        self.invalidator.set_dirty(false);
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
        self.restore_input_handler_after_degraded_draw(restored_input_handler_index);
        self.finish_layout_and_text_frame();
        let frame_retained_capacity = self.next_frame.retained_capacity();
        let scene_metrics = self.next_frame.scene.frame_metrics();
        self.last_generation_stats.scene = scene_metrics;
        self.last_generation_stats.frame_retained_capacity = frame_retained_capacity;
        self.last_generation_stats.list_measured_items = self.pending_list_measured_items;
        record_frame_retained_capacity(frame_retained_capacity);
        record_scene_frame_metrics(scene_metrics);
        self.rendered_frame
            .element_states
            .extend(self.next_frame.element_states.drain());
        self.next_frame.clear();
        self.invalidator.set_dirty(true);
        self.refreshing = false;
        self.invalidator.set_phase(DrawPhase::None);
        self.force_full_redraw.set(true);
        self.force_view_cache_refresh = true;
        self.recovering_degraded_draw = true;
        self.draw_deadline = None;
        ArenaClearNeeded
    }

    fn finish_completed_draw(
        &mut self,
        previous_scene_was_empty: bool,
        force_full_redraw: bool,
        directly_dirty_views: &[EntityId],
        cx: &mut App,
    ) -> ArenaClearNeeded {
        self.finish_layout_and_text_frame();
        self.next_frame.finish(&mut self.rendered_frame);
        let scene_animation_values = self
            .animation_engine
            .borrow()
            .scene_values(self.animation_time());
        self.next_frame
            .scene
            .replace_engine_animation_values(scene_animation_values);
        self.backdrop_blur_damage_plan = self
            .next_frame
            .scene
            .backdrop_blur_damage_plan(&self.rendered_frame.scene);
        self.prepare_render_plan_for_next_frame(
            previous_scene_was_empty || force_full_redraw || self.draw_was_degraded,
            directly_dirty_views,
        );
        let frame_retained_capacity = self.next_frame.retained_capacity();
        let scene_metrics = self.next_frame.scene.frame_metrics();
        self.last_generation_stats.scene = scene_metrics;
        self.last_generation_stats.frame_retained_capacity = frame_retained_capacity;
        self.last_generation_stats.list_measured_items = self.pending_list_measured_items;
        record_frame_retained_capacity(frame_retained_capacity);
        record_scene_frame_metrics(scene_metrics);

        self.invalidator.set_phase(DrawPhase::Focus);
        let previous_focus_path = self.rendered_frame.focus_path();
        let previous_window_active = self.rendered_frame.window_active;
        mem::swap(&mut self.rendered_frame, &mut self.next_frame);
        self.next_frame.clear();
        let live_scene_animation_ids = self.rendered_frame.scene.animation_ids();
        self.animation_engine
            .borrow_mut()
            .retain_scene_animations(&live_scene_animation_ids);
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
        self.has_completed_rendered_frame = true;
        // A layout RAF may notify and rebuild a view after its sampled value has already
        // converged. Retain the newly built CPU scene, but do not encode, upload, or present an
        // identical framebuffer. Platform exposure and explicit presentation requests still use
        // the presentation-only path without coming through this flag.
        self.needs_present.set(!self.render_dirty_region.is_empty());
        if self.draw_was_degraded {
            self.invalidator.set_dirty(true);
        } else {
            self.dirty_views.clear();
            self.animation_dirty_region = DirtyRegion::empty();
        }
        self.finish_debug_visualization_frame(cx);
        self.draw_deadline = None;

        ArenaClearNeeded
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
    ) {
        let viewport = Bounds::new(Point::default(), self.viewport_size).scale(self.scale_factor);
        let mut dirty_region = DirtyRegion::empty();

        let scene_requires_full_redraw = self.next_frame.scene.requires_full_redraw_fallback();
        let requires_full_redraw = force_full_redraw || scene_requires_full_redraw;

        if requires_full_redraw {
            if scene_requires_full_redraw {
                crate::diagnostics::performance_metrics::record_full_redraw_fallback();
            }
            dirty_region.mark_full(viewport);
        } else {
            // `dirty_views` deliberately contains the full ancestor path so AnyView can invalidate
            // retained prepaint/paint caches. That is a traversal/cache semantic, not a pixel-damage
            // semantic: treating every ancestor RetainedSceneSegment as changed makes a dirty root
            // segment turn a local child update into a full-window redraw. Compute damage from the
            // directly invalidated views instead and separately account for layout-induced moves.
            self.add_direct_view_damage(&mut dirty_region, directly_dirty_views);
            for rect in self.animation_dirty_region.rects() {
                dirty_region.push(rect.bounds);
            }

            for bounds in self
                .next_frame
                .scene
                .backdrop_blur_output_damage(&self.backdrop_blur_damage_plan)
            {
                dirty_region.push(bounds);
            }

            if dirty_region.is_empty() && self.next_frame.retained_scene_segments.is_empty() {
                dirty_region.mark_full(viewport);
            } else if !dirty_region.is_empty() {
                dirty_region.coalesce_if_large(viewport, DIRTY_REGION_FULL_REDRAW_RATIO);
            }
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

        let mut previous_entities: FxHashMap<EntityId, RetainedEntitySegments<'_>> =
            FxHashMap::default();
        let mut current_entities: FxHashMap<EntityId, RetainedEntitySegments<'_>> =
            FxHashMap::default();

        for segment in &self.rendered_frame.retained_scene_segments {
            previous_entities
                .entry(segment.entity_id)
                .and_modify(|entity| entity.push(segment))
                .or_insert_with(|| RetainedEntitySegments::new(segment));
        }
        for segment in &self.next_frame.retained_scene_segments {
            current_entities
                .entry(segment.entity_id)
                .and_modify(|entity| entity.push(segment))
                .or_insert_with(|| RetainedEntitySegments::new(segment));
        }

        // Diff the directly invalidated view's retained scene operations. A layout-driven spring
        // often belongs to a full-window page view even though only a button, pill, or menu changes;
        // using the whole view segment would promote that animation to full-window presentation.
        for entity_id in directly_dirty_views {
            let previous = previous_entities.get(entity_id);
            let current = current_entities.get(entity_id);
            let diffed = previous.zip(current).is_some_and(|(previous, current)| {
                if previous.segments.len() != 1 || current.segments.len() != 1 {
                    return false;
                }
                self.next_frame.scene.for_each_changed_bounds(
                    current.segments[0].scene_range.clone(),
                    &self.rendered_frame.scene,
                    previous.segments[0].scene_range.clone(),
                    |bounds| dirty_region.push(bounds),
                )
            });
            if !diffed {
                if let Some(previous) = previous {
                    dirty_region.push(previous.bounds);
                }
                if let Some(current) = current {
                    dirty_region.push(current.bounds);
                }
            }
        }

        // Layout changes can move siblings even though those siblings were not directly notified.
        // Compare retained bounds across frames and damage only entities whose visual extent moved,
        // appeared, or disappeared. Stable ancestors such as MainWindow therefore remain clean.
        for (entity_id, current) in &current_entities {
            match previous_entities.get(entity_id) {
                Some(previous)
                    if previous.segments.len() == current.segments.len()
                        && previous
                            .segments
                            .iter()
                            .zip(&current.segments)
                            .all(|(previous, current)| previous.bounds == current.bounds) => {}
                Some(previous) => {
                    for segment in &previous.segments {
                        dirty_region.push(segment.bounds);
                    }
                    for segment in &current.segments {
                        dirty_region.push(segment.bounds);
                    }
                }
                None => {
                    for segment in &current.segments {
                        dirty_region.push(segment.bounds);
                    }
                }
            }
        }
        for (entity_id, previous) in &previous_entities {
            if !current_entities.contains_key(entity_id) {
                for segment in &previous.segments {
                    dirty_region.push(segment.bounds);
                }
            }
        }
    }

    fn render_plan(&self) -> FrameRenderPlan<'_> {
        FrameRenderPlan {
            scene: &self.rendered_frame.scene,
            dirty_region: &self.render_dirty_region,
            backdrop_blur_damage_plan: &self.backdrop_blur_damage_plan,
            partial_present_mode: self.render_present_mode,
            trim_policy: self.render_trim_policy,
            force_full_backdrop_blur_refresh: false,
        }
    }

    fn record_entities_accessed(&mut self, cx: &mut App) {
        let mut entities_ref = cx.entities.accessed_entities.borrow_mut();
        let mut entities = mem::take(entities_ref.deref_mut());
        drop(entities_ref);
        let handle = self.handle;
        cx.record_entities_accessed(
            handle,
            // Try moving window invalidator into the Window
            self.invalidator.clone(),
            &entities,
        );
        let mut entities_ref = cx.entities.accessed_entities.borrow_mut();
        mem::swap(&mut entities, entities_ref.deref_mut());
    }

    fn invalidate_entities(&mut self) -> SmallVec<[EntityId; 8]> {
        let mut views = self.invalidator.take_views();
        let mut directly_dirty_views = SmallVec::new();
        directly_dirty_views.reserve(views.len());
        for entity in views.drain() {
            directly_dirty_views.push(entity);
            self.mark_view_dirty(entity);
        }
        self.invalidator.replace_views(views);
        directly_dirty_views
    }

    #[profiling::function]
    pub(super) fn present(&self) {
        self.platform_window.draw(self.render_plan());
        self.needs_present.set(false);
        profiling::finish_frame!();
    }

    pub(super) fn present_framebuffer_only(&self) {
        self.platform_window
            .present_framebuffer_only(self.render_plan());
        self.needs_present.set(false);
        profiling::finish_frame!();
    }

    fn draw_roots(&mut self, cx: &mut App) {
        self.invalidator.set_phase(DrawPhase::Prepaint);
        self.tooltip_bounds.take();

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

        let mut root_element = self.root.as_ref().unwrap().clone().into_any();
        self.with_critical_draw(|window| {
            root_element.prepaint_as_root(Point::default(), root_size.into(), window, cx);
        });

        #[cfg(any(feature = "inspector", debug_assertions))]
        let inspector_element = if self.inspector.is_some() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
            None
        } else {
            self.prepaint_inspector(_inspector_width, cx)
        };

        let mut sorted_deferred_draws =
            (0..self.next_frame.deferred_draws.len()).collect::<SmallVec<[_; 8]>>();
        sorted_deferred_draws.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);
        if !sorted_deferred_draws.is_empty() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
            sorted_deferred_draws.clear();
        } else {
            self.prepaint_deferred_draws(&sorted_deferred_draws, cx);
        }

        let mut prompt_element = None;
        let mut active_drag_element = None;
        let mut tooltip_element = None;
        let has_overlay_work = self.prompt.is_some()
            || cx.active_drag.is_some()
            || !self.next_frame.tooltip_requests.is_empty();
        if has_overlay_work && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else if let Some(prompt) = self.prompt.take() {
            let mut element = prompt.view.any_view().into_any();
            element.prepaint_as_root(Point::default(), root_size.into(), self, cx);
            prompt_element = Some(element);
            self.prompt = Some(prompt);
        } else if let Some(active_drag) = cx.active_drag.take() {
            let mut element = active_drag.view.clone().into_any();
            let offset = self.mouse_position() - active_drag.cursor_offset;
            element.prepaint_as_root(offset, AvailableSpace::min_size(), self, cx);
            active_drag_element = Some(element);
            cx.active_drag = Some(active_drag);
        } else {
            tooltip_element = self.prepaint_tooltip(cx);
        }

        self.mouse_hit_test = self.next_frame.hit_test(self.mouse_position);

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
            let mut element = tooltip_request.tooltip.view.clone().into_any();
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
