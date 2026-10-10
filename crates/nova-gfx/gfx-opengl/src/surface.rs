use crate::{
    device::{OpenGlDevice, native, native_extent},
    resources::format,
};
use gfx_core::*;
use glow::HasContext as _;
use glutin::{
    prelude::*,
    surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface},
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::num::NonZeroU32;

pub(crate) struct Swapchain {
    pub(crate) window: Surface<WindowSurface>,
    pub(crate) color: glow::NativeTexture,
    pub(crate) framebuffer: glow::NativeFramebuffer,
    pub(crate) config: SurfaceConfig,
    source: SurfaceId,
}
impl SurfaceDevice for OpenGlDevice {
    type SurfaceTarget = dyn HasWindowHandle;
    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        _desc: &SurfaceDescriptor,
    ) -> Result<SurfaceId> {
        let handle = target.window_handle().map_err(native)?.as_raw();
        if !matches!(
            handle,
            RawWindowHandle::Win32(_)
                | RawWindowHandle::Xlib(_)
                | RawWindowHandle::Xcb(_)
                | RawWindowHandle::Wayland(_)
        ) {
            return Err(Error::InvalidInput(
                "unsupported OpenGL window handle".into(),
            ));
        }
        Ok(self.surfaces.insert(handle))
    }
    fn create_swapchain(
        &mut self,
        source: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        native_extent(config.size)?;
        if config.format == Format::Depth32Float
            || !matches!(
                config.alpha_mode,
                CompositeAlphaMode::Auto
                    | CompositeAlphaMode::Opaque
                    | CompositeAlphaMode::Premultiplied
            )
        {
            return Err(Error::Unavailable(
                "OpenGL supports opaque or premultiplied color windows".into(),
            ));
        }
        let handle = *self.surfaces.get(source)?;
        let attributes = SurfaceAttributesBuilder::<WindowSurface>::new()
            .with_srgb(Some(config.format.is_srgb()))
            .build(
                handle,
                NonZeroU32::new(config.size.width())
                    .ok_or_else(|| Error::InvalidInput("zero surface width".into()))?,
                NonZeroU32::new(config.size.height())
                    .ok_or_else(|| Error::InvalidInput("zero surface height".into()))?,
            );
        // SAFETY: caller retains the native window, which shares the device display/format.
        let window = unsafe {
            self.display
                .create_window_surface(&self.config, &attributes)
        }
        .map_err(native)?;
        self.make_window_current(&window)?;
        window
            .set_swap_interval(
                &self.context,
                if config.present_mode == PresentMode::Fifo {
                    SwapInterval::Wait(NonZeroU32::MIN)
                } else {
                    SwapInterval::DontWait
                },
            )
            .map_err(native)?;
        let (color, framebuffer) = self.color_target(config)?;
        self.park()?;
        Ok(self.swapchains.insert(Swapchain {
            window,
            color,
            framebuffer,
            config,
            source,
        }))
    }
    fn destroy_swapchain(&mut self, id: SwapchainId) -> Result<()> {
        self.park()?;
        let chain = self.swapchains.take(id)?;
        unsafe {
            self.gl.delete_texture(chain.color);
            self.gl.delete_framebuffer(chain.framebuffer);
        }
        self.check()
    }
    fn destroy_surface(&mut self, id: SurfaceId) -> Result<()> {
        if self.swapchains.values().any(|chain| chain.source == id) {
            return Err(Error::InvalidInput(
                "OpenGL surface still owns a swapchain".into(),
            ));
        }
        self.surfaces.take(id)?;
        Ok(())
    }
}
impl OpenGlDevice {
    fn color_target(
        &self,
        config: SurfaceConfig,
    ) -> Result<(glow::NativeTexture, glow::NativeFramebuffer)> {
        let (width, height) = native_extent(config.size)?;
        self.park()?;
        // SAFETY: create and validate an owned offscreen target. Its row zero is logical
        // top, matching every CPU upload/filter/extension; presentation performs a Y flip.
        unsafe {
            let color = self.gl.create_texture().map_err(native)?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(color));
            self.gl
                .tex_storage_2d(glow::TEXTURE_2D, 1, format(config.format).0, width, height);
            let framebuffer = match self.gl.create_framebuffer() {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    self.gl.delete_texture(color);
                    return Err(native(error));
                }
            };
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(color),
                0,
            );
            let valid =
                self.gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if !valid {
                self.gl.delete_texture(color);
                self.gl.delete_framebuffer(framebuffer);
                return Err(Error::Backend("OpenGL window target incomplete".into()));
            }
            if let Err(error) = self.check() {
                self.gl.delete_texture(color);
                self.gl.delete_framebuffer(framebuffer);
                return Err(error);
            }
            Ok((color, framebuffer))
        }
    }
    /// Reallocates the latest drawable extent while retaining the native context/window.
    /// # Errors
    /// Returns stale-handle or native allocation errors; old GPU targets stay live on error.
    pub fn resize_swapchain(&mut self, id: SwapchainId, width: u32, height: u32) -> Result<()> {
        let size = Extent2d::new(width, height)?;
        let config = SurfaceConfig {
            size,
            ..self.swapchains.get(id)?.config
        };
        let (color, framebuffer) = self.color_target(config)?;
        let chain = self.swapchains.get_mut(id)?;
        chain.window.resize(
            &self.context,
            NonZeroU32::new(width).ok_or_else(|| Error::InvalidInput("zero width".into()))?,
            NonZeroU32::new(height).ok_or_else(|| Error::InvalidInput("zero height".into()))?,
        );
        unsafe {
            self.gl.delete_texture(chain.color);
            self.gl.delete_framebuffer(chain.framebuffer);
        }
        chain.color = color;
        chain.framebuffer = framebuffer;
        chain.config = config;
        self.check()
    }
    /// Applies extent, presentation and alpha policy without replacing the native window.
    /// # Errors
    /// Returns invalid formats/alpha modes or allocation/native interval errors.
    pub fn recreate_swapchain(
        &mut self,
        id: SwapchainId,
        config: SurfaceConfig,
    ) -> Result<SwapchainId> {
        if config.format != self.swapchains.get(id)?.config.format
            || !matches!(
                config.alpha_mode,
                CompositeAlphaMode::Auto
                    | CompositeAlphaMode::Opaque
                    | CompositeAlphaMode::Premultiplied
            )
        {
            return Err(Error::Unavailable(
                "OpenGL native window format cannot change".into(),
            ));
        }
        self.resize_swapchain(id, config.size.width(), config.size.height())?;
        // Bind the native window while only an immutable swapchain borrow is held.
        // Acquiring get_mut first would overlap with make_window_current(&self).
        self.make_window_current(&self.swapchains.get(id)?.window)?;
        let chain = self.swapchains.get_mut(id)?;
        chain
            .window
            .set_swap_interval(
                &self.context,
                if config.present_mode == PresentMode::Fifo {
                    SwapInterval::Wait(NonZeroU32::MIN)
                } else {
                    SwapInterval::DontWait
                },
            )
            .map_err(native)?;
        chain.config = config;
        self.park()?;
        Ok(id)
    }
    pub(crate) fn present(
        &mut self,
        id: SwapchainId,
        pass: RenderPassId,
        steps: RenderStepList<'_>,
        color: ClearColor,
        depth: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId> {
        self.render_target(
            RenderTarget::Swapchain {
                swapchain: id,
                image_index: 0,
            },
            pass,
            steps,
            LoadOp::Clear(color),
            depth,
        )?;
        let chain = self.swapchains.get(id)?;
        self.make_window_current(&chain.window)?;
        let width = chain.config.size.width() as i32;
        let height = chain.config.size.height() as i32;
        // SAFETY: both source and native backbuffer belong to this current context. A
        // nearest blit flips only orientation; no resolution/quality downgrade occurs.
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.disable(glow::FRAMEBUFFER_SRGB);
            self.gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(chain.framebuffer));
            self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
            self.gl.blit_framebuffer(
                0,
                0,
                width,
                height,
                0,
                height,
                width,
                0,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.check()?;
        chain.window.swap_buffers(&self.context).map_err(native)?;
        self.park()?;
        self.signal()
    }
}
