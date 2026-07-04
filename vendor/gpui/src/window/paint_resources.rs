use super::*;

#[cfg(target_os = "macos")]
use core_video::pixel_buffer::CVPixelBuffer;

/// A single image sprite to paint as part of [`Window::paint_images`].
pub struct ImagePaintRequest<'a> {
    /// The image bounds in logical window pixels.
    pub bounds: Bounds<Pixels>,
    /// The corner radii applied to this image.
    pub corner_radii: Corners<Pixels>,
    /// The decoded image to paint.
    pub image: &'a RenderImage,
    /// The frame index within the image.
    pub frame_index: usize,
    /// Whether this image should be sampled in grayscale.
    pub grayscale: bool,
}

impl<'a> ImagePaintRequest<'a> {
    /// Creates a request for painting the first frame of an image without rounded corners.
    pub fn new(bounds: Bounds<Pixels>, image: &'a RenderImage) -> Self {
        Self {
            bounds,
            corner_radii: Corners::all(px(0.0)),
            image,
            frame_index: 0,
            grayscale: false,
        }
    }
}

struct ImagePaintContext {
    scale_factor: f32,
    content_mask: ContentMask<ScaledPixels>,
    opacity: f32,
    animation_config: crate::AnimatedImageConfig,
}

impl Window {
    /// Paint a monochrome SVG into the scene for the next frame at the current stacking context.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_svg(
        &mut self,
        bounds: Bounds<Pixels>,
        path: SharedString,
        transformation: TransformationMatrix,
        color: Hsla,
        cx: &App,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();

        let bounds = bounds.scale(scale_factor);
        let svg_bounds =
            svg_paint_bounds_for_requested_bounds(bounds.map(|value| ScaledPixels(value.0)));
        let params = RenderSvgParams {
            path,
            size: svg_raster_size_for_paint_bounds(svg_bounds),
        };

        let Some(tile) = self
            .sprite_atlas
            .ensure_tile_with(&params.clone().into(), &mut || {
                let Some((size, bytes)) = cx.svg_renderer.render(&params)? else {
                    return Ok(None);
                };
                Ok(Some((size, Cow::Owned(bytes))))
            })?
        else {
            return Ok(());
        };
        let content_mask = self.content_mask().scale(scale_factor);

        self.next_frame.scene.insert_primitive(MonochromeSprite {
            order: 0,
            pad: MonochromeSpriteSampling::Linear as u32,
            animation_id: None,
            bounds: svg_bounds,
            content_mask,
            color: color.opacity(element_opacity),
            tile,
            transformation,
        });

        Ok(())
    }

    /// Paint an image into the scene for the next frame at the current z-index.
    /// This method will panic if the frame_index is not valid
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_image(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame_index: usize,
        grayscale: bool,
    ) -> Result<()> {
        let frame = data
            .frame(frame_index)
            .ok_or_else(|| anyhow!("invalid image frame index {frame_index}"))?;
        self.paint_image_frame(bounds, corner_radii, data, frame, grayscale)
    }

    /// Paint multiple images into the scene for the next frame at the current z-index.
    ///
    /// This is equivalent to calling [`Self::paint_image`] for every request, but it reuses
    /// per-frame paint state across the batch and avoids cloning image handles in hot loops.
    ///
    /// # Errors
    ///
    /// Returns an error if any request references an invalid image frame or if the backing
    /// sprite atlas fails while resolving an image.
    pub fn paint_images<'a>(
        &mut self,
        requests: impl IntoIterator<Item = ImagePaintRequest<'a>>,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let context = self.image_paint_context();
        for request in requests {
            let frame = request
                .image
                .frame(request.frame_index)
                .ok_or_else(|| anyhow!("invalid image frame index {}", request.frame_index))?;
            self.paint_image_frame_in_context(
                &context,
                request.bounds,
                request.corner_radii,
                request.image,
                frame,
                request.grayscale,
            )?;
        }

        Ok(())
    }

    pub(crate) fn paint_image_frame(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame: AnimatedFrame,
        grayscale: bool,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let context = self.image_paint_context();
        self.paint_image_frame_in_context(
            &context,
            bounds,
            corner_radii,
            data.as_ref(),
            frame,
            grayscale,
        )
    }

    fn image_paint_context(&self) -> ImagePaintContext {
        let scale_factor = self.scale_factor();
        ImagePaintContext {
            scale_factor,
            content_mask: self.content_mask().scale(scale_factor),
            opacity: self.element_opacity(),
            animation_config: self.image_pipeline_config.animated,
        }
    }

    fn paint_image_frame_in_context(
        &mut self,
        context: &ImagePaintContext,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: &RenderImage,
        frame: AnimatedFrame,
        grayscale: bool,
    ) -> Result<()> {
        let bounds = bounds.scale(context.scale_factor);
        let frame_sequence = frame.sequence();
        let frame_slot = data.gpu_frame_slot_for_frame(frame_sequence, context.animation_config);
        let pixel_format = frame.pixel_format();
        let params = RenderImageParams {
            image_id: data.id,
            frame_slot,
            pixel_format,
        };
        let animated_slot_key = AnimatedImageSlotKey {
            image_id: data.id,
            frame_slot,
        };
        let is_animated = data.is_animated();
        let update_animated_slot = is_animated
            && self.animated_image_slots.get(&animated_slot_key).copied() != Some(frame_sequence);
        let image_tile_cache_key = ImagePaintTileCacheKey {
            image_id: data.id,
            frame_slot,
            frame_sequence,
            pixel_format,
        };

        let atlas_key = params.into();
        let mut build = || Ok(Some((frame.size(), Cow::Borrowed(frame.bytes()))));
        let tile = if !is_animated
            && let Some(tile) = self
                .image_paint_tile_cache
                .get(&image_tile_cache_key)
                .copied()
        {
            Some(tile)
        } else if update_animated_slot {
            self.sprite_atlas
                .refresh_tile_with(&atlas_key, &mut build)?
        } else {
            self.sprite_atlas.ensure_tile_with(&atlas_key, &mut build)?
        };
        let Some(tile) = tile else {
            log::warn!(
                "gpui image atlas allocation failed; skipping image for this frame and retrying: image_id={:?} frame_slot={:?} size={:?} pixel_format={:?}",
                data.id,
                frame_slot,
                frame.size(),
                frame.pixel_format()
            );
            self.invalidator.set_dirty(true);
            return Ok(());
        };
        if update_animated_slot {
            self.animated_image_slots
                .insert(animated_slot_key, frame_sequence);
        }
        if !is_animated && tile.bounds.size == frame.size() {
            self.image_paint_tile_cache
                .insert(image_tile_cache_key, tile);
        }
        let corner_radii = corner_radii.scale(context.scale_factor);

        self.next_frame.scene.insert_primitive(PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale,
            animation_id: None,
            bounds: bounds
                .map_origin(|origin| origin.floor())
                .map_size(|size| size.ceil()),
            content_mask: context.content_mask.clone(),
            corner_radii,
            tile,
            opacity: context.opacity,
        });
        Ok(())
    }

    /// Paint a GPU-backed backdrop blur over content already drawn behind `bounds`.
    ///
    /// Backends that do not yet implement a real blur may draw the optional tint only; the
    /// primitive remains in the scene so diagnostics and future backend work are consistent.
    pub fn paint_backdrop_blur(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        style: BackdropBlurStyle,
    ) {
        use crate::PaintBackdropBlur;

        self.invalidator.debug_assert_paint();

        if style.radius <= Pixels::ZERO && style.tint.is_none() {
            return;
        }

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintBackdropBlur {
            order: 0,
            animation_id: None,
            bounds: bounds
                .map_origin(|origin| origin.floor())
                .map_size(|size| size.ceil()),
            content_mask,
            corner_radii: corner_radii.scale(scale_factor),
            radius: ScaledPixels::from(f32::from(style.radius) * scale_factor),
            downsample: style.downsample.max(1),
            levels: style.levels.clamp(1, 6),
            saturation: style.saturation.max(0.0),
            tint: style.tint,
        });
    }

    /// Paint a GPU-resident 3D mesh into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_gpu_mesh_3d(
        &mut self,
        bounds: Bounds<Pixels>,
        mesh: Arc<GpuMesh3d>,
        parameters: GpuMesh3dDrawParameters,
    ) {
        use crate::PaintGpuMesh3d;

        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintGpuMesh3d {
            order: 0,
            bounds,
            content_mask,
            mesh,
            parameters,
        });
    }

    /// Paint a surface into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    #[cfg(target_os = "macos")]
    pub fn paint_surface(&mut self, bounds: Bounds<Pixels>, image_buffer: CVPixelBuffer) {
        use crate::{PaintSurface, SurfaceContent};

        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask,
            content: SurfaceContent::CoreVideo(image_buffer),
        });
    }

    /// Removes an image from the sprite atlas.
    pub fn drop_image(&mut self, data: Arc<RenderImage>) -> Result<()> {
        let animation_config = self.image_pipeline_config.animated;
        let frame_slots = if data.is_animated() {
            data.frame_count()
                .min(animation_config.max_gpu_frame_slots.max(1))
        } else {
            data.frame_count()
        };
        for frame_slot in 0..frame_slots {
            let params = RenderImageParams {
                image_id: data.id,
                frame_slot,
                pixel_format: RenderImagePixelFormat::Bgra8,
            };

            self.sprite_atlas.remove(&params.clone().into());
            let params = RenderImageParams {
                image_id: data.id,
                frame_slot,
                pixel_format: RenderImagePixelFormat::Rgba8,
            };
            self.sprite_atlas.remove(&params.into());
        }
        self.animated_image_slots
            .retain(|slot_key, _| slot_key.image_id != data.id);
        self.image_paint_tile_cache
            .retain(|cache_key, _| cache_key.image_id != data.id);
        record_image_drop(1);

        Ok(())
    }

    /// Hints the platform renderer backing this window to release idle GPUI resources.
    pub fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            self.image_paint_tile_cache.clear();
        }
        self.rendered_frame.trim_retained_capacity_for_level(level);
        self.next_frame.trim_retained_capacity_for_level(level);
        self.text_system.trim_retained_capacity_for_level(level);
    }
}
