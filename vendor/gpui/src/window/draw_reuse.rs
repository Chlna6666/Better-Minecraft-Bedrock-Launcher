use super::*;

impl Window {
    pub(super) fn prepaint_deferred_draws(
        &mut self,
        deferred_draw_indices: &[usize],
        cx: &mut App,
    ) {
        assert_eq!(self.element_id_stack.len(), 0);

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
            self.text_style_stack
                .clone_from(&deferred_draw.text_style_stack);
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
        self.text_style_stack.clear();
    }

    pub(super) fn paint_deferred_draws(&mut self, deferred_draw_indices: &[usize], cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
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
                    text_style_stack: deferred_draw.text_style_stack.clone(),
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
}

fn frame_range_is_valid(start: usize, end: usize, len: usize) -> bool {
    start <= end && end <= len
}
