use crate::{App, Bounds, GlobalElementId, InspectorElementId, LayoutId, Pixels, Window};

/// Frame context reserved for a future explicit prepare phase.
///
/// This type is currently a structural placeholder. The active GPUI element API
/// still uses `request_layout`, `prepaint`, and `paint`.
pub struct PrepareCx<'a> {
    /// The current window.
    pub window: &'a mut Window,
    /// The current app context.
    pub app: &'a mut App,
    /// The current element global id, when available.
    pub global_id: Option<&'a GlobalElementId>,
    /// The current inspector id, when available.
    pub inspector_id: Option<&'a InspectorElementId>,
}

/// Frame context reserved for a future explicit layout phase.
///
/// This type is currently a structural placeholder. The active GPUI element API
/// still uses `request_layout`, `prepaint`, and `paint`.
pub struct LayoutCx<'a> {
    /// The current window.
    pub window: &'a mut Window,
    /// The current app context.
    pub app: &'a mut App,
    /// The current element global id, when available.
    pub global_id: Option<&'a GlobalElementId>,
    /// The current inspector id, when available.
    pub inspector_id: Option<&'a InspectorElementId>,
}

impl LayoutCx<'_> {
    /// Request a layout id from the current window.
    pub fn request_layout(
        &mut self,
        style: crate::Style,
        children: impl IntoIterator<Item = LayoutId>,
    ) -> LayoutId {
        self.window.request_layout(style, children, self.app)
    }
}

/// Frame context reserved for a future explicit prepaint phase.
///
/// This type is currently a structural placeholder. The active GPUI element API
/// still uses `request_layout`, `prepaint`, and `paint`.
pub struct PrepaintCx<'a> {
    /// The current window.
    pub window: &'a mut Window,
    /// The current app context.
    pub app: &'a mut App,
    /// The current element global id, when available.
    pub global_id: Option<&'a GlobalElementId>,
    /// The current inspector id, when available.
    pub inspector_id: Option<&'a InspectorElementId>,
    /// The current layout bounds.
    pub bounds: Bounds<Pixels>,
}

/// Frame context reserved for a future explicit paint phase.
///
/// This type is currently a structural placeholder. The active GPUI element API
/// still uses `request_layout`, `prepaint`, and `paint`.
pub struct PaintCx<'a> {
    /// The current window.
    pub window: &'a mut Window,
    /// The current app context.
    pub app: &'a mut App,
    /// The current element global id, when available.
    pub global_id: Option<&'a GlobalElementId>,
    /// The current inspector id, when available.
    pub inspector_id: Option<&'a InspectorElementId>,
    /// The current layout bounds.
    pub bounds: Bounds<Pixels>,
}
