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

    fn record(&mut self, primitive: &Primitive, previous: &Scene) {
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
    fn replay_primitive(&mut self, primitive: &Primitive, retain_order: bool) {
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
        let primitive_kind = self.push_replayed_primitive(primitive, order);
        let mut primitive = primitive.clone();
        primitive.set_order(order);
        if let (ScenePrimitiveKind::Path, Primitive::Path(path)) = (primitive_kind, &mut primitive)
        {
            path.id = PathId(self.scene.paths.len().saturating_sub(1));
        }
        self.scene
            .paint_operations
            .push(PaintOperation::Primitive(primitive));
    }
    fn push_replayed_primitive(
        &mut self,
        primitive: &Primitive,
        order: DrawOrder,
    ) -> ScenePrimitiveKind {
        match primitive {
            Primitive::Shadow(shadow) => {
                let mut shadow = *shadow;
                shadow.order = order;
                self.scene.shadows.push(shadow);
                ScenePrimitiveKind::Shadow
            }
            Primitive::Quad(quad) => {
                let mut quad = *quad;
                quad.order = order;
                self.scene.quads.push(quad);
                ScenePrimitiveKind::Quad
            }
            Primitive::Path(path) => {
                let mut path = path.clone();
                path.order = order;
                path.id = PathId(self.scene.paths.len());
                self.scene.paths.push(path);
                ScenePrimitiveKind::Path
            }
            Primitive::Underline(underline) => {
                let mut underline = *underline;
                underline.order = order;
                self.scene.underlines.push(underline);
                ScenePrimitiveKind::Underline
            }
            Primitive::MonochromeSprite(sprite) => {
                let mut sprite = *sprite;
                sprite.order = order;
                self.scene.monochrome_sprites.push(sprite);
                ScenePrimitiveKind::MonochromeSprite
            }
            Primitive::PolychromeSprite(sprite) => {
                let mut sprite = *sprite;
                sprite.order = order;
                self.scene.polychrome_sprites.push(sprite);
                ScenePrimitiveKind::PolychromeSprite
            }
            Primitive::Surface(surface) => {
                let mut surface = surface.clone();
                surface.order = order;
                self.scene.surfaces.push(surface);
                ScenePrimitiveKind::Surface
            }
            Primitive::BackdropBlur(blur) => {
                let mut blur = blur.clone();
                blur.order = order;
                self.scene.backdrop_blurs.push(blur);
                ScenePrimitiveKind::BackdropBlur
            }
            Primitive::Blur(blur) => {
                let mut blur = blur.clone();
                blur.order = order;
                self.scene.blurs.push(blur);
                ScenePrimitiveKind::Blur
            }
            Primitive::RendererExtension(extension) => {
                let mut extension = extension.clone();
                extension.order = order;
                self.scene.renderer_extensions.push(extension);
                ScenePrimitiveKind::RendererExtension
            }
        }
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
        let mut runs = prev_scene.paint_operations.range(range.clone()).slices();
        let mut run: &[PaintOperation] = &[];
        let mut run_end = range.start;
        while index < range.end {
            // Shared Quad/Shadow spans can jump across several backing runs.
            // Walk those boundaries without indexing each skipped operation.
            while index >= run_end {
                run = runs.next().expect("validated replay range");
                run_end += run.len();
            }
            let operation = &run[index - (run_end - run.len())];
            if index >= shadow_scan_end
                && retain_order
                && !self.retained_prefix_invalid
                && matches!(operation, PaintOperation::Primitive(Primitive::Shadow(_)))
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
            index += 1;
            match operation {
                PaintOperation::Primitive(primitive) => {
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
                                .get(operation_start)
                                .and_then(|operation| match operation {
                                    PaintOperation::Primitive(primitive) => Some(primitive.clone()),
                                    PaintOperation::StartLayer(_)
                                    | PaintOperation::EndLayer
                                    | PaintOperation::StartBlur(_)
                                    | PaintOperation::EndBlur => None,
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
                PaintOperation::StartLayer(bounds) => {
                    if let Some(capture) = self.blur_captures.last_mut() {
                        capture.scene.push_replayed_layer(*bounds);
                        self.scene
                            .paint_operations
                            .push(PaintOperation::StartLayer(*bounds));
                    } else {
                        self.push_replayed_layer(*bounds);
                    }
                }
                PaintOperation::EndLayer => {
                    if let Some(capture) = self.blur_captures.last_mut() {
                        capture.scene.pop_replayed_layer();
                        self.scene.paint_operations.push(PaintOperation::EndLayer);
                    } else {
                        self.pop_replayed_layer();
                    }
                }
                PaintOperation::StartBlur(config) => self.begin_blur(config.clone()),
                PaintOperation::EndBlur => self.end_blur(),
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
        assert!(operations.iter().all(|operation| matches!(operation,
            PaintOperation::Primitive(Primitive::Quad(quad))
                if quad.animation_id.is_none()
                    && self.layer_stack.last().is_none_or(|order| *order == quad.order)
        )));
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
        for operation in previous.paint_operations.range(range.clone()).iter() {
            let PaintOperation::Primitive(Primitive::Shadow(shadow)) = operation else {
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
        assert!(previous.paint_operations.range(paint_range.clone()).iter().all(|op| {
            matches!(op, PaintOperation::Primitive(Primitive::Shadow(shadow))
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

        current_prefix.matches_by(previous_prefix, ordering_operations_match)
    }
}
fn ordering_operations_match(current: &PaintOperation, previous: &PaintOperation) -> bool {
    match (current, previous) {
        (PaintOperation::Primitive(current), PaintOperation::Primitive(previous)) => {
            current.order() == previous.order()
                && Painter::ordering_bounds(current) == Painter::ordering_bounds(previous)
        }
        (PaintOperation::StartLayer(current), PaintOperation::StartLayer(previous)) => {
            current == previous
        }
        (PaintOperation::EndLayer, PaintOperation::EndLayer)
        | (PaintOperation::EndBlur, PaintOperation::EndBlur) => true,
        (PaintOperation::StartBlur(current), PaintOperation::StartBlur(previous)) => {
            current == previous
        }
        _ => false,
    }
}
