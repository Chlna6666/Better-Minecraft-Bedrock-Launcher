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

// Retained allocation extent and the currently visible client extent are not
// interchangeable. The display copy must never rescale UI coordinates when
// the native WM_SIZE runs ahead of a matching GPU frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PresentCopy {
    width: i32,
    height: i32,
    source_top: i32,
}

fn present_copy(
    allocated_height: i32,
    source: (i32, i32),
    client: (i32, i32),
) -> PresentCopy {
    PresentCopy {
        width: source.0.min(client.0).max(0),
        height: source.1.min(client.1).max(0),
        // The offscreen FBO stores logical row zero at this GL Y coordinate;
        // clipping a shorter client must keep the top-left content, not shift
        // the content up by the difference in client heights.
        source_top: allocated_height - source.1,
    }
}

pub(crate) struct Swapchain {
    pub(crate) window: Surface<WindowSurface>,
    pub(crate) color: glow::NativeTexture,
    pub(crate) framebuffer: glow::NativeFramebuffer,
    pub(crate) config: SurfaceConfig,
    pub(crate) source_extent: Extent2d,
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
        let source_extent = config.size;
        self.park()?;
        Ok(self.swapchains.insert(Swapchain {
            window,
            color,
            framebuffer,
            config,
            source_extent,
            source,
        }))
    }
    fn set_swapchain_source_extent(&mut self, id: SwapchainId, extent: Extent2d) -> Result<()> {
        let chain = self.swapchains.get_mut(id)?;
        if extent.width() == 0
            || extent.height() == 0
            || extent.width() > chain.config.size.width()
            || extent.height() > chain.config.size.height()
        {
            return Err(Error::InvalidInput(
                "OpenGL presentation source extent exceeds its swapchain".into(),
            ));
        }
        chain.source_extent = extent;
        Ok(())
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
        let source_size = (
            chain.source_extent.width() as i32,
            chain.source_extent.height() as i32,
        );
        #[cfg(target_os = "windows")]
        let native_size = (
            i32::try_from(chain.window.width().unwrap_or(width as u32)).map_err(native)?,
            i32::try_from(chain.window.height().unwrap_or(height as u32)).map_err(native)?,
        );
        #[cfg(not(target_os = "windows"))]
        let native_size = (width, height);
        let copy = present_copy(height, source_size, native_size);
        // SAFETY: both framebuffers belong to the current GL context.
        // The scene has already been rendered into the retained allocation;
        // only the top-left intersection is copied 1:1. A native window that
        // grew ahead of the GPU receives a temporary clear and edge extension
        // instead of undefined black backbuffer pixels or scaled UI controls.
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.disable(glow::FRAMEBUFFER_SRGB);
            self.gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(chain.framebuffer));
            self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
            self.gl.clear_color(color.red, color.green, color.blue, color.alpha);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            if copy.width > 0 && copy.height > 0 {
                let right = native_size.0;
                let bottom = native_size.1 - copy.height;
                self.gl.blit_framebuffer(
                    0,
                    copy.source_top,
                    copy.width,
                    copy.source_top + copy.height,
                    0,
                    native_size.1,
                    copy.width,
                    bottom,
                    glow::COLOR_BUFFER_BIT,
                    glow::NEAREST,
                );
                // Extend only the last valid column/row. The main scene and
                // its text remain pixel-perfect; the edges are a temporary
                // fallback until the latest geometry generation is presented.
                if right > copy.width {
                    self.gl.blit_framebuffer(
                        copy.width - 1,
                        copy.source_top,
                        copy.width,
                        copy.source_top + copy.height,
                        copy.width,
                        native_size.1,
                        right,
                        bottom,
                        glow::COLOR_BUFFER_BIT,
                        glow::NEAREST,
                    );
                }
                if bottom > 0 {
                    self.gl.blit_framebuffer(
                        0,
                        copy.source_top + copy.height - 1,
                        copy.width,
                        copy.source_top + copy.height,
                        0,
                        bottom,
                        copy.width,
                        0,
                        glow::COLOR_BUFFER_BIT,
                        glow::NEAREST,
                    );
                    if right > copy.width {
                        self.gl.blit_framebuffer(
                            copy.width - 1,
                            copy.source_top + copy.height - 1,
                            copy.width,
                            copy.source_top + copy.height,
                            copy.width,
                            bottom,
                            right,
                            0,
                            glow::COLOR_BUFFER_BIT,
                            glow::NEAREST,
                        );
                    }
                }
            }
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.check()?;
        chain.window.swap_buffers(&self.context).map_err(native)?;
        self.park()?;
        self.signal()
    }
}

#[cfg(test)]
mod present_copy_tests {
    use super::present_copy;

    #[test]
    fn native_growth_does_not_scale_the_scene() {
        let copy = present_copy(1200, (800, 600), (1100, 900));
        assert_eq!((copy.width, copy.height), (800, 600));
        assert_eq!(copy.source_top, 600);
    }

    #[test]
    fn smaller_client_clips_from_logical_top_left_not_from_bottom() {
        let copy = present_copy(1200, (800, 600), (400, 300));
        assert_eq!((copy.width, copy.height), (400, 300));
        assert_eq!(copy.source_top, 600);
    }

    #[test]
    fn exact_extent_has_one_to_one_copy() {
        let copy = present_copy(720, (1280, 720), (1280, 720));
        assert_eq!((copy.width, copy.height, copy.source_top), (1280, 720, 0));
    }
}
