use std::{
    any::{Any, TypeId},
    fmt,
    sync::Arc,
    time::Instant,
};

use crate::{Bounds, ContentMask, ScaledPixels, SceneAnimationId};

use super::DrawOrder;

pub use super::extension_device::ExtensionDevice;
#[path = "extension_pass.rs"]
mod extension_pass;
pub use extension_pass::RendererExtensionPass;
pub use gfx_core::{MemoryTrimLevel, RenderCommand, RenderPassId, ScissorRect};

/// Frame-local information passed to a Nova renderer extension.
#[derive(Clone, Debug)]
pub struct RendererExtensionContext {
    backend_kind: gfx_core::BackendKind,
    render_pass: RenderPassId,
    color_format: gfx_core::Format,
    viewport: gfx_core::Extent2d,
    bounds: Bounds<ScaledPixels>,
    content_mask: ContentMask<ScaledPixels>,
    scissor: ScissorRect,
    frame_time: Instant,
}

impl RendererExtensionContext {
    /// Describes a texture target for renderer extensions outside a window surface.
    /// No swapchain, presentation or submission ownership is transferred.
    #[must_use]
    pub fn for_target(
        backend_kind: gfx_core::BackendKind,
        render_pass: RenderPassId,
        color_format: gfx_core::Format,
        viewport: gfx_core::Extent2d,
    ) -> Self {
        let bounds = Bounds {
            origin: crate::point(ScaledPixels(0.), ScaledPixels(0.)),
            size: crate::size(
                ScaledPixels(viewport.width() as f32),
                ScaledPixels(viewport.height() as f32),
            ),
        };
        Self {
            backend_kind,
            render_pass,
            color_format,
            viewport,
            bounds,
            content_mask: ContentMask::new(bounds),
            scissor: ScissorRect {
                x: 0,
                y: 0,
                width: viewport.width(),
                height: viewport.height(),
            },
            frame_time: Instant::now(),
        }
    }
    /// Creates local target metadata while preserving the host frame time and backend.
    /// The caller owns only offscreen attachments, never a window surface.
    #[must_use]
    pub fn offscreen(
        &self,
        render_pass: RenderPassId,
        color_format: gfx_core::Format,
        viewport: gfx_core::Extent2d,
    ) -> Self {
        let bounds = Bounds {
            origin: crate::point(ScaledPixels(0.), ScaledPixels(0.)),
            size: crate::size(
                ScaledPixels(viewport.width() as f32),
                ScaledPixels(viewport.height() as f32),
            ),
        };
        Self {
            render_pass,
            color_format,
            viewport,
            bounds,
            content_mask: ContentMask::new(bounds),
            scissor: ScissorRect {
                x: 0,
                y: 0,
                width: viewport.width(),
                height: viewport.height(),
            },
            ..self.clone()
        }
    }
    /// Replaces only the render target attachment for a 3D sidecar pass.
    /// Keep the original element bounds, content mask, scissor and frame time,
    /// unlike `offscreen` which describes a new full-extent target.
    #[must_use]
    pub fn offscreen_preserving_bounds(
        &self,
        render_pass: RenderPassId,
        color_format: gfx_core::Format,
    ) -> Self {
        Self {
            render_pass,
            color_format,
            ..self.clone()
        }
    }

    /// Create a local coordinate system for a cropped 3D producer texture.
    /// Preserve the SceneView element's original projection dimensions while
    /// subtracting the producer's pixel-space origin from all clip-space
    /// inputs. No window or native queue ownership crosses this boundary.
    #[must_use]
    pub fn offscreen_region(
        &self,
        render_pass: RenderPassId,
        color_format: gfx_core::Format,
        origin: (u32, u32),
        viewport: gfx_core::Extent2d,
    ) -> Self {
        let translation = crate::point(
            ScaledPixels(origin.0 as f32),
            ScaledPixels(origin.1 as f32),
        );
        let mut bounds = self.bounds;
        bounds.origin.x -= translation.x;
        bounds.origin.y -= translation.y;
        let mut content_mask = self.content_mask;
        content_mask.bounds.origin.x -= translation.x;
        content_mask.bounds.origin.y -= translation.y;
        Self {
            render_pass,
            color_format,
            viewport,
            bounds,
            content_mask,
            scissor: ScissorRect {
                x: 0,
                y: 0,
                width: viewport.width(),
                height: viewport.height(),
            },
            ..self.clone()
        }
    }

    pub(crate) fn new(
        backend_kind: gfx_core::BackendKind,
        render_pass: RenderPassId,
        color_format: gfx_core::Format,
        viewport: gfx_core::Extent2d,
        bounds: Bounds<ScaledPixels>,
        content_mask: ContentMask<ScaledPixels>,
        scissor: ScissorRect,
        frame_time: Instant,
    ) -> Self {
        Self {
            backend_kind,
            render_pass,
            color_format,
            viewport,
            bounds,
            content_mask,
            scissor,
            frame_time,
        }
    }

    /// Graphics API used by this Nova renderer.
    #[must_use]
    pub fn backend_kind(&self) -> gfx_core::BackendKind {
        self.backend_kind
    }

    /// Render pass owned by the host window renderer.
    #[must_use]
    pub fn render_pass(&self) -> RenderPassId {
        self.render_pass
    }

    /// Color attachment format used by the host render pass.
    #[must_use]
    pub fn color_format(&self) -> gfx_core::Format {
        self.color_format
    }

    /// Drawable extent in target pixels.
    #[must_use]
    pub fn viewport(&self) -> gfx_core::Extent2d {
        self.viewport
    }

    /// Extension element bounds in GPUI scaled pixels.
    #[must_use]
    pub fn bounds(&self) -> Bounds<ScaledPixels> {
        self.bounds
    }

    /// Content mask used by the extension element.
    #[must_use]
    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        &self.content_mask
    }

    /// Rectangular clip applied to every draw step returned by the extension.
    #[must_use]
    pub fn scissor(&self) -> ScissorRect {
        self.scissor
    }

    /// Shared visual sample time for the current renderer frame.
    #[must_use]
    pub fn frame_time(&self) -> Instant {
        self.frame_time
    }
}

/// Immutable renderer input stored in a scene and shared with the presentation owner.
///
/// GPUI creates one [`RendererExtensionRenderer`] per [`RendererExtension::renderer_type`] and
/// window's Nova renderer.
/// The instance owns that window's GPU resources; `render` receives the current immutable input
/// each frame and appends draw steps at this element's painter-order position. Implementations run
/// synchronously on the renderer owner and must not perform application callbacks or blocking I/O.
pub trait RendererExtension: Any + Send + Sync {
    /// Requests compositor-owned frames for time-dependent shading without rebuilding UI layout.
    fn is_time_dependent(&self) -> bool {
        false
    }
    /// Identifies the per-window renderer shared by this input's scene nodes.
    ///
    /// The default uses the concrete input type. Inputs overriding this to share a renderer must
    /// agree on its factory and accepted input types. The renderer is created on the GPU owner,
    /// retained while any node of this type is present, and destroyed on that same owner.
    fn renderer_type(&self) -> TypeId {
        self.type_id()
    }

    /// Creates per-window GPU state on the renderer's device.
    ///
    /// # Errors
    ///
    /// Return an error when required GPU resources or pipelines cannot be created. Clean up any
    /// resources allocated before returning an error because the host cannot receive the renderer
    /// instance to call [`RendererExtensionRenderer::destroy`].
    fn create_renderer(
        &self,
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
    ) -> crate::Result<Box<dyn RendererExtensionRenderer>>;
}

impl dyn RendererExtension {
    /// Returns this extension as its concrete input type.
    #[must_use]
    pub fn downcast_ref<T: RendererExtension>(&self) -> Option<&T> {
        let extension: &dyn Any = self;
        extension.downcast_ref()
    }
}

/// Per-window state shared by scene inputs with the same renderer type.
///
/// Creation, encoding, trimming and destruction all run on the GPU owner. This state does not
/// need to be `Send`; only the immutable [`RendererExtension`] inputs cross the UI boundary.
pub trait RendererExtensionRenderer {
    /// Confirms that this frame's offscreen commands were accepted by the GPU queue.
    /// This is submission ordering, not a resource-reclamation completion signal.
    fn render_passes_submitted(&mut self) {}
    /// Moves ordered offscreen passes to the host. They execute before this frame's UI draws.
    /// Implementations must clear their pending list here, including on subsequent empty frames.
    fn drain_render_passes(&mut self, _passes: &mut Vec<RendererExtensionPass>) {}
    /// Whether this renderer has retired suballocations awaiting a GPU completion proof.
    fn needs_gpu_completion(&self) -> bool {
        false
    }

    /// Notifies this renderer that the host has verified completion of all GPU work submitted
    /// before this callback. The host must not invoke this for an untracked or failed submission.
    /// Implementations may make previously retired suballocations available for reuse.
    fn gpu_work_completed(&mut self) {}

    /// Encodes the current extension input into ordered draw steps.
    ///
    /// The host applies the element scissor to every returned step. The `frame_time` in `context`
    /// is the same visual sample used by the rest of the GPUI scene.
    ///
    /// # Errors
    ///
    /// Return an error when updating resources or producing draw steps fails.
    fn render(
        &mut self,
        extension: &dyn RendererExtension,
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
        steps: &mut Vec<RenderCommand>,
    ) -> crate::Result<()>;

    /// Releases renderer-owned caches after a GPUI memory-trim request.
    ///
    /// The host calls this on the renderer owner for `Moderate` and `Aggressive` trims, after
    /// draining pending GPU submissions. Implementations may release GPU resources they own. This
    /// hook may be called repeatedly; rendering must recreate any released cache from the current
    /// extension input. The host does not call it for `Light` trims.
    ///
    /// # Errors
    ///
    /// Return an error when a requested cache release fails.
    fn trim_memory(
        &mut self,
        _device: &mut dyn ExtensionDevice,
        _level: MemoryTrimLevel,
    ) -> crate::Result<()> {
        Ok(())
    }

    /// Releases resources owned by this renderer instance before its device is destroyed.
    ///
    /// # Errors
    ///
    /// Return an error when one or more GPU resources cannot be destroyed.
    fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> crate::Result<()>;
}

pub(crate) type SharedRendererExtension = Arc<dyn RendererExtension>;

#[derive(Clone)]
pub(crate) struct PaintRendererExtension {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub extension: SharedRendererExtension,
    pub animation_id: Option<SceneAnimationId>,
}

impl PaintRendererExtension {
    pub(crate) fn new(
        bounds: Bounds<ScaledPixels>,
        content_mask: ContentMask<ScaledPixels>,
        extension: SharedRendererExtension,
    ) -> Self {
        Self {
            order: 0,
            bounds,
            content_mask,
            extension,
            animation_id: None,
        }
    }

    pub(crate) fn visually_eq(&self, other: &Self) -> bool {
        self.order == other.order
            && self.bounds == other.bounds
            && self.content_mask == other.content_mask
            && Arc::ptr_eq(&self.extension, &other.extension)
            && self.animation_id == other.animation_id
    }
}

impl fmt::Debug for PaintRendererExtension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PaintRendererExtension")
            .field("order", &self.order)
            .field("bounds", &self.bounds)
            .field("content_mask", &self.content_mask)
            .field("renderer_type", &self.extension.renderer_type())
            .field("animation_id", &self.animation_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct First;
    struct Second;

    macro_rules! input {
        ($type:ty) => {
            impl RendererExtension for $type {
                fn create_renderer(
                    &self,
                    _device: &mut dyn ExtensionDevice,
                    _context: RendererExtensionContext,
                ) -> crate::Result<Box<dyn RendererExtensionRenderer>> {
                    anyhow::bail!("this identity test must not create GPU resources")
                }
            }
        };
    }
    input!(First);
    input!(Second);

    #[test]
    fn cropped_sidecar_target_remaps_projection_to_local_pixels() {
        let viewport = gfx_core::Extent2d::new(1920, 1080).unwrap();
        let bounds = Bounds {
            origin: crate::point(ScaledPixels(400.0), ScaledPixels(220.0)),
            size: crate::size(ScaledPixels(500.0), ScaledPixels(300.0)),
        };
        let source = RendererExtensionContext::new(
            gfx_core::BackendKind::Vulkan,
            RenderPassId::from_parts(10, 1),
            gfx_core::Format::Rgba8Unorm,
            viewport,
            bounds,
            ContentMask::new(bounds),
            ScissorRect { x: 400, y: 220, width: 500, height: 300 },
            Instant::now(),
        );
        let local = source.offscreen_region(
            RenderPassId::from_parts(11, 1),
            source.color_format(),
            (400, 220),
            gfx_core::Extent2d::new(500, 300).unwrap(),
        );
        assert_eq!(local.bounds().origin, crate::point(ScaledPixels(0.0), ScaledPixels(0.0)));
        assert_eq!(local.bounds().size, source.bounds().size);
        assert_eq!(local.content_mask().bounds.origin, local.bounds().origin);
        assert_eq!(local.scissor(), ScissorRect { x: 0, y: 0, width: 500, height: 300 });
        assert_eq!(local.frame_time(), source.frame_time());
    }

    #[test]
    fn sidecar_target_retains_ui_projection_and_clip() {
        let bounds = Bounds {
            origin: crate::point(ScaledPixels(19.0), ScaledPixels(37.0)),
            size: crate::size(ScaledPixels(400.0), ScaledPixels(270.0)),
        };
        let mask = ContentMask::new(bounds);
        let source_pass = RenderPassId::from_parts(5, 1);
        let capture_pass = RenderPassId::from_parts(6, 1);
        let format = gfx_core::Format::Rgba8Unorm;
        let context = RendererExtensionContext::new(
            gfx_core::BackendKind::Vulkan,
            source_pass,
            format,
            gfx_core::Extent2d::new(1280, 720).unwrap(),
            bounds,
            mask,
            ScissorRect { x: 19, y: 37, width: 400, height: 270 },
            Instant::now(),
        );
        let offscreen = context.offscreen_preserving_bounds(capture_pass, format);
        assert_eq!(offscreen.render_pass(), capture_pass);
        assert_eq!(offscreen.viewport(), context.viewport());
        assert_eq!(offscreen.bounds(), context.bounds());
        assert_eq!(offscreen.scissor(), context.scissor());
        assert_eq!(offscreen.content_mask().bounds, context.content_mask().bounds);
        assert_eq!(offscreen.frame_time(), context.frame_time());
    }

    #[test]
    fn shared_inputs_use_concrete_renderer_identity() {
        let first: SharedRendererExtension = Arc::new(First);
        let second: SharedRendererExtension = Arc::new(Second);
        assert_eq!(first.renderer_type(), TypeId::of::<First>());
        assert_eq!(second.renderer_type(), TypeId::of::<Second>());
        assert_ne!(first.renderer_type(), second.renderer_type());
    }
}
