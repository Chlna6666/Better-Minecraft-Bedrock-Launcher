use super::frame::{
    DeferredRetainedMetadata, DeferredRetainedReplay, ReconcileKey, RetainedElementRange,
    RetainedPaintContext,
};
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
        let mut deferred_metadata = mem::take(&mut self.next_frame.deferred_retained_metadata);
        deferred_metadata.resize_with(deferred_draws.len(), DeferredRetainedMetadata::default);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            let replay_prepaint_range = deferred_metadata[*deferred_draw_ix]
                .replay_source
                .as_ref()
                .map(|source| source.prepaint_range.clone());
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
            } else if let Some(source_prepaint_range) = replay_prepaint_range {
                if !self.reuse_prepaint(source_prepaint_range) {
                    self.degrade_current_draw();
                    break;
                }
            } else {
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
        self.next_frame.deferred_retained_metadata = deferred_metadata;
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
        let mut deferred_metadata = mem::take(&mut self.next_frame.deferred_retained_metadata);
        deferred_metadata.resize_with(deferred_draws.len(), DeferredRetainedMetadata::default);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            let retained_metadata = &mut deferred_metadata[*deferred_draw_ix];
            let replay_source = retained_metadata.replay_source.clone();
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

            let metadata_start = self.retained_element_metadata_len();
            let paint_start = self.paint_index();
            let current_view = deferred_draw.current_view;
            if let Some(element) = deferred_draw.element.as_mut() {
                self.with_rendered_view(current_view, |window| {
                    element.paint(window, cx);
                })
            } else if let Some(source) = replay_source {
                let reused = self.with_rendered_view(current_view, |window| {
                    window.reuse_paint(source.paint_range.clone())
                });
                if !reused {
                    self.degrade_current_draw();
                    break;
                }
                let target_paint = paint_start.clone()..self.paint_index();
                if !self.replay_retained_element_metadata(
                    &source.prepaint_range,
                    &source.paint_range,
                    &source.metadata_range,
                    &deferred_draw.prepaint_range,
                    &target_paint,
                ) {
                    self.degrade_current_draw();
                    break;
                }
            } else {
                self.degrade_current_draw();
                break;
            }
            let paint_end = self.paint_index();
            deferred_draw.paint_range = paint_start..paint_end;
            retained_metadata.metadata_range =
                metadata_start..self.retained_element_metadata_len();
            retained_metadata.replay_source = None;
        }
        self.next_frame.deferred_draws = deferred_draws;
        self.next_frame.deferred_retained_metadata = deferred_metadata;
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
            .deferred_retained_metadata
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
        ) || !frame_range_is_valid(
            range.start,
            range.end,
            self.rendered_frame.deferred_retained_metadata.len(),
        ) {
            return false;
        }

        range.into_iter().all(|index| {
            let draw = &self.rendered_frame.deferred_draws[index];
            let metadata = &self.rendered_frame.deferred_retained_metadata[index];
            self.prepaint_range_indices_are_valid(&draw.prepaint_range)
                && self.can_reuse_paint(&draw.paint_range)
                && retained_metadata_range_is_valid(
                    &metadata.metadata_range,
                    self.rendered_frame.retained_element_order.len(),
                )
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

        let deferred_range =
            range.start.deferred_draws_index..range.end.deferred_draws_index;
        self.next_frame
            .deferred_retained_metadata
            .resize_with(self.next_frame.deferred_draws.len(), DeferredRetainedMetadata::default);
        self.next_frame.deferred_retained_metadata.extend(
            deferred_range.clone().map(|index| {
                let deferred_draw = &self.rendered_frame.deferred_draws[index];
                let metadata = &self.rendered_frame.deferred_retained_metadata[index];
                DeferredRetainedMetadata {
                    metadata_range: 0..0,
                    replay_source: Some(DeferredRetainedReplay {
                        prepaint_range: deferred_draw.prepaint_range.clone(),
                        paint_range: deferred_draw.paint_range.clone(),
                        metadata_range: metadata.metadata_range.clone(),
                    }),
                }
            }),
        );
        self.next_frame.deferred_draws.extend(
            self.rendered_frame.deferred_draws[deferred_range]
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
            range.start.line_layout_index.clone()..range.end.line_layout_index,
        );
        let old_scene_range = range.start.scene_index..range.end.scene_index;
        self.next_frame
            .scene
            .replay(old_scene_range, &self.rendered_frame.scene);
        true
    }

    pub(crate) fn current_retained_paint_context(&self) -> RetainedPaintContext {
        RetainedPaintContext {
            opacity: self.element_opacity,
            scale: self.element_visual_transform.scale,
            translation: self.element_visual_transform.translation,
            content_mask: self.content_mask(),
            visual_content_mask: self.visual_content_mask(),
            text_style: self.text_style(),
            rem_size: self.rem_size(),
        }
    }

    /// Returns a previously retained element range that is safe to replay in the current frame.
    ///
    /// Targeted retained frames keep the existing structural-identity rules. Generic dirty frames
    /// are accepted only for side-effect-free plain text whose exact output key matches the previous
    /// frame; this never broadens anonymous replay to interactive elements.
    pub(crate) fn reusable_retained_element(
        &self,
        retained_id: &GlobalElementId,
        bounds: Bounds<Pixels>,
        plain_text_key: Option<&crate::element::RetainedPlainTextKey>,
    ) -> Option<RetainedElementRange> {
        if self.force_view_cache_refresh() {
            return None;
        }

        let targeted_replay = self.invalidator.active_targeted_replay();
        if targeted_replay && self.invalidator.retained_path_is_dirty(retained_id) {
            return None;
        }
        // ReconcileSubtree deliberately separates "visit descendants" from "all descendants are
        // dirty". Until a candidate carries both layout- and semantic-subtree proofs, an ancestor
        // inside that scope must not outer-replay and hide a moving/restyled child whose own bounds
        // have not yet been inspected. This is the conservative bridge to fingerprinted replay.
        if targeted_replay
            && self
                .invalidator
                .retained_path_requires_reconciliation(retained_id)
        {
            return None;
        }

        let retained = self.rendered_frame.retained_element_ranges.get(retained_id)?;
        if retained.paint_context != self.current_retained_paint_context() {
            return None;
        }
        if targeted_replay {
            if !retained.identity_stable || !retained.subtree_stable {
                let current_plain_text = plain_text_key?;
                if retained.plain_text_key.as_ref()? != current_plain_text
                    || !retained_plain_text_range_is_side_effect_free(retained)
                {
                    return None;
                }
            } else if retained_id_is_anonymous(retained_id)
                && retained_range_contains_frame_bound_interactivity(retained)
            {
                return None;
            }
        } else {
            let current_plain_text = plain_text_key?;
            if retained.plain_text_key.as_ref()? != current_plain_text
                || !retained_plain_text_range_is_side_effect_free(retained)
            {
                return None;
            }
        }

        if retained.bounds != bounds
            || !self.can_reuse_prepaint(&retained.prepaint_range)
            || !self.can_reuse_paint(&retained.paint_range)
            || !retained_metadata_range_is_valid(
                &retained.metadata_range,
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
        div_self_scene: Option<crate::element::RetainedDivSelfScene>,
        plain_text_key: Option<crate::element::RetainedPlainTextKey>,
        identity_stable: bool,
        subtree_stable: bool,
    ) {
        debug_assert!(metadata_start <= self.next_frame.retained_element_order.len());
        let paint_context = self.current_retained_paint_context();
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
                paint_context,
                div_self_scene,
                plain_text_key,
                identity_stable,
                subtree_stable,
            },
        );
        if !identity_stable {
            self.next_frame.retained_unstable_identity_count = self
                .next_frame
                .retained_unstable_identity_count
                .saturating_add(1);
        }
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
        if !retained_metadata_range_is_valid(
            source_metadata,
            self.rendered_frame.retained_element_order.len(),
        ) {
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
            let div_self_scene = if let Some(source_self_scene) = source_range.div_self_scene.as_ref() {
                let Some(child_scene_range) = rebase_scene_range(
                    &source_self_scene.child_scene_range,
                    source_paint,
                    target_paint,
                ) else {
                    return false;
                };
                Some(crate::element::RetainedDivSelfScene {
                    style: source_self_scene.style.clone(),
                    child_scene_range,
                })
            } else {
                None
            };
            let Some(metadata_start_offset) = source_range
                .metadata_range
                .start
                .checked_sub(source_metadata.start)
            else {
                return false;
            };
            let Some(metadata_end_offset) = source_range
                .metadata_range
                .end
                .checked_sub(source_metadata.start)
            else {
                return false;
            };
            let Some(metadata_start) = target_metadata_start.checked_add(metadata_start_offset)
            else {
                return false;
            };
            let Some(metadata_end) = target_metadata_start.checked_add(metadata_end_offset) else {
                return false;
            };

            rebased.push((
                key,
                RetainedElementRange {
                    bounds: source_range.bounds,
                    prepaint_range,
                    paint_range,
                    metadata_range: metadata_start..metadata_end,
                    paint_context: source_range.paint_context.clone(),
                    div_self_scene,
                    plain_text_key: source_range.plain_text_key.clone(),
                    identity_stable: source_range.identity_stable,
                    subtree_stable: source_range.subtree_stable,
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

fn retained_id_is_anonymous(retained_id: &GlobalElementId) -> bool {
    matches!(retained_id.0.last(), Some(ElementId::InstanceSlot(_)))
}

fn retained_plain_text_range_is_side_effect_free(retained: &RetainedElementRange) -> bool {
    let prepaint = &retained.prepaint_range;
    let paint = &retained.paint_range;

    retained.metadata_range.end == retained.metadata_range.start.saturating_add(1)
        && prepaint.start.hitboxes_index == prepaint.end.hitboxes_index
        && prepaint.start.tooltips_index == prepaint.end.tooltips_index
        && prepaint.start.deferred_draws_index == prepaint.end.deferred_draws_index
        && prepaint.start.accessed_element_states_index == prepaint.end.accessed_element_states_index
        && paint.start.mouse_listeners_index == paint.end.mouse_listeners_index
        && paint.start.input_handlers_index == paint.end.input_handlers_index
        && paint.start.cursor_styles_index == paint.end.cursor_styles_index
        && paint.start.window_control_hitboxes_index == paint.end.window_control_hitboxes_index
        && paint.start.accessed_element_states_index == paint.end.accessed_element_states_index
        && paint.start.tab_handle_index == paint.end.tab_handle_index
}

fn retained_range_contains_frame_bound_interactivity(retained: &RetainedElementRange) -> bool {
    let prepaint = &retained.prepaint_range;
    let paint = &retained.paint_range;

    prepaint.start.hitboxes_index != prepaint.end.hitboxes_index
        || prepaint.start.tooltips_index != prepaint.end.tooltips_index
        || prepaint.start.deferred_draws_index != prepaint.end.deferred_draws_index
        || prepaint.start.dispatch_tree_index != prepaint.end.dispatch_tree_index
        || prepaint.start.accessed_element_states_index != prepaint.end.accessed_element_states_index
        || paint.start.mouse_listeners_index != paint.end.mouse_listeners_index
        || paint.start.input_handlers_index != paint.end.input_handlers_index
        || paint.start.cursor_styles_index != paint.end.cursor_styles_index
        || paint.start.window_control_hitboxes_index != paint.end.window_control_hitboxes_index
        || paint.start.accessed_element_states_index != paint.end.accessed_element_states_index
        || paint.start.tab_handle_index != paint.end.tab_handle_index
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

fn rebase_scene_range(
    range: &Range<usize>,
    source: &Range<PaintIndex>,
    target: &Range<PaintIndex>,
) -> Option<Range<usize>> {
    let start_offset = range.start.checked_sub(source.start.scene_index)?;
    let end_offset = range.end.checked_sub(source.start.scene_index)?;
    Some(
        target.start.scene_index.checked_add(start_offset)?
            ..target.start.scene_index.checked_add(end_offset)?,
    )
}

fn retained_metadata_range_is_valid(range: &Range<usize>, len: usize) -> bool {
    range.start < range.end && range.end <= len
}

fn frame_range_is_valid(start: usize, end: usize, len: usize) -> bool {
    start <= end && end <= len
}
