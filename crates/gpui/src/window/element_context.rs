use super::*;
use crate::TransformOrigin;

#[derive(Default)]
struct RetainedIdentityScope {
    source_occurrences:
        FxHashMap<core::panic::Location<'static>, (u32, Rc<Cell<bool>>)>,
    owner_ambiguity: SmallVec<[Rc<Cell<bool>>; 4]>,
}

thread_local! {
    static RETAINED_IDENTITY_SCOPES: RefCell<Vec<RetainedIdentityScope>> = RefCell::new(Vec::new());
}

impl Window {
    /// Acquire a globally unique identifier for the given ElementId.
    /// Only valid for the duration of the provided closure.
    pub fn with_global_id<R>(
        &mut self,
        element_id: ElementId,
        f: impl FnOnce(&GlobalElementId, &mut Self) -> R,
    ) -> R {
        self.element_id_stack.push(element_id);
        let global_id = GlobalElementId(self.element_id_stack.clone());
        let result = f(&global_id, self);
        self.element_id_stack.pop();
        result
    }

    /// Begins one retained element identity scope during request layout.
    ///
    /// Explicit IDs remain authoritative and form a stable retained boundary: anonymous ambiguity
    /// above the ID does not poison the identified subtree because the full retained path still
    /// changes whenever the surrounding positional segment or the explicit ID changes. Anonymous
    /// elements prefer a construction source location plus a parent-local occurrence number, so
    /// inserting a different call site before them does not shift their reconciliation identity.
    /// Repeated elements produced from the same call site share an ambiguity token; once a second
    /// occurrence appears, the first occurrence and all anonymous descendants are retroactively
    /// treated as unsafe for frame-bound interaction replay. Elements without source information
    /// fall back to positional identity and are always marked ambiguous for interaction replay.
    pub(crate) fn begin_retained_element(
        &mut self,
        explicit_id: Option<ElementId>,
        source_location: Option<&'static core::panic::Location<'static>>,
    ) -> (
        ElementId,
        GlobalElementId,
        SmallVec<[Rc<Cell<bool>>; 4]>,
    ) {
        let depth = self.retained_child_slot_stack.len();
        let slot = if let Some(next_slot) = self.retained_child_slot_stack.last_mut() {
            let slot = *next_slot;
            *next_slot = next_slot.saturating_add(1);
            slot
        } else {
            0
        };

        let (segment, ambiguity) = RETAINED_IDENTITY_SCOPES.with(|scopes| {
            let mut scopes = scopes.borrow_mut();

            // Lazy containers may add a synthetic retained child scope after their own
            // request-layout scope has already been popped. Keep the identity-scope stack aligned
            // with the slot stack instead of clearing all prior scopes. A synthetic level that also
            // contributes a retained path segment (`with_retained_child_key`) is a stable boundary;
            // a slot-only child scope inherits its parent's ambiguity conservatively.
            if scopes.len() > depth {
                scopes.clear();
            }
            while scopes.len() < depth {
                let synthetic_depth = scopes.len();
                let keyed_boundary = self.retained_element_id_stack.len() > synthetic_depth;
                let owner_ambiguity = if keyed_boundary {
                    SmallVec::new()
                } else {
                    scopes
                        .last()
                        .map(|scope| scope.owner_ambiguity.clone())
                        .unwrap_or_default()
                };
                scopes.push(RetainedIdentityScope {
                    source_occurrences: FxHashMap::default(),
                    owner_ambiguity,
                });
            }

            let has_explicit_id = explicit_id.is_some();
            let mut ambiguity = if has_explicit_id {
                SmallVec::new()
            } else {
                scopes
                    .last()
                    .map(|scope| scope.owner_ambiguity.clone())
                    .unwrap_or_default()
            };

            let segment = if let Some(explicit_id) = explicit_id {
                explicit_id
            } else if let Some(source_location) = source_location {
                if let Some(parent) = scopes.last_mut() {
                    let entry = parent
                        .source_occurrences
                        .entry(*source_location)
                        .or_insert_with(|| (0, Rc::new(Cell::new(false))));
                    let occurrence = entry.0;
                    entry.0 = entry.0.saturating_add(1);
                    if occurrence > 0 {
                        entry.1.set(true);
                    }
                    ambiguity.push(entry.1.clone());
                    ElementId::RetainedSourceSlot(*source_location, occurrence)
                } else {
                    // A window has a single root element, so a source-derived root identity cannot
                    // collide with a sibling in the same retained namespace.
                    ElementId::RetainedSourceSlot(*source_location, 0)
                }
            } else {
                ambiguity.push(Rc::new(Cell::new(true)));
                ElementId::InstanceSlot(slot)
            };

            scopes.push(RetainedIdentityScope {
                source_occurrences: FxHashMap::default(),
                owner_ambiguity: ambiguity.clone(),
            });
            (segment, ambiguity)
        });

        self.retained_element_id_stack.push(segment.clone());
        let retained_id = GlobalElementId(self.retained_element_id_stack.clone());
        self.retained_child_slot_stack.push(0);
        (segment, retained_id, ambiguity)
    }

    /// Ends the retained identity scope opened by [`Self::begin_retained_element`].
    pub(crate) fn end_retained_element(&mut self) {
        self.retained_child_slot_stack.pop();
        self.retained_element_id_stack.pop();
        RETAINED_IDENTITY_SCOPES.with(|scopes| {
            scopes.borrow_mut().pop();
        });
    }

    /// Restores one already-assigned retained identity segment for prepaint/paint traversal.
    pub(crate) fn with_retained_element_segment<R>(
        &mut self,
        segment: &ElementId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.retained_element_id_stack.push(segment.clone());
        let result = f(self);
        self.retained_element_id_stack.pop();
        result
    }

    /// Returns the retained rendering identity of the element currently being prepainted/painted.
    pub(crate) fn current_retained_element_id(&self) -> Option<GlobalElementId> {
        (!self.retained_element_id_stack.is_empty())
            .then(|| GlobalElementId(self.retained_element_id_stack.clone()))
    }

    /// Executes the provided function with the specified rem size.
    ///
    /// This method must only be called as part of element drawing.
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
    /// `set_cursor_style`. This method should only be called during the prepaint
    /// phase of element drawing.
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
    pub fn with_content_mask<R>(
        &mut self,
        mask: Option<ContentMask<Pixels>>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(mask) = mask {
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
        }

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

    pub(crate) fn with_scene_animation<R>(
        &mut self,
        animation_id: crate::SceneAnimationId,
        property: crate::TransitionProperty,
        paint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint();
        let previous_animation = self.scene_animation.replace((animation_id, property));
        let result = paint(self);
        self.scene_animation = previous_animation;
        result
    }

    pub(crate) fn with_sampled_scene_animation<R>(
        &mut self,
        property: crate::TransitionProperty,
        progress: f32,
        from: [f32; 4],
        to: [f32; 4],
        paint: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint();
        let animation_id = self.next_frame.scene.allocate_animation_id();
        self.next_frame
            .scene
            .push_animation_value(crate::SceneAnimationValue {
                animation_id,
                property,
                progress: if progress.is_finite() { progress } else { 0.0 },
                from,
                to,
            });
        self.with_scene_animation(animation_id, property, paint)
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

    pub(crate) fn visual_scale(&self) -> f32 {
        self.element_visual_transform.scale
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
        let result = f(self);
        if result.is_err() {
            self.truncate_prepaint_to(index);
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

    /// Immediately push an element ID onto the stack. Useful for simplifying IDs in lists
    pub fn with_id<R>(&mut self, id: impl Into<ElementId>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.with_global_id(id.into(), |_, window| f(window))
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

    /// Sets the focus handle for the current element. This handle will be used to manage focus state
    /// and keyboard event dispatch for the element.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn set_focus_handle(&mut self, focus_handle: &FocusHandle, _: &App) {
        self.invalidator.debug_assert_prepaint();
        if focus_handle.is_focused(self) {
            self.next_frame.focus = Some(focus_handle.id);
        }
        self.next_frame.dispatch_tree.set_focus_id(focus_handle.id);

        if let Some(retained_id) = self.current_retained_element_id()
            && let Some(view_id) = self.rendered_entity_stack.last().copied()
        {
            self.focus_retained_targets.insert(
                focus_handle.id,
                FocusRetainedTarget {
                    view_id,
                    retained_id,
                },
            );
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
        let should_track_segment = self.invalidator.phase() == DrawPhase::Paint;
        let prepaint_start = should_track_segment.then(|| self.prepaint_index());
        let paint_start = should_track_segment.then(|| self.paint_index());
        if let Some(paint_start) = &paint_start {
            self.view_bounds_stack.push(ViewBoundsFrame {
                scanned_until: paint_start.scene_index,
                bounds: None,
            });
        }
        let result = f(self);
        if should_track_segment {
            if let (Some(prepaint_start), Some(paint_start)) = (prepaint_start, paint_start) {
                let paint_end = self.paint_index();
                let scene_range = paint_start.scene_index..paint_end.scene_index;
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
                        .scene
                        .bounds_for_range(scanned_until..scene_range.end)
                {
                    view_bounds = Some(match view_bounds {
                        Some(bounds) => bounds.union(&tail_bounds),
                        None => tail_bounds,
                    });
                }
                // Fold this view's result into the parent frame, covering any of the
                // parent's own operations painted since its previous merge point.
                if let Some(parent_scanned_until) = self
                    .view_bounds_stack
                    .last()
                    .map(|parent| parent.scanned_until)
                {
                    let gap_bounds = if parent_scanned_until < scene_range.start {
                        self.next_frame
                            .scene
                            .bounds_for_range(parent_scanned_until..scene_range.start)
                    } else {
                        None
                    };
                    if let Some(parent) = self.view_bounds_stack.last_mut() {
                        for bounds in gap_bounds.iter().chain(view_bounds.iter()) {
                            parent.bounds = Some(match parent.bounds {
                                Some(existing) => existing.union(bounds),
                                None => *bounds,
                            });
                        }
                        parent.scanned_until = scene_range.end;
                    }
                }
                if scene_range.start != scene_range.end
                    && let Some(bounds) = view_bounds
                {
                    self.next_frame
                        .retained_scene_segments
                        .push(RetainedSceneSegment {
                            bounds,
                            scene_range,
                            paint_range: paint_start..paint_end,
                            prepaint_range: prepaint_start..self.prepaint_index(),
                            entity_id: id,
                        });
                }
            }
        }
        self.rendered_entity_stack.pop();
        result
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
