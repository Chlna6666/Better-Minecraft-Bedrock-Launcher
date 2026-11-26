use super::*;

#[inline]
fn snap_measured_size_to_device_pixels(measured: Size<Pixels>, scale_factor: f32) -> Size<Pixels> {
    measured.map(|axis| {
        let device_pixels = (axis.0.max(0.0) * scale_factor).ceil();
        Pixels(device_pixels / scale_factor)
    })
}

impl Window {
    /// Add a node to the layout tree for the current frame. Takes the `Style` of the element for which
    /// layout is being requested, along with the layout ids of any children. This method is called during
    /// calls to the [`Element::request_layout`] trait method and enables any element to participate in layout.
    ///
    /// This method should only be called as part of the request_layout or prepaint phase of element drawing.
    #[must_use]
    pub fn request_layout(
        &mut self,
        style: Style,
        children: impl IntoIterator<Item = LayoutId>,
        cx: &mut App,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        cx.layout_id_buffer.clear();
        cx.layout_id_buffer.extend(children);
        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();

        let owner = self.current_stable_layout_owner();
        self.layout_engine.as_mut().unwrap().request_layout(
            owner,
            style,
            rem_size,
            scale_factor,
            &cx.layout_id_buffer,
        )
    }

    /// Add a node to the layout tree for the current frame. Instead of taking a `Style` and children,
    /// this variant takes a function that is invoked during layout so you can use arbitrary logic to
    /// determine the element's size. One place this is used internally is when measuring text.
    ///
    /// The given closure is invoked at layout time with the known dimensions and available space and
    /// returns a `Size`.
    ///
    /// This method should only be called as part of the request_layout or prepaint phase of element drawing.
    pub fn request_measured_layout<
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    >(
        &mut self,
        style: Style,
        mut measure: F,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        let owner = self.current_stable_layout_owner();
        self.layout_engine
            .as_mut()
            .unwrap()
            .request_measured_layout(
                owner,
                style,
                rem_size,
                scale_factor,
                move |known, available, window, cx| {
                    snap_measured_size_to_device_pixels(
                        measure(known, available, window, cx),
                        scale_factor,
                    )
                },
            )
    }

    /// Requests a side-effect-free measured leaf whose result depends solely
    /// on its explicit measurement inputs and Taffy's constraints.
    /// Keep this internal: callers must not hide mutable app state or paint
    /// preparation in the callback. The fingerprint includes rem and DPI.
    pub(crate) fn request_keyed_size_layout<K: std::hash::Hash, F>(
        &mut self,
        style: Style,
        key: K,
        mut measure: F,
    ) -> LayoutId
    where
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels> + 'static,
    {
        use std::hash::{Hash, Hasher};
        self.invalidator.debug_assert_prepaint();
        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        let owner = self.current_stable_layout_owner();
        let mut hasher = collections::FxHasher::default();
        std::any::type_name::<K>().hash(&mut hasher);
        key.hash(&mut hasher);
        rem_size.0.to_bits().hash(&mut hasher);
        scale_factor.to_bits().hash(&mut hasher);
        self.layout_engine.as_mut().unwrap().request_fingerprinted_layout(
            owner,
            style,
            rem_size,
            scale_factor,
            Some(hasher.finish()),
            move |known, available, window, cx| {
                snap_measured_size_to_device_pixels(
                    measure(known, available, window, cx),
                    scale_factor,
                )
            },
        )
    }

    pub(crate) fn request_default_measured_layout<
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    >(
        &mut self,
        replayable: bool,
        mut measure: F,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        let owner = self.current_stable_layout_owner();
        self.layout_engine
            .as_mut()
            .unwrap()
            .request_default_measured_layout(
                owner,
                rem_size,
                scale_factor,
                replayable,
                move |known, available, window, cx| {
                    snap_measured_size_to_device_pixels(
                        measure(known, available, window, cx),
                        scale_factor,
                    )
                },
            )
    }

    /// Requests a measured leaf whose result is a pure function of its fingerprint seed and
    /// the constraints Taffy supplies.
    ///
    /// The caller must be able to show that: equal seed and equal constraints always produce
    /// the same size. Under that proof an unchanged node keeps its Taffy cache instead of
    /// being dirtied every frame, which is what stops one wrapped label from forcing a
    /// whole-root recompute.
    pub(crate) fn request_pure_measured_layout<
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    >(
        &mut self,
        fingerprint_seed: u64,
        mut measure: F,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        let owner = self.current_stable_layout_owner();
        self.layout_engine
            .as_mut()
            .unwrap()
            .request_fingerprinted_layout(
                owner,
                Style::default(),
                rem_size,
                scale_factor,
                Some(fingerprint_seed),
                move |known, available, window, cx| {
                    snap_measured_size_to_device_pixels(
                        measure(known, available, window, cx),
                        scale_factor,
                    )
                },
            )
    }

    /// Compute the layout for the given id within the given available space.
    /// This method is called for its side effect, typically by the framework prior to painting.
    /// After calling it, you can request the bounds of the given layout node id or any descendant.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn compute_layout(
        &mut self,
        layout_id: LayoutId,
        available_space: Size<AvailableSpace>,
        cx: &mut App,
    ) {
        self.invalidator.debug_assert_prepaint();

        if self
            .last_generation_stats
            .deadline_remaining_at_layout_start_us
            .is_none()
        {
            self.last_generation_stats
                .deadline_remaining_at_layout_start_us =
                super::draw::deadline_remaining_micros(self.draw_deadline);
        }

        let mut layout_engine = self.layout_engine.take().unwrap();
        layout_engine.compute_layout(layout_id, available_space, self, cx);
        self.layout_engine = Some(layout_engine);
    }

    /// Obtain the bounds computed for the given LayoutId relative to the window. This method will usually be invoked by
    /// GPUI itself automatically in order to pass your element its `Bounds` automatically.
    ///
    /// This method should only be called as part of element drawing.
    pub fn layout_bounds(&mut self, layout_id: LayoutId) -> Bounds<Pixels> {
        self.invalidator.debug_assert_prepaint();

        let scale_factor = self.scale_factor();
        let mut bounds = self
            .layout_engine
            .as_mut()
            .unwrap()
            .layout_bounds(layout_id, scale_factor)
            .map(Into::into);
        bounds.origin += self.element_offset();
        bounds
    }

}
