use super::frame::RetainedPaintContext;
use super::state::ElementVisualTransform;
use super::*;


impl Window {
    pub(super) fn prepaint_deferred_draws(&mut self, cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);
        assert!(self.fast_layout.is_empty());
        assert!(self.paint_layout_key.is_none());

        // Deferred prepaint must stay in the same vector coordinate space captured by
        // PrepaintStateIndex. Processing in place also allows a deferred child to enqueue another
        // deferred draw without shifting the retained ranges recorded by its cached ancestors.
        let mut round_start = 0;
        let mut depth = 0;
        'rounds: loop {
            let round_end = self.next_frame.deferred_draws.len();
            if round_start == round_end {
                break;
            }
            assert!(depth < 10, "exceeded maximum (10) deferred draw depth");
            depth += 1;

            let mut traversal_order = (round_start..round_end).collect::<SmallVec<[usize; 8]>>();
            traversal_order.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);

            for deferred_draw_ix in traversal_order {
                if self.draw_budget_exhausted() {
                    self.degrade_current_draw();
                    break 'rounds;
                }

                // Do not hold a borrow into deferred_draws while prepainting: nested deferred
                // elements append to the same vector. Temporarily exchange the captured
                // context with the window; both passes can use it without cloning paths.
                let (
                    mut element,
                    parent_node,
                    current_view,
                    absolute_offset,
                    element_visual_transform,
                ) = {
                    let deferred_draw = &mut self.next_frame.deferred_draws[deferred_draw_ix];
                    self.element_id_stack.restore_prefix(deferred_draw.element_prefix.take());
                    mem::swap(&mut self.text_style_stack, &mut deferred_draw.text_style_stack);
                    mem::swap(&mut self.content_mask_stack, &mut deferred_draw.content_mask_stack);
                    mem::swap(
                        &mut self.visual_content_mask_stack,
                        &mut deferred_draw.visual_content_mask_stack,
                    );
                    (
                        deferred_draw.element.take(),
                        deferred_draw.parent_node,
                        deferred_draw.current_view,
                        deferred_draw.absolute_offset,
                        deferred_draw.element_visual_transform,
                    )
                };

                self.element_visual_transform = element_visual_transform;
                self.next_frame.dispatch_tree.set_active_node(parent_node);

                let prepaint_start = self.prepaint_index();
                if let Some(element) = element.as_mut() {
                    self.with_rendered_view(current_view, |window| {
                        window.with_absolute_element_offset(absolute_offset, |window| {
                            element.prepaint(window, cx)
                        });
                    });
                } else {
                    self.degrade_current_draw();
                }
                let prepaint_end = self.prepaint_index();

                let deferred_draw = &mut self.next_frame.deferred_draws[deferred_draw_ix];
                deferred_draw.element_prefix = self.element_id_stack.take_prefix();
                mem::swap(&mut self.text_style_stack, &mut deferred_draw.text_style_stack);
                mem::swap(&mut self.content_mask_stack, &mut deferred_draw.content_mask_stack);
                mem::swap(
                    &mut self.visual_content_mask_stack,
                    &mut deferred_draw.visual_content_mask_stack,
                );
                deferred_draw.element = element;
                deferred_draw.prepaint_range = prepaint_start..prepaint_end;

                if self.draw_was_degraded() {
                    break 'rounds;
                }
            }

            self.element_id_stack.clear();
            self.text_style_stack.clear();
            self.element_visual_transform = ElementVisualTransform::identity();
            self.content_mask_stack.clear();
            self.visual_content_mask_stack.clear();
            round_start = round_end;
        }

        self.element_id_stack.clear();
        self.text_style_stack.clear();
        self.element_visual_transform = ElementVisualTransform::identity();
        self.content_mask_stack.clear();
        self.visual_content_mask_stack.clear();
    }

    pub(super) fn deferred_draw_traversal_order(&self) -> SmallVec<[usize; 8]> {
        let mut sorted_indices =
            (0..self.next_frame.deferred_draws.len()).collect::<SmallVec<[_; 8]>>();
        sorted_indices.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);
        sorted_indices
    }

    pub(super) fn paint_deferred_draws(&mut self, deferred_draw_indices: &[usize], cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);
        assert!(self.fast_layout.is_empty());
        assert!(self.paint_layout_key.is_none());

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack.restore_prefix(deferred_draw.element_prefix.take());
            self.element_visual_transform = deferred_draw.element_visual_transform;
            mem::swap(&mut self.content_mask_stack, &mut deferred_draw.content_mask_stack);
            mem::swap(
                &mut self.visual_content_mask_stack,
                &mut deferred_draw.visual_content_mask_stack,
            );
            self.next_frame
                .dispatch_tree
                .set_active_node(deferred_draw.parent_node);

            let paint_start = self.paint_index();
            let current_view = deferred_draw.current_view;
            let painted = if let Some(element) = deferred_draw.element.as_mut() {
                self.with_rendered_view(current_view, |window| {
                    element.paint(window, cx);
                });
                true
            } else {
                self.degrade_current_draw();
                false
            };
            deferred_draw.element_prefix = self.element_id_stack.take_prefix();
            mem::swap(&mut self.content_mask_stack, &mut deferred_draw.content_mask_stack);
            mem::swap(
                &mut self.visual_content_mask_stack,
                &mut deferred_draw.visual_content_mask_stack,
            );
            if !painted {
                break;
            }
            let paint_end = self.paint_index();
            deferred_draw.paint_range = paint_start..paint_end;
        }
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
        self.element_visual_transform = ElementVisualTransform::identity();
        self.content_mask_stack.clear();
        self.visual_content_mask_stack.clear();
    }

    pub(crate) fn prepaint_index(&self) -> PrepaintStateIndex {
        PrepaintStateIndex {
            hitboxes_index: self.next_frame.hitboxes.len(),
            tooltips_index: self.next_frame.tooltip_requests.len(),
            deferred_draws_index: self.next_frame.deferred_draws.len(),
            dispatch_tree_index: self.next_frame.dispatch_tree.len(),
            accessed_element_states_index: self.next_frame.accessed_element_states.len(),
            line_layout_index: self.text_system.layout_index(),
        }
    }

    pub(crate) fn truncate_prepaint_to(&mut self, index: PrepaintStateIndex) {
        self.next_frame.hitboxes.truncate(index.hitboxes_index);
        self.next_frame
            .tooltip_requests
            .truncate(index.tooltips_index);
        self.next_frame
            .deferred_draws
            .truncate(index.deferred_draws_index);
        self.next_frame
            .dispatch_tree
            .truncate(index.dispatch_tree_index);
        self.next_frame.focus = self.focus.filter(|focus| {
            self.next_frame.dispatch_tree.focusable_node_id(*focus).is_some()
        });
        self.next_frame
            .accessed_element_states
            .truncate(index.accessed_element_states_index);
        self.text_system.truncate_layouts(index.line_layout_index);
    }

    pub(crate) fn can_reuse_prepaint(&self, range: &Range<PrepaintStateIndex>) -> bool {
        let _work = super::element_work::begin_named(
            "validate_prepaint_range",
            super::element_work::ElementWorkPhase::Replay,
        );
        self.prepaint_range_indices_are_valid(range)
            && range.start.deferred_draws_index == range.end.deferred_draws_index
            && range.start.tooltips_index == range.end.tooltips_index
    }

    fn prepaint_range_indices_are_valid(&self, range: &Range<PrepaintStateIndex>) -> bool {
        frame_range_is_valid(
            range.start.hitboxes_index,
            range.end.hitboxes_index,
            self.rendered_frame.hitboxes.len(),
        ) && frame_range_is_valid(
            range.start.tooltips_index,
            range.end.tooltips_index,
            self.rendered_frame.tooltip_requests.len(),
        ) && frame_range_is_valid(
            range.start.deferred_draws_index,
            range.end.deferred_draws_index,
            self.rendered_frame.deferred_draws.len(),
        ) && frame_range_is_valid(
            range.start.dispatch_tree_index,
            range.end.dispatch_tree_index,
            self.rendered_frame.dispatch_tree.len(),
        ) && frame_range_is_valid(
            range.start.accessed_element_states_index,
            range.end.accessed_element_states_index,
            self.rendered_frame.accessed_element_states.len(),
        ) && self.text_system.can_reuse_layouts(
            range.start.line_layout_index.clone()..range.end.line_layout_index.clone(),
        )
    }

    #[cfg(test)]
    pub(crate) fn reuse_prepaint(&mut self, range: Range<PrepaintStateIndex>) -> bool {
        if !self.can_reuse_prepaint(&range) {
            return false;
        }
        self.replay_validated_prepaint(range);
        true
    }

    /// The view admission already validated both phase ranges.
    pub(crate) fn replay_validated_prepaint(&mut self, range: Range<PrepaintStateIndex>) {
        let parent = self.next_frame.dispatch_tree.active_node_id();
        self.replay_prepaint_segment(range, |_| parent);
    }

    /// Replay a validated phase segment; external parents may be in earlier segments.
    pub(super) fn replay_prepaint_segment(
        &mut self,
        range: Range<PrepaintStateIndex>,
        external_parent: impl FnMut(Option<DispatchNodeId>) -> Option<DispatchNodeId>,
    ) {
        let _work = super::element_work::begin_named(
            "reuse_prepaint_range",
            super::element_work::ElementWorkPhase::Replay,
        );

        self.next_frame.hitboxes.extend_from_slice(
            &self.rendered_frame.hitboxes[range.start.hitboxes_index..range.end.hitboxes_index],
        );
        self.replay_prepaint_records(range, external_parent);
    }

    /// Carry non-geometric prepaint records independently of hitbox placement.
    /// A caller replacing hitbox geometry must still preserve dispatch, focus,
    /// element-state ownership and text layout lifetimes.
    pub(super) fn replay_prepaint_records(
        &mut self,
        range: Range<PrepaintStateIndex>,
        external_parent: impl FnMut(Option<DispatchNodeId>) -> Option<DispatchNodeId>,
    ) {
        // Tooltip ranges are excluded at admission; there are no owners to move.
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .cloned(),
        );
        self.text_system
            .reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);

        let reused_subtree = self.next_frame.dispatch_tree.replay_segment(
            range.start.dispatch_tree_index..range.end.dispatch_tree_index,
            &self.rendered_frame.dispatch_tree,
            self.focus,
            external_parent,
        );

        if reused_subtree.contains_focus() {
            self.next_frame.focus = self.focus;
        }

    }

    pub(crate) fn paint_index(&self) -> PaintIndex {
        PaintIndex {
            scene_index: self.next_frame.painter.len(),
            #[cfg(any(test, feature = "test-support"))]
            debug_bounds_index: self.next_frame.debug_bounds_records.len(),
            mouse_listeners_index: self.next_frame.mouse_listeners.len(),
            input_handlers_index: self.next_frame.input_handlers.len(),
            cursor_styles_index: self.next_frame.cursor_styles.len(),
            window_control_hitboxes_index: self.next_frame.window_control_hitboxes.len(),
            accessed_element_states_index: self.next_frame.accessed_element_states.len(),
            tab_handle_index: self.next_frame.tab_stops.paint_index(),
            line_layout_index: self.text_system.layout_index(),
        }
    }

    pub(crate) fn can_reuse_paint(&self, range: &Range<PaintIndex>) -> bool {
        let _work = super::element_work::begin_named(
            "validate_paint_range",
            super::element_work::ElementWorkPhase::Replay,
        );
        let scene_valid = frame_range_is_valid(
            range.start.scene_index,
            range.end.scene_index,
            self.rendered_frame.scene().len(),
        );
        let replayable = self
            .rendered_frame
            .range_is_replayable(range.start.scene_index..range.end.scene_index);
        let listeners_valid = frame_range_is_valid(
            range.start.mouse_listeners_index,
            range.end.mouse_listeners_index,
            self.rendered_frame.mouse_listeners.len(),
        );
        let input_valid = frame_range_is_valid(
            range.start.input_handlers_index,
            range.end.input_handlers_index,
            self.rendered_frame.input_handlers.len(),
        );
        let cursor_valid = frame_range_is_valid(
            range.start.cursor_styles_index,
            range.end.cursor_styles_index,
            self.rendered_frame.cursor_styles.len(),
        );
        let controls_valid = frame_range_is_valid(
            range.start.window_control_hitboxes_index,
            range.end.window_control_hitboxes_index,
            self.rendered_frame.window_control_hitboxes.len(),
        );
        let states_valid = frame_range_is_valid(
            range.start.accessed_element_states_index,
            range.end.accessed_element_states_index,
            self.rendered_frame.accessed_element_states.len(),
        );
        let tabs_valid = frame_range_is_valid(
            range.start.tab_handle_index,
            range.end.tab_handle_index,
            self.rendered_frame.tab_stops.insertion_history.len(),
        );
        let text_valid = self.text_system.can_reuse_layouts(
            range.start.line_layout_index.clone()..range.end.line_layout_index.clone(),
        );
        let valid = scene_valid
            && replayable
            && listeners_valid
            && input_valid
            && cursor_valid
            && controls_valid
            && states_valid
            && tabs_valid
            && text_valid;
        if !valid && log::log_enabled!(target: "gpui::diagnostics", log::Level::Trace) {
            log::trace!(
                target: "gpui::diagnostics",
                "gpui paint range invalid: scene={} replayable={} listeners={} input={} cursor={} controls={} states={} tabs={} text={}",
                scene_valid,
                replayable,
                listeners_valid,
                input_valid,
                cursor_valid,
                controls_valid,
                states_valid,
                tabs_valid,
                text_valid,
            );
        }
        valid && {
            #[cfg(any(test, feature = "test-support"))]
            {
                frame_range_is_valid(
                    range.start.debug_bounds_index,
                    range.end.debug_bounds_index,
                    self.rendered_frame.debug_bounds_records.len(),
                )
            }
            #[cfg(not(any(test, feature = "test-support")))]
            {
                true
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn reuse_paint(&mut self, range: Range<PaintIndex>) -> bool {
        let _work = super::element_work::begin_named(
            "reuse_paint_range",
            super::element_work::ElementWorkPhase::Replay,
        );
        if !self.can_reuse_paint(&range) {
            log::debug!(
                "gpui retained paint range invalid: window={}",
                self.handle.window_id().as_u64()
            );
            return false;
        }
        self.replay_validated_paint(range);
        true
    }

    /// Copies a paint range that was validated earlier in this frame.
    ///
    /// A whole-view replay validates both of its ranges during prepaint; the committed frame does
    /// not change afterwards, so paint must not pay for the validation again (and a required text
    /// layout was already pinned by the prepaint copy).
    pub(crate) fn replay_validated_paint(&mut self, range: Range<PaintIndex>) {
        self.replay_validated_paint_records(&range);
        self.replay_validated_scene(range.start.scene_index..range.end.scene_index);
    }

    /// Carry the input and state records without replaying any scene operations.
    /// Retained compositor content must not replay its primitives into the root
    /// painter merely to keep listeners, focus/IME and layout resources alive.
    /// The caller must validate the range before using this method.
    pub(super) fn replay_validated_paint_records(&mut self, range: &Range<PaintIndex>) {
        #[cfg(any(test, feature = "test-support"))]
        {
            for (selector, bounds) in &self.rendered_frame.debug_bounds_records
                [range.start.debug_bounds_index..range.end.debug_bounds_index]
            {
                self.next_frame.record_debug_bounds(selector.clone(), *bounds);
            }
        }

        self.next_frame.cursor_styles.extend(
            self.rendered_frame.cursor_styles
                [range.start.cursor_styles_index..range.end.cursor_styles_index]
                .iter()
                .cloned(),
        );
        self.next_frame.window_control_hitboxes.extend(
            self.rendered_frame.window_control_hitboxes[range.start.window_control_hitboxes_index
                ..range.end.window_control_hitboxes_index]
                .iter()
                .cloned(),
        );
        let input_target_start = self.next_frame.input_handlers.len();
        for (offset, handler) in self.rendered_frame.input_handlers
            [range.start.input_handlers_index..range.end.input_handlers_index]
            .iter_mut()
            .enumerate()
        {
            if handler.is_some() {
                self.next_frame.replayed_input_handlers.push((
                    range.start.input_handlers_index + offset,
                    input_target_start + offset,
                ));
            }
            self.next_frame.input_handlers.push(handler.take());
        }
        super::scroll_dependencies::carry_scroll_sources(
            &self.rendered_frame.mounted_scroll_sources,
            &mut self.next_frame.mounted_scroll_sources,
            range.start.mouse_listeners_index..range.end.mouse_listeners_index,
            self.next_frame.mouse_listeners.len(),
        );
        self.next_frame.mouse_listeners.extend(
            self.rendered_frame.mouse_listeners
                [range.start.mouse_listeners_index..range.end.mouse_listeners_index]
                .iter()
                .cloned(),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .cloned(),
        );
        self.next_frame.tab_stops.replay(
            &self.rendered_frame.tab_stops.insertion_history
                [range.start.tab_handle_index..range.end.tab_handle_index],
        );

        self.text_system
            .reuse_layouts(range.start.line_layout_index.clone()..range.end.line_layout_index);
    }

    /// Replay only the visual fragment; the independent record carry is
    /// responsible for input, element states, tab stops and text layouts.
    pub(super) fn replay_validated_scene(&mut self, old_scene_range: Range<usize>) {
        let replay_start = self.next_frame.painter.len();
        let spans = self
            .rendered_frame
            .retained_paint
            .replay_quads(self.rendered_frame.scene(), old_scene_range.clone());
        if self.next_frame.painter.replay_with_ordering(
            old_scene_range.clone(),
            self.rendered_frame.scene(),
            spans,
            Some(&self.rendered_frame.painter),
        ) {
            Arc::get_mut(&mut self.next_frame.retained_paint)
                .expect("unpublished paint retention")
                .replay(
                    old_scene_range,
                    &self.rendered_frame.retained_paint,
                    replay_start,
                );
        }
    }

    pub(crate) fn current_retained_paint_context(&self) -> RetainedPaintContext {
        RetainedPaintContext {
            device_scale_factor: self.scale_factor(),
            opacity: self.element_opacity,
            scale: self.element_visual_transform.scale,
            translation: self.element_visual_transform.translation,
            element_offset: self.element_offset_stack.last().copied().unwrap_or_default(),
            text_raster_scale: self.scene_text_raster_scale,
            content_mask: self.content_mask(),
            visual_content_mask: self.visual_content_mask(),
            text_style: self.text_style(),
            rem_size: self.rem_size(),
        }
    }

    /// Compare the inherited context without constructing a temporary TextStyle or context.
    pub(super) fn retained_paint_context_matches(&self, previous: &RetainedPaintContext) -> bool {
        previous.device_scale_factor == self.scale_factor()
            && previous.opacity == self.element_opacity
            && previous.scale == self.element_visual_transform.scale
            && previous.translation == self.element_visual_transform.translation
            && previous.element_offset == self.element_offset_stack.last().copied().unwrap_or_default()
            && previous.text_raster_scale == self.scene_text_raster_scale
            && previous.content_mask == self.content_mask()
            && previous.visual_content_mask == self.visual_content_mask()
            && previous.rem_size == self.rem_size()
            && self.with_text_style_ref(|style| &previous.text_style == style)
    }

}

pub(crate) fn rebase_prepaint_range(
    range: &Range<PrepaintStateIndex>,
    source: &Range<PrepaintStateIndex>,
    target: &Range<PrepaintStateIndex>,
) -> Option<Range<PrepaintStateIndex>> {
    Some(
        range.start.rebased_from(&source.start, &target.start)?
            ..range.end.rebased_from(&source.start, &target.start)?,
    )
}

pub(crate) fn rebase_paint_range(
    range: &Range<PaintIndex>,
    source: &Range<PaintIndex>,
    target: &Range<PaintIndex>,
) -> Option<Range<PaintIndex>> {
    Some(
        range.start.rebased_from(&source.start, &target.start)?
            ..range.end.rebased_from(&source.start, &target.start)?,
    )
}

fn frame_range_is_valid(start: usize, end: usize, len: usize) -> bool {
    start <= end && end <= len
}
