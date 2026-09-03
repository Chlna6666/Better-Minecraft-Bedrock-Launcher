use super::frame::{ReconcileKey, RetainedElementRange};
use super::state::ElementVisualTransform;
use super::*;

impl Window {
    pub(super) fn prepaint_deferred_draws(
        &mut self,
        deferred_draw_indices: &[usize],
        cx: &mut App,
    ) {
        assert_eq!(self.element_id_stack.len(), 0);
        assert_eq!(self.retained_element_id_stack.len(), 0);

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
            self.retained_element_id_stack
                .clone_from(&deferred_draw.retained_element_id_stack);
            self.text_style_stack
                .clone_from(&deferred_draw.text_style_stack);
            self.element_visual_transform = deferred_draw.element_visual_transform;
            self.content_mask_stack
                .clone_from(&deferred_draw.content_mask_stack);
            self.visual_content_mask_stack
                .clone_from(&deferred_draw.visual_content_mask_stack);
            self.next_frame
                .dispatch_tree
                .set_active_node(deferred_draw.parent_node);

            let prepaint_start = self.prepaint_index();
            if let Some(element) = deferred_draw.element.as_mut() {
                self.with_rendered_view(deferred_draw.current_view, |window| {
                    window.with_absolute_element_offset(deferred_draw.absolute_offset, |window| {
                        element.prepaint(window, cx)
                    });
                })
            } else if !self.reuse_prepaint(deferred_draw.prepaint_range.clone()) {
                self.degrade_current_draw();
                break;
            }
            let prepaint_end = self.prepaint_index();
            deferred_draw.prepaint_range = prepaint_start..prepaint_end;
        }
        assert_eq!(
            self.next_frame.deferred_draws.len(),
            0,
            "cannot call defer_draw during deferred drawing"
        );
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
        self.retained_element_id_stack.clear();
        self.text_style_stack.clear();
        self.element_visual_transform = ElementVisualTransform::identity();
        self.content_mask_stack.clear();
        self.visual_content_mask_stack.clear();
    }

    pub(super) fn paint_deferred_draws(&mut self, deferred_draw_indices: &[usize], cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);
        assert_eq!(self.retained_element_id_stack.len(), 0);

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
            self.retained_element_id_stack
                .clone_from(&deferred_draw.retained_element_id_stack);
            self.element_visual_transform = deferred_draw.element_visual_transform;
            self.content_mask_stack
                .clone_from(&deferred_draw.content_mask_stack);
            self.visual_content_mask_stack
                .clone_from(&deferred_draw.visual_content_mask_stack);
            self.next_frame
                .dispatch_tree
                .set_active_node(deferred_draw.parent_node);

            let paint_start = self.paint_index();
            let current_view = deferred_draw.current_view;
            if let Some(element) = deferred_draw.element.as_mut() {
                self.with_rendered_view(current_view, |window| {
                    element.paint(window, cx);
                })
            } else {
                let paint_range = deferred_draw.paint_range.clone();
                let reused =
                    self.with_rendered_view(current_view, |window| window.reuse_paint(paint_range));
                if !reused {
                    self.degrade_current_draw();
                    break;
                }
            }
            let paint_end = self.paint_index();
            deferred_draw.paint_range = paint_start..paint_end;
        }
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
        self.retained_element_id_stack.clear();
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
        self.next_frame
            .accessed_element_states
            .truncate(index.accessed_element_states_index);
        self.text_system.truncate_layouts(index.line_layout_index);
    }

    pub(crate) fn can_reuse_prepaint(&self, range: &Range<PrepaintStateIndex>) -> bool {
        self.prepaint_range_indices_are_valid(range)
            && self.deferred_draw_ranges_are_reusable(
                range.start.deferred_draws_index..range.end.deferred_draws_index,
            )
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

    fn deferred_draw_ranges_are_reusable(&self, range: Range<usize>) -> bool {
        if !frame_range_is_valid(
            range.start,
            range.end,
            self.rendered_frame.deferred_draws.len(),
        ) {
            return false;
        }

        self.rendered_frame.deferred_draws[range]
            .iter()
            .all(|draw| {
                self.prepaint_range_indices_are_valid(&draw.prepaint_range)
                    && self.can_reuse_paint(&draw.paint_range)
            })
    }

    pub(crate) fn reuse_prepaint(&mut self, range: Range<PrepaintStateIndex>) -> bool {
        if !self.can_reuse_prepaint(&range) {
            log::debug!(
                "gpui retained prepaint range invalid: window={}",
                self.handle.window_id().as_u64()
            );
            return false;
        }

        self.next_frame.hitboxes.extend(
            self.rendered_frame.hitboxes[range.start.hitboxes_index..range.end.hitboxes_index]
                .iter()
                .cloned(),
        );
        self.next_frame.tooltip_requests.extend(
            self.rendered_frame.tooltip_requests
                [range.start.tooltips_index..range.end.tooltips_index]
                .iter_mut()
                .map(|request| request.take()),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (GlobalElementId(id.0.clone()), *type_id)),
        );
        self.text_system
            .reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);

        let reused_subtree = self.next_frame.dispatch_tree.reuse_subtree(
            range.start.dispatch_tree_index..range.end.dispatch_tree_index,
            &mut self.rendered_frame.dispatch_tree,
            self.focus,
        );

        if reused_subtree.contains_focus() {
            self.next_frame.focus = self.focus;
        }

        self.next_frame.deferred_draws.extend(
            self.rendered_frame.deferred_draws
                [range.start.deferred_draws_index..range.end.deferred_draws_index]
                .iter()
                .map(|deferred_draw| DeferredDraw {
                    current_view: deferred_draw.current_view,
                    parent_node: reused_subtree.refresh_node_id(deferred_draw.parent_node),
                    element_id_stack: deferred_draw.element_id_stack.clone(),
                    retained_element_id_stack: deferred_draw.retained_element_id_stack.clone(),
                    text_style_stack: deferred_draw.text_style_stack.clone(),
                    element_visual_transform: deferred_draw.element_visual_transform,
                    content_mask_stack: deferred_draw.content_mask_stack.clone(),
                    visual_content_mask_stack: deferred_draw.visual_content_mask_stack.clone(),
                    priority: deferred_draw.priority,
                    element: None,
                    absolute_offset: deferred_draw.absolute_offset,
                    prepaint_range: deferred_draw.prepaint_range.clone(),
                    paint_range: deferred_draw.paint_range.clone(),
                }),
        );
        true
    }

    pub(crate) fn paint_index(&self) -> PaintIndex {
        PaintIndex {
            scene_index: self.next_frame.scene.len(),
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
        frame_range_is_valid(
            range.start.scene_index,
            range.end.scene_index,
            self.rendered_frame.scene.len(),
        ) && frame_range_is_valid(
            range.start.mouse_listeners_index,
            range.end.mouse_listeners_index,
            self.rendered_frame.mouse_listeners.len(),
        ) && frame_range_is_valid(
            range.start.input_handlers_index,
            range.end.input_handlers_index,
            self.rendered_frame.input_handlers.len(),
        ) && frame_range_is_valid(
            range.start.cursor_styles_index,
            range.end.cursor_styles_index,
            self.rendered_frame.cursor_styles.len(),
        ) && frame_range_is_valid(
            range.start.window_control_hitboxes_index,
            range.end.window_control_hitboxes_index,
            self.rendered_frame.window_control_hitboxes.len(),
        ) && frame_range_is_valid(
            range.start.accessed_element_states_index,
            range.end.accessed_element_states_index,
            self.rendered_frame.accessed_element_states.len(),
        ) && frame_range_is_valid(
            range.start.tab_handle_index,
            range.end.tab_handle_index,
            self.rendered_frame.tab_stops.insertion_history.len(),
        ) && self.text_system.can_reuse_layouts(
            range.start.line_layout_index.clone()..range.end.line_layout_index.clone(),
        )
    }

    pub(crate) fn reuse_paint(&mut self, range: Range<PaintIndex>) -> bool {
        if !self.can_reuse_paint(&range) {
            log::debug!(
                "gpui retained paint range invalid: window={}",
                self.handle.window_id().as_u64()
            );
            return false;
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
        self.next_frame.input_handlers.extend(
            self.rendered_frame.input_handlers
                [range.start.input_handlers_index..range.end.input_handlers_index]
                .iter_mut()
                .map(|handler| handler.take()),
        );
        self.next_frame.mouse_listeners.extend(
            self.rendered_frame.mouse_listeners
                [range.start.mouse_listeners_index..range.end.mouse_listeners_index]
                .iter_mut()
                .map(|listener| listener.take()),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (GlobalElementId(id.0.clone()), *type_id)),
        );
        self.next_frame.tab_stops.replay(
            &self.rendered_frame.tab_stops.insertion_history
                [range.start.tab_handle_index..range.end.tab_handle_index],
        );

        self.text_system.reuse_layouts(
            range.start.line_layout_index.clone()..range.end.line_layout_index.clone(),
        );
        let old_scene_range = range.start.scene_index..range.end.scene_index;
        self.next_frame
            .scene
            .replay(old_scene_range, &self.rendered_frame.scene);
        true
    }

    /// Returns a previously retained element range that is safe to replay in the current
    /// interaction-only frame.
    pub(crate) fn reusable_interaction_element(
        &self,
        retained_id: &GlobalElementId,
        bounds: Bounds<Pixels>,
    ) -> Option<RetainedElementRange> {
        if self.force_view_cache_refresh()
            || !self.invalidator.active_interaction_only()
            || self.invalidator.interactive_path_is_dirty(retained_id)
        {
            return None;
        }

        let retained = self.rendered_frame.retained_element_ranges.get(retained_id)?;
        if retained.bounds != bounds
            || !self.can_reuse_prepaint(&retained.prepaint_range)
            || !self.can_reuse_paint(&retained.paint_range)
            || !frame_range_is_valid(
                retained.metadata_range.start,
                retained.metadata_range.end,
                self.rendered_frame.retained_element_order.len(),
            )
        {
            return None;
        }
        Some(retained.clone())
    }

    /// Number of post-order reconciliation metadata records emitted into the current frame.
    pub(crate) fn retained_element_metadata_len(&self) -> usize {
        self.next_frame.retained_element_order.len()
    }

    /// Records one normally painted retained element. Descendants have already appended their
    /// metadata, so `metadata_start..end` becomes one contiguous post-order subtree span.
    pub(crate) fn record_retained_element_range(
        &mut self,
        retained_id: GlobalElementId,
        bounds: Bounds<Pixels>,
        prepaint_range: Range<PrepaintStateIndex>,
        paint_range: Range<PaintIndex>,
        metadata_start: usize,
    ) {
        debug_assert!(metadata_start <= self.next_frame.retained_element_order.len());
        let key = ReconcileKey::from(retained_id);
        self.next_frame.retained_element_order.push(key.clone());
        let metadata_end = self.next_frame.retained_element_order.len();
        self.next_frame.retained_element_ranges.insert(
            key,
            RetainedElementRange {
                bounds,
                prepaint_range,
                paint_range,
                metadata_range: metadata_start..metadata_end,
            },
        );
    }

    /// Carries every descendant reconciliation record of a replayed subtree into the current
    /// frame, translating all frame-local indices relative to the replayed root's new location.
    ///
    /// The source metadata is post-order and contiguous, so this is O(subtree retained nodes) and
    /// never scans unrelated entries in the frame-wide retained map.
    pub(crate) fn replay_retained_element_metadata(
        &mut self,
        source_prepaint: &Range<PrepaintStateIndex>,
        source_paint: &Range<PaintIndex>,
        source_metadata: &Range<usize>,
        target_prepaint: &Range<PrepaintStateIndex>,
        target_paint: &Range<PaintIndex>,
    ) -> bool {
        if source_metadata.start >= source_metadata.end
            || !frame_range_is_valid(
                source_metadata.start,
                source_metadata.end,
                self.rendered_frame.retained_element_order.len(),
            )
        {
            return false;
        }

        let target_metadata_start = self.next_frame.retained_element_order.len();
        let mut rebased = Vec::with_capacity(source_metadata.end - source_metadata.start);

        for source_index in source_metadata.clone() {
            let key = self.rendered_frame.retained_element_order[source_index].clone();
            let Some(source_range) = self.rendered_frame.retained_element_ranges.get(&key) else {
                return false;
            };
            if source_range.metadata_range.start < source_metadata.start
                || source_range.metadata_range.end > source_metadata.end
            {
                return false;
            }

            let Some(prepaint_range) = rebase_prepaint_range(
                &source_range.prepaint_range,
                source_prepaint,
                target_prepaint,
            ) else {
                return false;
            };
            let Some(paint_range) =
                rebase_paint_range(&source_range.paint_range, source_paint, target_paint)
            else {
                return false;
            };
            let Some(metadata_start) = target_metadata_start.checked_add(
                source_range
                    .metadata_range
                    .start
                    .checked_sub(source_metadata.start)
                    .unwrap_or(usize::MAX),
            ) else {
                return false;
            };
            let Some(metadata_end) = target_metadata_start.checked_add(
                source_range
                    .metadata_range
                    .end
                    .checked_sub(source_metadata.start)
                    .unwrap_or(usize::MAX),
            ) else {
                return false;
            };

            rebased.push((
                key,
                RetainedElementRange {
                    bounds: source_range.bounds,
                    prepaint_range,
                    paint_range,
                    metadata_range: metadata_start..metadata_end,
                },
            ));
        }

        for (key, range) in rebased {
            self.next_frame.retained_element_order.push(key.clone());
            self.next_frame.retained_element_ranges.insert(key, range);
        }
        true
    }
}

fn rebase_prepaint_range(
    range: &Range<PrepaintStateIndex>,
    source: &Range<PrepaintStateIndex>,
    target: &Range<PrepaintStateIndex>,
) -> Option<Range<PrepaintStateIndex>> {
    Some(
        range.start.rebased_from(&source.start, &target.start)?
            ..range.end.rebased_from(&source.start, &target.start)?,
    )
}

fn rebase_paint_range(
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
