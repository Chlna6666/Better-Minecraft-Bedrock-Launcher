use super::*;
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
                    .insert_with_order(clipped_bounds, primitive.order())
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

    fn replay_animation_values(&mut self, range: Range<usize>, previous: &Scene) {
        if previous.animation_values.is_empty() {
            return;
        }
        let referenced: FxHashSet<_> = previous
            .paint_operations
            .range(range)
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::Primitive(primitive) => primitive.animation_id(),
                _ => None,
            })
            .collect();
        let mut emitted: FxHashSet<_> = self
            .animation_values
            .iter()
            .map(|value| (value.animation_id, value.property))
            .collect();
        // Samples are part of the replayed content's binding proof. Keep a freshly
        // emitted sample when an executing wrapper retargets the same stable id.
        self.scene.animation_values.extend(
            previous
                .animation_values
                .iter()
                .filter(|value| {
                    referenced.contains(&value.animation_id)
                        && emitted.insert((value.animation_id, value.property))
                })
                .copied(),
        );
    }

    /// Replays Paint operations and reports whether their draw-order mapping was preserved.
    /// Retention owners use this result to decide whether separately owned Chunk metadata is reusable.
    /// Quad spans must be qualified against this exact source generation by the caller;
    /// their payload is shared only while the current Layer and ordering prefix still match.
    pub fn replay(
        &mut self,
        range: Range<usize>,
        prev_scene: &Scene,
        quad_spans: &[super::QuadReplay],
    ) -> bool {
        self.replay_animation_values(range.clone(), prev_scene);
        self.scene.revision = 0;
        let range_end = range.end;
        let retain_order = !self.retained_prefix_invalid
            && self.scene.paint_operations.len() == range.start
            && self.ordering_prefix_matches_previous(prev_scene, range.start);
        if !retain_order {
            self.retained_prefix_invalid = true;
        }
        let mut index = range.start;
        let mut spans = quad_spans.iter().peekable();
        while index < range.end {
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
                && self.replay_quad_span(prev_scene, span)
            {
                index = span.paint_range.end;
                spans.next();
                continue;
            }
            let operation = &prev_scene.paint_operations[index];
            index += 1;
            match operation {
                PaintOperation::Primitive(primitive) => {
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

    fn replay_quad_span(&mut self, previous: &Scene, span: &super::QuadReplay) -> bool {
        if !self.blur_captures.is_empty()
            || span.paint_range.is_empty()
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
        if !operations.iter().all(|operation| matches!(operation,
            PaintOperation::Primitive(Primitive::Quad(quad))
                if quad.animation_id.is_none() && self.layer_stack.last().is_none_or(|order| *order == quad.order)
        )) { return false; }
        for operation in operations.iter() {
            let PaintOperation::Primitive(primitive) = operation else {
                unreachable!()
            };
            if self.layer_stack.is_empty() {
                self.primitive_bounds
                    .insert_with_order(Self::ordering_bounds(primitive), primitive.order());
            }
        }
        self.scene
            .paint_operations
            .share(&previous.paint_operations, span.paint_range.clone());
        self.scene
            .quads
            .share(&previous.quads, span.quad_range.clone());
        self.scene.replayed_primitives += operations.len();
        true
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

        current_prefix
            .iter()
            .zip(previous_prefix.iter())
            .all(|(current, previous)| ordering_operations_match(current, previous))
    }
}
fn ordering_operations_match(current: &PaintOperation, previous: &PaintOperation) -> bool {
    match (current, previous) {
        (PaintOperation::Primitive(current), PaintOperation::Primitive(previous)) => {
            current.order() == previous.order()
                && current.bounds().intersect(&current.content_mask().bounds)
                    == previous.bounds().intersect(&previous.content_mask().bounds)
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
