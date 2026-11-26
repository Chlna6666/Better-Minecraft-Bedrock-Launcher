use super::*;
use crate::TransformOrigin;

impl Window {
    /// Acquire a globally unique identifier for the given ElementId.
    /// Only valid for the duration of the provided closure.
    pub fn with_global_id<R>(
        &mut self,
        element_id: ElementId,
        f: impl FnOnce(&GlobalElementId, &mut Self) -> R,
    ) -> R {
        self.element_id_stack.push(element_id);
        let global_id = self.global_ids.borrow_mut().get(&self.element_id_stack);
        let result = f(&global_id, self);
        self.element_id_stack.pop();
        result
    }

    /// Executes the provided function with the specified rem size.
    ///
    /// This method must only be called as part of element drawing.
    #[inline]
    pub fn with_rem_size<F, R>(&mut self, rem_size: Option<impl Into<Pixels>>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        if let Some(rem_size) = rem_size {
            self.rem_size_override_stack.push(rem_size.into());
            let result = f(self);
            self.rem_size_override_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Push a text style onto the stack, and call a function with that style active.
    /// Use [`Window::text_style`] to get the current, combined text style. This method
    /// should only be called as part of element drawing.
    #[inline]
    pub fn with_text_style<F, R>(&mut self, style: Option<TextStyleRefinement>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(style) = style {
            self.text_style_stack.push(style);
            let result = f(self);
            self.text_style_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Updates the cursor style at the platform level. This method should only be called
    /// during the prepaint phase of element drawing.
    pub fn set_cursor_style(&mut self, style: CursorStyle, hitbox: &Hitbox) {
        self.invalidator.debug_assert_paint();
        self.next_frame.cursor_styles.push(CursorStyleRequest {
            hitbox_id: Some(hitbox.id),
            style,
        });
    }

    /// Updates the cursor style for the entire window at the platform level. A cursor
    /// style using this method will have precedence over any cursor style set using
    /// `set_cursor_style`. This method should only be called as part of element drawing.
    pub fn set_window_cursor_style(&mut self, style: CursorStyle) {
        self.invalidator.debug_assert_paint();
        self.next_frame.cursor_styles.push(CursorStyleRequest {
            hitbox_id: None,
            style,
        })
    }

    /// Sets a tooltip to be rendered for the upcoming frame. This method should only be called
    /// during the paint phase of element drawing.
    pub fn set_tooltip(&mut self, tooltip: AnyTooltip) -> TooltipId {
        self.invalidator.debug_assert_prepaint();
        let id = TooltipId(post_inc(&mut self.next_tooltip_id.0));
        self.next_frame
            .tooltip_requests
            .push(Some(TooltipRequest { id, tooltip }));
        id
    }

    /// Invoke the given function with the given content mask after intersecting it
    /// with the current mask. This method should only be called during element drawing.
    #[inline]
    pub fn with_content_mask<R>(
        &mut self,
        mask: Option<ContentMask<Pixels>>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(mask) = mask {
            self.next_frame.input_clip_epoch = self.next_frame.input_clip_epoch.wrapping_add(1);
            let logical_mask = mask.intersect(&self.content_mask());
            let visual_mask = self
                .element_visual_transform
                .transform_mask(&mask)
                .intersect(&self.visual_content_mask());
            self.content_mask_stack.push(logical_mask);
            self.visual_content_mask_stack.push(visual_mask);
            let result = f(self);
            self.content_mask_stack.pop();
            self.visual_content_mask_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Updates the global element offset relative to the current offset. This is used to implement
    /// scrolling. This method should only be called during the prepaint phase of element drawing.
    pub fn with_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();

        if offset.is_zero() {
            return f(self);
        };

        let abs_offset = self.element_offset() + offset;
        self.with_absolute_element_offset(abs_offset, f)
    }

    /// Updates the global element offset based on the given offset. This is used to implement
    /// drag handles and other manual painting of elements. This method should only be called during
    /// the prepaint phase of element drawing.
    pub fn with_absolute_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();
        self.element_offset_stack.push(offset);
        let result = f(self);
        self.element_offset_stack.pop();
        result
    }

    pub(crate) fn with_element_opacity<R>(
        &mut self,
        opacity: Option<f32>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();

        let Some(opacity) = opacity else {
            return f(self);
        };

        let previous_opacity = self.element_opacity;
        self.element_opacity = previous_opacity * opacity;
        let result = f(self);
        self.element_opacity = previous_opacity;
        result
    }

    /// Temporarily prevents retained/view-cache replay while prepainting a subtree whose
    /// renderer-owned primitive binding has changed. The barrier is scoped to the subtree so
    /// unrelated siblings keep their retained ranges, and the previous window-wide recovery flag
    /// is restored exactly after traversal.
    pub(crate) fn with_retained_replay_barrier<R>(
        &mut self,
        enabled: bool,
        prepaint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();
        if !enabled {
            return prepaint(self);
        }

        let previous = self.force_view_cache_refresh;
        self.force_view_cache_refresh = true;
        let result = prepaint(self);
        self.force_view_cache_refresh = previous;
        result
    }

    pub(crate) fn with_scene_animation<R>(
        &mut self,
        animation_id: crate::SceneAnimationId,
        property: crate::TransitionProperty,
        text_raster_scale: f32,
        paint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.apply_scene_animation(animation_id, property, text_raster_scale, None, None, paint)
    }

    pub(crate) fn with_scene_composite_animation<R>(
        &mut self,
        animation_id: crate::SceneAnimationId,
        property: crate::TransitionProperty,
        text_raster_scale: f32,
        capture_bounds: Bounds<Pixels>,
        paint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        debug_assert!(matches!(
            property,
            crate::TransitionProperty::Transform
                | crate::TransitionProperty::Rotation
                | crate::TransitionProperty::ClipReveal
                | crate::TransitionProperty::Translation
        ));
        self.apply_scene_animation(
            animation_id,
            property,
            text_raster_scale,
            Some(capture_bounds),
            None,
            paint,
        )
    }

    pub(crate) fn with_scene_blur_animation<R>(
        &mut self,
        animation_id: crate::SceneAnimationId,
        text_raster_scale: f32,
        capture_bounds: Bounds<Pixels>,
        max_radius_device: f32,
        paint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.apply_scene_animation(
            animation_id,
            crate::TransitionProperty::FilterBlur,
            text_raster_scale,
            Some(capture_bounds),
            Some((capture_bounds, max_radius_device)),
            paint,
        )
    }

    fn apply_scene_animation<R>(
        &mut self,
        animation_id: crate::SceneAnimationId,
        property: crate::TransitionProperty,
        text_raster_scale: f32,
        subtree_capture_bounds: Option<Bounds<Pixels>>,
        blur_capture: Option<(Bounds<Pixels>, f32)>,
        paint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint();

        let completed_text_raster_scale = self
            .animation_engine
            .borrow()
            .completed_scene_text_raster_scale(animation_id);
        if completed_text_raster_scale.is_none()
            && self
                .animation_engine
                .borrow()
                .animation_needs_completion_check(animation_id)
            && let Some(view_id) = self.current_view_or_root()
        {
            self.animation_engine
                .borrow_mut()
                .set_animation_completion_check(animation_id, view_id);
        }

        let text_raster_scale = match completed_text_raster_scale {
            Some(scale) if scale.is_finite() && scale > 0.0 => scale,
            _ if text_raster_scale.is_finite() => text_raster_scale.max(1.0),
            _ => 1.0,
        };
        let previous_text_raster_scale = self.scene_text_raster_scale;
        self.scene_text_raster_scale *= text_raster_scale;

        // Scene primitives currently carry one animation id. Promote an existing parent visual
        // animation to a retained composite before entering a nested animation so the child cannot
        // replace the parent's motion binding. Re-entering this method is safe because
        // paint_element_blur temporarily removes the promoted parent binding while capturing.
        if self.scene_animation.is_some_and(|(_, parent_property)| {
            matches!(
                parent_property,
                crate::TransitionProperty::Opacity
                    | crate::TransitionProperty::Scale
                    | crate::TransitionProperty::Transform
                    | crate::TransitionProperty::Translation
                    | crate::TransitionProperty::Rotation
                    | crate::TransitionProperty::ClipReveal
            )
        }) {
            let capture_bounds =
                subtree_capture_bounds.unwrap_or_else(|| self.content_mask().bounds);
            let result = self.paint_composite_layer(capture_bounds, |window| {
                // paint_composite_layer temporarily removes the promoted parent animation while
                // capturing. Re-enter through the property-specific path so child bounds/blur
                // metadata are not lost merely because this animation is nested.
                if property == crate::TransitionProperty::FilterBlur
                    && let Some((blur_bounds, max_radius_device)) = blur_capture
                {
                    window.with_scene_blur_animation(
                        animation_id,
                        1.0,
                        blur_bounds,
                        max_radius_device,
                        paint,
                    )
                } else if matches!(
                    property,
                    crate::TransitionProperty::Transform
                        | crate::TransitionProperty::Rotation
                        | crate::TransitionProperty::ClipReveal
                ) || property == crate::TransitionProperty::Translation
                    && subtree_capture_bounds.is_some()
                {
                    window.with_scene_composite_animation(
                        animation_id,
                        property,
                        1.0,
                        capture_bounds,
                        paint,
                    )
                } else {
                    window.with_scene_animation(animation_id, property, 1.0, paint)
                }
            });
            self.scene_text_raster_scale = previous_text_raster_scale;
            return result;
        }

        // Blur is a subtree filter, not a per-primitive property. Capture the subtree once with
        // the largest endpoint sigma so its 3-sigma sampling footprint stays stable for the whole
        // animation. Radius sampling is clamped to that endpoint interval; retained frames upload
        // only the dense animation-value sidecar while the primitive, pass descriptors and targets
        // remain byte-for-byte stable.
        if property == crate::TransitionProperty::FilterBlur {
            const MIN_SIGMA: f32 = 1.0 / 4096.0;
            let (capture_bounds, max_radius_device) = blur_capture.unwrap_or_else(|| {
                let scale = (self.scale_factor() * self.visual_scale()).abs();
                (self.content_mask().bounds, MIN_SIGMA * scale.max(1.0))
            });
            let device_scale = (self.scale_factor() * self.visual_scale()).abs();
            let sigma = if max_radius_device.is_finite() && device_scale > f32::EPSILON {
                (max_radius_device.abs() / device_scale).max(MIN_SIGMA)
            } else {
                MIN_SIGMA
            };
            let previous_animation = self.scene_animation.replace((animation_id, property));
            let result = self.paint_element_blur(capture_bounds, px(sigma), paint);
            self.scene_animation = previous_animation;
            self.scene_text_raster_scale = previous_text_raster_scale;
            return result;
        }

        // Subtree transforms, reveals and explicitly clipped translations bind to one retained
        // composite so backgrounds, paths, images, SVGs and text share one sample. Path, underline
        // and surface primitives do not carry direct scene animation ids.
        if matches!(
            property,
            crate::TransitionProperty::Transform
                | crate::TransitionProperty::Rotation
                | crate::TransitionProperty::ClipReveal
        ) || property == crate::TransitionProperty::Translation
            && subtree_capture_bounds.is_some()
        {
            let previous_animation = self.scene_animation.replace((animation_id, property));
            let capture_bounds =
                subtree_capture_bounds.unwrap_or_else(|| self.content_mask().bounds);
            let result = self.paint_composite_layer(capture_bounds, paint);
            self.scene_animation = previous_animation;
            self.scene_text_raster_scale = previous_text_raster_scale;
            return result;
        }

        let previous_animation = self.scene_animation.replace((animation_id, property));
        let result = paint(self);
        self.scene_animation = previous_animation;
        self.scene_text_raster_scale = previous_text_raster_scale;
        result
    }

    pub(crate) fn with_element_scale<R>(
        &mut self,
        bounds: Bounds<Pixels>,
        scale: f32,
        origin: TransformOrigin,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();

        let scale = if scale.is_finite() {
            scale.max(0.0)
        } else {
            1.0
        };
        if scale == 1.0 {
            return f(self);
        }

        let previous_transform = self.element_visual_transform;
        self.element_visual_transform =
            previous_transform.then_scale(scale, origin.resolve(bounds));
        let result = f(self);
        self.element_visual_transform = previous_transform;
        result
    }

    pub(crate) fn visual_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        self.element_visual_transform.transform_bounds(bounds)
    }

    pub(crate) fn visual_point(&self, point: Point<Pixels>) -> Point<Pixels> {
        self.element_visual_transform.transform_point(point)
    }

    pub(crate) fn visual_scale(&self) -> f32 {
        self.element_visual_transform.scale
    }

    pub(crate) fn text_raster_scale(&self) -> f32 {
        self.element_visual_transform.scale * self.scene_text_raster_scale
    }

    pub(crate) fn scene_text_raster_scale(&self) -> f32 {
        self.scene_text_raster_scale
    }

    pub(crate) fn visual_device_bounds(
        &self,
        bounds: Bounds<ScaledPixels>,
        device_scale: f32,
    ) -> Bounds<ScaledPixels> {
        let transform = self.element_visual_transform;
        Bounds {
            origin: point(
                ScaledPixels(
                    bounds.origin.x.0 * transform.scale + transform.translation.x.0 * device_scale,
                ),
                ScaledPixels(
                    bounds.origin.y.0 * transform.scale + transform.translation.y.0 * device_scale,
                ),
            ),
            size: bounds.size.map(|value| value * transform.scale),
        }
    }

    /// Perform prepaint on child elements in a "retryable" manner, so that any side effects
    /// of prepaints can be discarded before prepainting again. This is used to support autoscroll
    /// where we need to prepaint children to detect the autoscroll bounds, then adjust the
    /// element offset and prepaint again. See [`crate::List`] for an example. This method should only be
    /// called during the prepaint phase of element drawing.
    pub fn transact<T, U>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, U>) -> Result<T, U> {
        self.invalidator.debug_assert_prepaint();
        let index = self.prepaint_index();
        let view_records = self.next_frame.view_retention_records.len();
        let claim_start = self.layout_engine.as_ref().map(|engine| engine.claim_log_len());
        let result = f(self);
        if result.is_err() {
            self.truncate_prepaint_to(index);
            self.next_frame.view_retention_records.truncate(view_records);
            if let (Some(start), Some(engine)) = (claim_start, self.layout_engine.as_mut()) {
                let end = engine.claim_log_len();
                engine.release_kept(&crate::layout::LayoutClaimBlocks::request(start..end));
            }
        }
        result
    }

    /// When you call this method during [`Element::prepaint`], containing elements will attempt to
    /// scroll to cause the specified bounds to become visible. When they decide to autoscroll, they will call
    /// [`Element::prepaint`] again with a new set of bounds. See [`crate::List`] for an example of an element
    /// that supports this method being called on the elements it contains. This method should only be
    /// called during the prepaint phase of element drawing.
    pub fn request_autoscroll(&mut self, bounds: Bounds<Pixels>) {
        self.invalidator.debug_assert_prepaint();
        self.requested_autoscroll = Some(bounds);
    }

    /// This method can be called from a containing element such as [`crate::List`] to support the autoscroll behavior
    /// described in [`Self::request_autoscroll`].
    pub fn take_autoscroll(&mut self) -> Option<Bounds<Pixels>> {
        self.invalidator.debug_assert_prepaint();
        self.requested_autoscroll.take()
    }

    /// Obtain the current element offset. This method should only be called during the
    /// prepaint phase of element drawing.
    pub fn element_offset(&self) -> Point<Pixels> {
        self.invalidator.debug_assert_prepaint();
        self.element_offset_stack
            .last()
            .copied()
            .unwrap_or_default()
    }

    /// Obtain the current element opacity. This method should only be called during the
    /// prepaint phase of element drawing.
    #[inline]
    pub(crate) fn element_opacity(&self) -> f32 {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.element_opacity
    }

    /// Obtain the current content mask. This method should only be called during element drawing.
    pub fn content_mask(&self) -> ContentMask<Pixels> {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.content_mask_stack
            .last()
            .cloned()
            .unwrap_or_else(|| ContentMask {
                bounds: Bounds {
                    origin: Point::default(),
                    size: self.viewport_size,
                },
                ..Default::default()
            })
    }

    pub(crate) fn visual_content_mask(&self) -> ContentMask<Pixels> {
        self.visual_content_mask_stack
            .last()
            .cloned()
            .unwrap_or_else(|| {
                ContentMask::new(Bounds {
                    origin: Point::default(),
                    size: self.viewport_size,
                })
            })
    }

    /// Provide elements in the called function with a new namespace in which their identifiers must be unique.
    /// This can be used within a custom element to distinguish multiple sets of child elements.
    pub fn with_element_namespace<R>(
        &mut self,
        element_id: impl Into<ElementId>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.element_id_stack.push(element_id.into());
        let result = f(self);
        self.element_id_stack.pop();
        result
    }

    /// Immediately push an element ID onto the stack. Useful for simplifying IDs in lists.
    ///
    /// Unlike `with_global_id`, this does not materialize the full element path when the caller
    /// only needs a namespace for descendants.
    pub fn with_id<R>(&mut self, id: impl Into<ElementId>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.element_id_stack.push(id.into());
        let result = f(self);
        self.element_id_stack.pop();
        result
    }

    /// Executes the given closure within the context of a tab group.
    #[inline]
    pub fn with_tab_group<R>(&mut self, index: Option<isize>, f: impl FnOnce(&mut Self) -> R) -> R {
        if let Some(index) = index {
            self.next_frame.tab_stops.begin_group(index);
            let result = f(self);
            self.next_frame.tab_stops.end_group();
            result
        } else {
            f(self)
        }
    }

    /// Sets the key context for the current element. This context will be used to translate
    /// keybindings into actions.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn set_key_context(&mut self, context: KeyContext) {
        self.invalidator.debug_assert_paint();
        self.next_frame.dispatch_tree.set_key_context(context);
    }

    /// Sets the focus handle for the current element. This method should only be called as part of the prepaint phase.
    pub fn set_focus_handle(&mut self, focus_handle: &FocusHandle, _: &App) {
        self.invalidator.debug_assert_prepaint();
        if focus_handle.is_focused(self) {
            self.next_frame.focus = Some(focus_handle.id);
        }
        self.next_frame.dispatch_tree.set_focus_id(focus_handle.id);

        if let Some(view_id) = self.rendered_entity_stack.last().copied() {
            self.focus_retained_targets.insert(focus_handle.id, view_id);
        }
    }

    /// Sets the view id for the current element, which will be used to manage view caching.
    ///
    /// This method should only be called as part of element prepaint. We plan on removing this
    /// method eventually when we solve some issues that require us to construct editor elements
    /// directly instead of always using editors via views.
    pub fn set_view_id(&mut self, view_id: EntityId) {
        self.invalidator.debug_assert_prepaint();
        self.next_frame.dispatch_tree.set_view_id(view_id);
    }

    /// Get the entity ID for the currently rendering view
    pub fn current_view(&self) -> EntityId {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.rendered_entity_stack.last().copied().unwrap()
    }

    pub(crate) fn with_rendered_view<R>(
        &mut self,
        id: EntityId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.rendered_entity_stack.push(id);
        let scene_start = (self.invalidator.phase() == DrawPhase::Paint)
            .then(|| self.next_frame.painter.len());
        if let Some(scene_start) = scene_start {
            self.view_bounds_stack.push(ViewBoundsFrame {
                scanned_until: scene_start,
                bounds: None,
            });
        }
        let result = f(self);
        if let Some(scene_start) = scene_start {
            let scene_range = scene_start..self.next_frame.painter.len();
            // Nested views have already folded their bounds into this frame; only the
            // operations this view painted after its last child still need scanning, so
            // each scene operation is visited exactly once across the whole view tree.
            let frame = self.view_bounds_stack.pop();
            let mut view_bounds = frame.and_then(|frame| frame.bounds);
            let scanned_until = frame
                .map(|frame| frame.scanned_until)
                .unwrap_or(scene_range.start);
            if scanned_until < scene_range.end
                && let Some(tail_bounds) = self
                    .next_frame
                    .scene()
                    .bounds_for_range(scanned_until..scene_range.end)
            {
                view_bounds = Some(match view_bounds {
                    Some(bounds) => bounds.union(&tail_bounds),
                    None => tail_bounds,
                });
            }
            // Fold this view's result into the parent, scanning only its gap.
            self.fold_view_bounds(scene_range.clone(), view_bounds);
            if scene_range.start != scene_range.end
                && let Some(bounds) = view_bounds
            {
                self.next_frame
                    .push_retained_scene_segment(RetainedSceneSegment::new(id, bounds, scene_range));
            }
        }
        self.rendered_entity_stack.pop();
        result
    }

    /// Advance the active view's bounds through a completed child or a
    /// geometry-preserving replay. Any preceding freshly painted gap still
    /// belongs to this view and must be scanned exactly once.
    pub(super) fn fold_view_bounds(
        &mut self,
        range: Range<usize>,
        bounds: Option<Bounds<ScaledPixels>>,
    ) {
        let Some(scanned_until) = self.view_bounds_stack.last().map(|frame| frame.scanned_until)
        else {
            return;
        };
        if scanned_until > range.start {
            // Never advance across an overlapping or already consumed span.
            return;
        }
        let gap = if scanned_until < range.start {
            self.next_frame.scene().bounds_for_range(scanned_until..range.start)
        } else {
            None
        };
        let frame = self.view_bounds_stack.last_mut().expect("active view bounds");
        for bounds in gap.iter().chain(bounds.iter()) {
            frame.bounds = Some(match frame.bounds {
                Some(existing) => existing.union(bounds),
                None => *bounds,
            });
        }
        frame.scanned_until = range.end;
    }

    /// Executes the provided function with the specified image cache.
    pub fn with_image_cache<F, R>(&mut self, image_cache: Option<AnyImageCache>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        if let Some(image_cache) = image_cache {
            self.image_cache_stack.push(image_cache);
            let result = f(self);
            self.image_cache_stack.pop();
            result
        } else {
            f(self)
        }
    }
}

