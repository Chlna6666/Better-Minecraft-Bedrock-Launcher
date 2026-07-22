use super::state::ElementVisualTransform;
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

/// The result of painting images with a limit for newly uploaded image tiles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImagePaintProgress {
    /// Number of image requests emitted into the current frame scene.
    pub painted_requests: usize,
    /// Number of requests skipped because their image tile was not yet resident.
    pub deferred_requests: usize,
}

struct ImagePaintContext {
    scale_factor: f32,
    visual_transform: ElementVisualTransform,
    content_mask: ContentMask<ScaledPixels>,
    opacity: f32,
    animation_config: crate::AnimatedImageConfig,
}

fn source_crop_axis(
    source_length: i32,
    image_origin: Pixels,
    image_length: Pixels,
    visible_origin: Pixels,
    visible_length: Pixels,
) -> (i32, i32) {
    if source_length <= 0 || image_length.0 <= 0.0 {
        return (0, source_length.max(1));
    }

    let source_start = ((visible_origin.0 - image_origin.0) / image_length.0 * source_length as f32)
        .round() as i32;
    let source_end = ((visible_origin.0 + visible_length.0 - image_origin.0) / image_length.0
        * source_length as f32)
        .round() as i32;
    let source_start = source_start.clamp(0, source_length - 1);
    let source_end = source_end.clamp(source_start + 1, source_length);

    (source_start, source_end - source_start)
}

fn crop_image_tile_to_visible_bounds(
    mut tile: AtlasTile,
    image_bounds: Bounds<Pixels>,
    visible_bounds: Bounds<Pixels>,
) -> AtlasTile {
    let (source_x, source_width) = source_crop_axis(
        tile.bounds.size.width.0,
        image_bounds.origin.x,
        image_bounds.size.width,
        visible_bounds.origin.x,
        visible_bounds.size.width,
    );
    let (source_y, source_height) = source_crop_axis(
        tile.bounds.size.height.0,
        image_bounds.origin.y,
        image_bounds.size.height,
        visible_bounds.origin.y,
        visible_bounds.size.height,
    );

    tile.bounds.origin.x += DevicePixels(source_x);
    tile.bounds.origin.y += DevicePixels(source_y);
    tile.bounds.size.width = DevicePixels(source_width);
    tile.bounds.size.height = DevicePixels(source_height);
    tile
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
        let svg_bounds = self.visual_device_bounds(svg_bounds, scale_factor);
        let content_mask = self.visual_content_mask().scale(scale_factor);

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
                request.bounds,
                request.corner_radii,
                request.image,
                frame,
                request.grayscale,
            )?;
        }

        Ok(())
    }

    /// Paint images while limiting newly resident static image tiles per frame.
    ///
    /// Already resident images are always emitted. Requests that need a new atlas tile after the
    /// budget has been reached are skipped and reported to the caller, which can schedule a
    /// follow-up frame without blocking input on a large burst of texture uploads.
    pub fn paint_images_budgeted<'a>(
        &mut self,
        requests: impl IntoIterator<Item = ImagePaintRequest<'a>>,
        max_new_image_tiles: usize,
    ) -> Result<ImagePaintProgress> {
        self.invalidator.debug_assert_paint();

        let context = self.image_paint_context();
        let mut progress = ImagePaintProgress::default();
        let mut new_image_tiles = 0usize;
        for request in requests {
            let frame = request
                .image
                .frame(request.frame_index)
                .ok_or_else(|| anyhow!("invalid image frame index {}", request.frame_index))?;
            let frame_sequence = frame.sequence();
            let frame_slot = request
                .image
                .gpu_frame_slot_for_frame(frame_sequence, context.animation_config);
            let cache_key = ImagePaintTileCacheKey {
                image_id: request.image.id,
                frame_slot,
                frame_sequence,
                pixel_format: frame.pixel_format(),
            };
            let requires_new_image_tile = request.image.is_animated()
                || !self.image_paint_tile_cache.contains_key(&cache_key);
            if requires_new_image_tile && new_image_tiles >= max_new_image_tiles {
                progress.deferred_requests = progress.deferred_requests.saturating_add(1);
                continue;
            }

            self.paint_image_frame_in_context(
                &context,
                request.bounds,
                request.bounds,
                request.corner_radii,
                request.image,
                frame,
                request.grayscale,
            )?;
            progress.painted_requests = progress.painted_requests.saturating_add(1);
            if requires_new_image_tile {
                new_image_tiles = new_image_tiles.saturating_add(1);
            }
        }

        Ok(progress)
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
            bounds,
            corner_radii,
            data.as_ref(),
            frame,
            grayscale,
        )
    }

    pub(crate) fn paint_image_frame_clipped(
        &mut self,
        image_bounds: Bounds<Pixels>,
        visible_bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame: AnimatedFrame,
        grayscale: bool,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let context = self.image_paint_context();
        self.paint_image_frame_in_context(
            &context,
            image_bounds,
            visible_bounds,
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
            visual_transform: self.element_visual_transform,
            content_mask: self.visual_content_mask().scale(scale_factor),
            opacity: self.element_opacity(),
            animation_config: self.image_pipeline_config.animated,
        }
    }

    fn paint_image_frame_in_context(
        &mut self,
        context: &ImagePaintContext,
        image_bounds: Bounds<Pixels>,
        visible_bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: &RenderImage,
        frame: AnimatedFrame,
        grayscale: bool,
    ) -> Result<()> {
        let bounds = context
            .visual_transform
            .transform_bounds(visible_bounds)
            .scale(context.scale_factor);
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
        let tile = crop_image_tile_to_visible_bounds(tile, image_bounds, visible_bounds);
        let corner_radii =
            corner_radii.scale(context.scale_factor * context.visual_transform.scale);

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
        let visual_scale = self.visual_scale();
        let bounds = self.visual_bounds(bounds).scale(scale_factor);
        let content_mask = self.visual_content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintBackdropBlur {
            order: 0,
            animation_id: None,
            bounds: bounds
                .map_origin(|origin| origin.floor())
                .map_size(|size| size.ceil()),
            content_mask,
            corner_radii: corner_radii.scale(scale_factor * visual_scale),
            radius: ScaledPixels::from(f32::from(style.radius) * scale_factor * visual_scale),
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
        let bounds = self.visual_bounds(bounds).scale(scale_factor);
        let content_mask = self.visual_content_mask().scale(scale_factor);
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
        let bounds = self.visual_bounds(bounds).scale(scale_factor);
        let content_mask = self.visual_content_mask().scale(scale_factor);
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
    pub(crate) fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            self.image_paint_tile_cache.clear();
        }
        self.rendered_frame.trim_retained_capacity_for_level(level);
        self.next_frame.trim_retained_capacity_for_level(level);
        self.text_system.trim_retained_capacity_for_level(level);
        self.platform_window.trim_gpui_memory(level);
    }
}

#[cfg(test)]
mod tests {
    use super::source_crop_axis;
    use crate::px;

    #[test]
    fn source_crop_axis_selects_the_visible_center_of_a_cover_image() {
        assert_eq!(
            source_crop_axis(200, px(-25.0), px(100.0), px(0.0), px(50.0)),
            (50, 100),
        );
    }
}
