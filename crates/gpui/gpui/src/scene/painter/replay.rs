use super::*;

enum ShadowReplay {
    Shared(usize),
    Rebind(usize),
}
/// Candidate-local sample references. Small sample tables use an inline
/// mask; larger tables allocate only if an animated primitive is encountered.
enum ReplayAnimationReferences {
    None,
    Inline(u16),
    Many(FxHashSet<SceneAnimationId>),
}

impl ReplayAnimationReferences {
    fn new(previous: &Scene) -> Self {
        match previous.animation_values.len() {
            0 => Self::None,
            1..=16 => Self::Inline(0),
            _ => Self::Many(FxHashSet::default()),
        }
    }

    fn record(&mut self, primitive: PrimitiveRef<'_>, previous: &Scene) {
        if matches!(self, Self::None) {
            return;
        }
        let Some(id) = primitive.animation_id() else { return; };
        match self {
            Self::None => {}
            Self::Inline(mask) => {
                for (index, value) in previous.animation_values.iter().enumerate() {
                    if value.animation_id == id {
                        *mask |= 1 << index;
                    }
                }
            }
            Self::Many(ids) => { ids.insert(id); }
        }
    }

    fn append(self, previous: &Scene, current: &mut Vec<SceneAnimationValue>) {
        // Keep the original source sample order and the destination's newer
        // values. An identical id/property pair is emitted at most once.
        match self {
            Self::None | Self::Inline(0) => {}
            Self::Inline(mask) => {
                for (index, value) in previous.animation_values.iter().enumerate() {
                    if mask & (1 << index) != 0
                        && !current.iter().any(|old| {
                            old.animation_id == value.animation_id && old.property == value.property
                        })
                    {
                        current.push(*value);
                    }
                }
            }
            Self::Many(ids) => {
                if ids.is_empty() {
                    return;
                }
                let mut emitted: FxHashSet<_> = current.iter()
                    .map(|value| (value.animation_id, value.property))
                    .collect();
                current.extend(previous.animation_values.iter().filter(|value| {
                    ids.contains(&value.animation_id)
                        && emitted.insert((value.animation_id, value.property))
                }).copied());
            }
        }
    }
}

impl Painter {
    fn replay_primitive(&mut self, primitive: PrimitiveRef<'_>, retain_order: bool) {
        // A retained-order proof belongs to the exact ordering prefix that produced it.
        // If an enclosing layer was rebuilt with a different DrawOrder, keeping the
        // primitive's old order would mix two frame-local ordering domains. Rebind to
        // the current layer and invalidate the remainder of this retained prefix.
        let retain_order = retain_order && !self.retained_prefix_invalid;
        let order = if retain_order {
            let clipped_bounds = Self::ordering_bounds(primitive);
            if clipped_bounds.is_empty() {
                return;
            }
            if let Some(layer_order) = self.layer_stack.last().copied() {
                if layer_order != primitive.order() {
                    self.retained_prefix_invalid = true;
                }
                layer_order
            } else {
                self.primitive_bounds
                    .insert_ordered(clipped_bounds, primitive.order())
            }
        } else if let Some(order) = self.order_for_primitive(primitive) {
            order
        } else {
            return;
        };
        self.scene.replayed_primitives = self.scene.replayed_primitives.saturating_add(1);
        self.push_ordered_primitive(primitive.to_primitive(), order, true);
    }

    /// Replays Paint operations and reports whether their draw-order mapping was preserved.
    /// Retention owners use this result to decide whether separately owned Chunk metadata is reusable.
    /// Quad spans must be qualified against this exact source generation by the caller;
    /// their payload is shared only while the current Layer and ordering prefix still match.
    pub fn replay(
        &mut self,
        range: Range<usize>,
        prev_scene: &Scene,
        quad_spans: impl IntoIterator<Item = super::QuadReplay>,
    ) -> bool {
        self.replay_with_ordering(range, prev_scene, quad_spans, None)
    }

    /// The source Painter belongs to the exact committed Scene being replayed.
    pub(crate) fn replay_with_ordering(
        &mut self,
        range: Range<usize>,
        prev_scene: &Scene,
        quad_spans: impl IntoIterator<Item = super::QuadReplay>,
        previous_painter: Option<&Painter>,
    ) -> bool {
        let previous_tree = previous_painter.map(|painter| &painter.primitive_bounds);
        let mut animation_references = ReplayAnimationReferences::new(prev_scene);
        self.scene.revision = 0;
        let range_end = range.end;
        let retain_order = !self.retained_prefix_invalid
            && self.scene.paint_operations.len() == range.start
            && self.ordering_prefix_matches_previous(prev_scene, range.start);
        if !retain_order {
            self.retained_prefix_invalid = true;
        }
        let mut index = range.start;
        let mut shadow_scan_end = index;
        let mut spans = quad_spans.into_iter().peekable();
        let mut runs = prev_scene.paint_operations.range(range.clone()).runs();
        let mut run = super::super::operations::OperationRun::default();
        let mut run_end = range.start;
        while index < range.end {
            // Shared Quad/Shadow spans can jump across several backing runs.
            // Walk those boundaries without indexing each skipped operation.
            while index >= run_end {
                run = runs.next().expect("validated replay range");
                run_end += run.len();
            }
            let operation = run.get(index - (run_end - run.len())).expect("validated replay operation");
            if index >= shadow_scan_end
                && retain_order
                && !self.retained_prefix_invalid
                && matches!(operation, PaintOperationRef::Primitive(PrimitiveRef::Shadow(_)))
            {
                match self.replay_shadows(prev_scene, index..range.end, previous_tree) {
                    ShadowReplay::Shared(end) => {
                        index = end;
                        continue;
                    }
                    ShadowReplay::Rebind(end) => shadow_scan_end = end,
                }
            }
            while spans
                .peek()
                .is_some_and(|span| span.paint_range.start < index)
            {
                spans.next();
            }
            if let Some(span) = spans.peek()
                && span.paint_range.start == index
                && span.paint_range.end <= range.end
                && retain_order
                && !self.retained_prefix_invalid
                && self.replay_quad_span(prev_scene, span, previous_tree)
            {
                index = span.paint_range.end;
                spans.next();
                continue;
            }
            // Ordinary static glyphs, image sprites and quads do not need
            // a retained upload chunk in order to share their operation
            // payload. Keep the specialized typed Quad/Shadow path ahead
            // of this one, and never skip the next candidate chunk.
            if retain_order && !self.retained_prefix_invalid
                && previous_painter.is_some() && prev_scene.revision != 0
                && spans.peek().is_none_or(|span| span.paint_range.start > index)
            {
                let end = spans.peek().map_or(run_end, |span| run_end.min(span.paint_range.start));
                let start = index - (run_end - run.len());
                if let Some(end) = self.replay_static_primitive_run(
                    prev_scene, index, run.range(start..start + end - index), previous_tree,
                ) {
                    index = end;
                    continue;
                }
            }
            if let PaintOperationRef::StartBlur(config) = operation
                && let Some(end) = self.replay_static_capture(
                    prev_scene, index..range.end, config,
                )
            {
                index = end;
                continue;
            }
            index += 1;
            match operation {
                PaintOperationRef::Primitive(primitive) => {
                    // Shared Quad and Shadow runs are qualified as static.
                    // All remaining operations are visited here exactly once,
                    // including primitives that are subsequently clipped out.
                    animation_references.record(primitive, prev_scene);
                    if self.blur_captures.last().is_some() {
                        let (captured_primitive, capture_prefix_invalid) = {
                            let capture = self.blur_captures.last_mut().unwrap();
                            let operation_start = capture.scene.paint_operations.len();
                            capture.scene.replay_primitive(primitive, retain_order);
                            let captured_primitive = capture
                                .scene
                                .paint_operations
                                .view(operation_start)
                                .and_then(|operation| match operation {
                                    PaintOperationRef::Primitive(primitive) => Some(primitive.to_primitive()),
                                    PaintOperationRef::StartLayer(_)
                                    | PaintOperationRef::EndLayer
                                    | PaintOperationRef::StartBlur(_)
                                    | PaintOperationRef::EndBlur
                                    | PaintOperationRef::DeferredScroll(_) => None,
                                });
                            (captured_primitive, capture.scene.retained_prefix_invalid)
                        };
                        if capture_prefix_invalid {
                            self.retained_prefix_invalid = true;
                        }
                        if let Some(primitive) = captured_primitive {
                            self.scene
                                .paint_operations
                                .push(PaintOperation::Primitive(primitive));
                        }
                    } else {
                        self.replay_primitive(primitive, retain_order);
                    }
                }
                PaintOperationRef::DeferredScroll(placement) => {
                    // UI retention carries the one-operation placement.
                    // The ordinary owner reconstruction deliberately has
                    // no previous UI Painter and expands it instead.
                    if previous_painter.is_none() || !self.retain_deferred_scroll(placement.clone()) {
                        // Eager expansion DOES insert ordinary geometry and
                        // can change every following draw-order assignment.
                        self.retained_prefix_invalid = true;
                        placement.replay(self);
                    }
                }
                PaintOperationRef::StartLayer(bounds) => {
                    if let Some(capture) = self.blur_captures.last_mut() {
                        capture.scene.push_replayed_layer(*bounds);
                        self.scene
                            .paint_operations
                            .push(PaintOperation::StartLayer(*bounds));
                    } else {
                        self.push_replayed_layer(*bounds);
                    }
                }
                PaintOperationRef::EndLayer => {
                    if let Some(capture) = self.blur_captures.last_mut() {
                        capture.scene.pop_replayed_layer();
                        self.scene.paint_operations.push(PaintOperation::EndLayer);
                    } else {
                        self.pop_replayed_layer();
                    }
                }
                PaintOperationRef::StartBlur(config) => {
                    // Replay must preserve the same published content owner as
                    // a normal isolated-layer paint. The closing capture still
                    // proves equality before it adopts this candidate.
                    if let Some(previous) = config.layer_id.as_ref()
                        .and_then(|id| prev_scene.blur_content_for(id))
                    {
                        self.begin_previous_blur(config.clone(), Some(previous));
                    } else {
                        self.begin_blur(config.clone());
                    }
                }
                PaintOperationRef::EndBlur => self.end_blur(),
            }
        }
        animation_references.append(prev_scene, &mut self.scene.animation_values);
        if retain_order
            && !self.retained_prefix_invalid
            && self.scene.paint_operations.len() == range_end
        {
            self.retained_prefix_verified_len = range_end;
            true
        } else {
            if retain_order {
                self.retained_prefix_invalid = true;
            }
            false
        }
    }

    /// Carry an unchanged operation range without cloning its large enum
    /// payload or constructing per-primitive ordering tree nodes. Typed
    /// streams still receive their existing contiguous, sortable instances.
    /// The caller has proved the entire preceding ordering prefix.
    fn replay_static_primitive_run(
        &mut self,
        previous: &Scene,
        start: usize,
        operations: super::super::operations::OperationRun<'_>,
        previous_tree: Option<&BoundsTree<ScaledPixels>>,
    ) -> Option<usize> {
        if !self.blur_captures.is_empty() {
            return None;
        }
        let layer_order = self.layer_stack.last().copied();
        let static_primitive = |operation: PaintOperationRef<'_>| {
            let PaintOperationRef::Primitive(primitive) = operation else { return false; };
            matches!(primitive, PrimitiveRef::Quad(_) | PrimitiveRef::Underline(_)
                | PrimitiveRef::MonochromeSprite(_) | PrimitiveRef::PolychromeSprite(_))
                && primitive.animation_id().is_none()
                && layer_order.is_none_or(|order| order == primitive.order())
                && !Self::ordering_bounds(primitive).is_empty()
        };
        let count = operations.iter().take_while(|op| static_primitive(*op)).count();
        if count == 0 {
            return None;
        }
        self.flush_draw_payloads();
        let range = start..start + count;
        let primitives = || operations.range(0..count).iter().map(|operation| match operation {
            PaintOperationRef::Primitive(primitive) => primitive,
            _ => unreachable!("qualified static primitive"),
        });
        if layer_order.is_none() {
            // The source prefix and unchanged static geometry establish the
            // exact draw order. A whole contiguous run can import the old
            // balanced page, or build it once on the first replay.
            self.primitive_bounds.extend_ordered(
                range.clone(), previous_tree,
                primitives().map(|primitive| (Self::ordering_bounds(primitive), primitive.order())),
            );
        }
        // Static quads can borrow contiguous sorted slots directly from the
        // operation owner, without requiring a retained GPU upload chunk.
        // Glyphs first try the existing complete draw-order group, then
        // borrow a contiguous operation-owned range if it is already sorted.
        let mut cursor = 0;
        while cursor < count {
            if matches!(operations.get(cursor).expect("qualified operation"), PaintOperationRef::Primitive(PrimitiveRef::Quad(_))) {
                let end = cursor + operations.range(cursor..count).iter().take_while(|op| {
                    matches!(op, PaintOperationRef::Primitive(PrimitiveRef::Quad(_)))
                }).count();
                let run = operations.range(cursor..end);
                if !run.share_quad_run(&mut self.scene.quads) {
                    for op in run.iter() {
                        let PaintOperationRef::Primitive(primitive) = op else { unreachable!() };
                        self.push_ordered_primitive(primitive.to_primitive(), primitive.order(), false);
                    }
                }
                cursor = end;
                continue;
            }
            if matches!(operations.get(cursor).expect("qualified operation"), PaintOperationRef::Primitive(PrimitiveRef::MonochromeSprite(_))) {
                let end = cursor + operations.range(cursor..count).iter().take_while(|op| {
                    matches!(op, PaintOperationRef::Primitive(PrimitiveRef::MonochromeSprite(_)))
                }).count();
                let mut first = DrawOrder::MAX;
                let mut last = DrawOrder::MIN;
                for op in operations.range(cursor..end).iter() {
                    let PaintOperationRef::Primitive(PrimitiveRef::MonochromeSprite(sprite)) = op else { unreachable!() };
                    first = first.min(sprite.order);
                    last = last.max(sprite.order);
                }
                let typed = &previous.monochrome_sprites;
                let from = typed.partition_point(|sprite| sprite.order < first);
                let to = typed.partition_point(|sprite| sprite.order <= last);
                if to - from == end - cursor {
                    self.scene.monochrome_sprites.share(typed, from..to);
                    cursor = end;
                    continue;
                }
                let run = operations.range(cursor..end);
                if !run.share_mono_run(&mut self.scene.monochrome_sprites) {
                    for op in run.iter() {
                        let PaintOperationRef::Primitive(primitive) = op else { unreachable!() };
                        self.push_ordered_primitive(primitive.to_primitive(), primitive.order(), false);
                    }
                }
                cursor = end;
                continue;
            }
            if matches!(operations.get(cursor).expect("qualified operation"), PaintOperationRef::Primitive(PrimitiveRef::PolychromeSprite(_))) {
                let end = cursor + operations.range(cursor..count).iter().take_while(|op| {
                    matches!(op, PaintOperationRef::Primitive(PrimitiveRef::PolychromeSprite(_)))
                }).count();
                let run = operations.range(cursor..end);
                if !run.share_poly_run(&mut self.scene.polychrome_sprites) {
                    for op in run.iter() {
                        let PaintOperationRef::Primitive(primitive) = op else { unreachable!() };
                        self.push_ordered_primitive(primitive.to_primitive(), primitive.order(), false);
                    }
                }
                cursor = end;
                continue;
            }
            let PaintOperationRef::Primitive(primitive) = operations.get(cursor).expect("qualified operation") else { unreachable!() };
            self.push_ordered_primitive(primitive.to_primitive(), primitive.order(), false);
            cursor += 1;
        }
        self.scene.paint_operations.share(&previous.paint_operations, range.clone());
        self.scene.replayed_primitives = self.scene.replayed_primitives.saturating_add(count);
        Some(range.end)
    }

    /// Adopt a complete, static leaf capture directly from the committed scene.
    /// The raw operation range remains available to damage and retention, but
    /// no child painter, ordering tree or typed stream is rebuilt.
    fn replay_static_capture(
        &mut self,
        previous: &Scene,
        range: Range<usize>,
        config: &BlurCapture,
    ) -> Option<usize> {
        if !self.blur_captures.is_empty()
            || config.radius.0 != 0.0
            || config.animation_id.is_some()
            || !previous.animation_values.is_empty()
        {
            return None;
        }
        let first = previous.capture_groups.partition_point(|group| group.range.end <= range.start);
        let group = previous.capture_groups.get(first)?;
        // Groups close inside-out; the first group must be this exact leaf.
        if group.range.start != range.start || group.range.end > range.end {
            return None;
        }
        let (revision, order) = group.output?;
        let first = previous.blurs.partition_point(|blur| blur.order < order);
        let mut matches = previous.blurs[first..].iter()
            .take_while(|blur| blur.order == order)
            .filter(|blur| {
                blur.content.revision == revision
                    && blur.layer_id == config.layer_id
                    && blur.bounds == config.bounds
                    && blur.content_mask == config.content_mask
                    && blur.radius == config.radius
                    && blur.opacity == config.opacity
                    && blur.animation_id.is_none()
            });
        let blur = matches.next()?;
        if matches.next().is_some()
            || !blur.content.capture_groups.is_empty()
            || !blur.content.animation_values.is_empty()
            || !blur.content.blurs.is_empty()
            || !blur.content.backdrop_blurs.is_empty()
            || !blur.content.surfaces.is_empty()
            || !blur.content.renderer_extensions.is_empty()
        {
            return None;
        }

        let primitive = Primitive::from(blur.clone());
        let new_order = self.order_for_primitive(primitive.as_ref())?;
        if new_order != order {
            self.retained_prefix_invalid = true;
        }
        self.flush_draw_payloads();
        let start = self.scene.len();
        self.push_ordered_primitive(primitive, new_order, false);
        self.scene.paint_operations.share(&previous.paint_operations, group.range.clone());
        self.scene.capture_groups.push(super::super::geometry::CaptureGroup {
            range: start..self.scene.len(),
            bounds: group.bounds,
            output: Some((revision, new_order)),
        });
        self.scene.replayed_primitives = self.scene.replayed_primitives.saturating_add(1);
        Some(group.range.end)
    }

    fn replay_quad_span(
        &mut self,
        previous: &Scene,
        span: &super::QuadReplay,
        previous_tree: Option<&BoundsTree<ScaledPixels>>,
    ) -> bool {
        if !self.blur_captures.is_empty()
            || span.source_revision == 0
            || span.source_revision != previous.revision
            || span.paint_range.is_empty()
            || span.quad_range.start > span.quad_range.end
            || span.quad_range.end > previous.quads.len()
            || span.paint_range.len() != span.quad_range.len()
        {
            return false;
        }
        let Some(operations) = previous
            .paint_operations
            .get_range(span.paint_range.clone())
        else {
            return false;
        };
        // The paired, finished chunk planner already proved static Quad
        // types, animation absence and exclusive sorted order coverage.
        // Only the destination ordering context remains to be checked.
        if let Some(&order) = self.layer_stack.last() {
            if previous.quads[span.quad_range.start].order != order
                || previous.quads[span.quad_range.end - 1].order != order
            {
                return false;
            }
        } else {
            // The qualified typed range has exactly the same leaves as the
            // operation range. Build a balanced subtree without per-leaf
            // searches through the destination's existing ordering tree.
            self.primitive_bounds.extend_ordered(
                span.paint_range.clone(),
                previous_tree,
                previous.quads.slices(span.quad_range.clone()).flatten().map(|quad| {
                    (quad.bounds.intersect(&quad.content_mask.bounds), quad.order)
                }),
            );
        }
        #[cfg(test)]
        assert!(operations.views().all(|operation| matches!(operation,
            PaintOperationRef::Primitive(PrimitiveRef::Quad(quad))
                if quad.animation_id.is_none()
                    && self.layer_stack.last().is_none_or(|order| *order == quad.order)
        )));
        self.flush_draw_payloads();
        self.scene
            .paint_operations
            .share(&previous.paint_operations, span.paint_range.clone());
        self.scene
            .quads
            .share(&previous.quads, span.quad_range.clone());
        self.scene.replayed_primitives += operations.len();
        true
    }

    /// Basic drawing-data replay, independent of GPU Chunk/cache eligibility.
    /// Only an unchanged ordering prefix and a complete static typed range may
    /// share storage; changed layers/captures take the ordinary rebinding path.
    fn replay_shadows(
        &mut self,
        previous: &Scene,
        range: Range<usize>,
        previous_tree: Option<&BoundsTree<ScaledPixels>>,
    ) -> ShadowReplay {
        if !self.blur_captures.is_empty() {
            return ShadowReplay::Rebind(range.start + 1);
        }
        // The prior painter admitted this exact static range and its typed
        // payload was shared. Its immutable ordering page also proves the
        // source operations are static, so only check typed coverage again.
        if self.layer_stack.is_empty()
            && let Some((end, first, last)) = previous_tree
                .and_then(|tree| tree.sorted_run_orders(range.clone()))
            && let Some(typed) = Self::shadow_range_for_orders(previous, first, last, end - range.start)
        {
            return self.share_shadow_range(previous, range.start..end, typed, previous_tree);
        }
        let mut count = 0;
        let mut first = DrawOrder::MAX;
        let mut last = DrawOrder::MIN;
        for operation in previous.paint_operations.range(range.clone()).views() {
            let PaintOperationRef::Primitive(PrimitiveRef::Shadow(shadow)) = operation else {
                break;
            };
            if shadow.animation_id.is_some()
                || self
                    .layer_stack
                    .last()
                    .is_some_and(|order| *order != shadow.order)
            {
                break;
            }
            first = first.min(shadow.order);
            last = last.max(shadow.order);
            count += 1;
        }
        if count == 0 {
            return ShadowReplay::Rebind(range.start + 1);
        }
        let Some(typed) = Self::shadow_range_for_orders(previous, first, last, count) else {
            return ShadowReplay::Rebind(range.start + count);
        };
        self.share_shadow_range(previous, range.start..range.start + count, typed, previous_tree)
    }

    fn shadow_range_for_orders(
        previous: &Scene,
        first: DrawOrder,
        last: DrawOrder,
        count: usize,
    ) -> Option<Range<usize>> {
        let start = previous.shadows.partition_point(|shadow| shadow.order < first);
        let end = previous.shadows.partition_point(|shadow| shadow.order <= last);
        // Partial equal-order ranges can include siblings not present in this
        // replay. Never import those typed instances or lose their relative order.
        (end - start == count).then_some(start..end)
    }

    fn share_shadow_range(
        &mut self,
        previous: &Scene,
        paint_range: Range<usize>,
        typed: Range<usize>,
        previous_tree: Option<&BoundsTree<ScaledPixels>>,
    ) -> ShadowReplay {
        #[cfg(test)]
        assert!(previous.paint_operations.range(paint_range.clone()).views().all(|op| {
            matches!(op, PaintOperationRef::Primitive(PrimitiveRef::Shadow(shadow))
                if shadow.animation_id.is_none())
        }));
        if self.layer_stack.is_empty() {
            self.primitive_bounds.extend_ordered(
                paint_range.clone(),
                previous_tree,
                previous.shadows.slices(typed.clone()).flatten().map(|shadow| {
                    (shadow.bounds.intersect(&shadow.content_mask.bounds), shadow.order)
                }),
            );
        }
        self.flush_draw_payloads();
        self.scene.paint_operations.share(&previous.paint_operations, paint_range.clone());
        self.scene.shadows.share(&previous.shadows, typed);
        self.scene.replayed_primitives += paint_range.len();
        ShadowReplay::Shared(paint_range.end)
    }

    fn ordering_prefix_matches_previous(&self, prev_scene: &Scene, prefix_end: usize) -> bool {
        let prefix_start = self.retained_prefix_verified_len;
        let Some(current_prefix) = self
            .scene
            .paint_operations
            .get_range(prefix_start..prefix_end)
        else {
            return false;
        };
        let Some(previous_prefix) = prev_scene
            .paint_operations
            .get_range(prefix_start..prefix_end)
        else {
            return false;
        };

        current_prefix.matches_views(previous_prefix, ordering_operations_match)
    }
}
fn ordering_operations_match(current: PaintOperationRef<'_>, previous: PaintOperationRef<'_>) -> bool {
    match (current, previous) {
        (PaintOperationRef::Primitive(current), PaintOperationRef::Primitive(previous)) => {
            current.order() == previous.order()
                && Painter::ordering_bounds(current) == Painter::ordering_bounds(previous)
        }
        (PaintOperationRef::DeferredScroll(current), PaintOperationRef::DeferredScroll(previous)) => {
            // Both operations reserve the same logical slot, with no
            // ordinary BoundsTree leaf. Translation is not an input to
            // ordinary DrawOrder; a changed source remains conservative.
            current.same_source_placement_domain(previous)
        }
        (PaintOperationRef::StartLayer(current), PaintOperationRef::StartLayer(previous)) => {
            current == previous
        }
        (PaintOperationRef::EndLayer, PaintOperationRef::EndLayer)
        | (PaintOperationRef::EndBlur, PaintOperationRef::EndBlur) => true,
        (PaintOperationRef::StartBlur(current), PaintOperationRef::StartBlur(previous)) => {
            current == previous
        }
        _ => false,
    }
}
