use super::state::ElementVisualTransform;
use super::*;
use crate::SceneAnimationId;

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

fn image_sprite_bounds(
    bounds: Bounds<ScaledPixels>,
    visual_transform: ElementVisualTransform,
) -> Bounds<ScaledPixels> {
    if visual_transform.scale == 1.0 && visual_transform.translation == Point::default() {
        bounds
            .map_origin(|origin| origin.floor())
            .map_size(|size| size.ceil())
    } else {
        bounds
    }
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
    fn record_external_texture_damage(&mut self, damage: Bounds<ScaledPixels>) {
        if damage.is_empty() {
            return;
        }

        // Texture bytes can change while the retained primitive itself remains byte-for-byte
        // identical. Carry that out-of-band mutation into the same spatial damage channel used by
        // retained/layout animations, and include any previous-frame backdrop output that samples
        // the changed pixels. Blur topology/config changes are handled separately by the normal
        // scene damage plan.
        let backdrop_damage = self
            .rendered_frame
            .scene
            .backdrop_blur_damage(damage)
            .collect::<SmallVec<[_; 4]>>();
        self.animation_dirty_region.push(damage);
        for bounds in backdrop_damage {
            self.animation_dirty_region.push(bounds);
        }
    }

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
        self.paint_svg_animated(bounds, path, transformation, color, None, cx)
    }

    pub(crate) fn paint_svg_animated(
        &mut self,
        bounds: Bounds<Pixels>,
        path: SharedString,
        transformation: TransformationMatrix,
        color: Hsla,
        animation_id: Option<SceneAnimationId>,
        cx: &App,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        // A zero-sized element can occur while layout is settling or while its
        // parent is hidden. There is no pixel to rasterize in that state.
        if bounds.size.is_zero() {
            return Ok(());
        }

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

        let animation_id = animation_id.or_else(|| {
            self.scene_animation_id_for(&[
                crate::TransitionProperty::Opacity,
                crate::TransitionProperty::Rotation,
                crate::TransitionProperty::Scale,
                crate::TransitionProperty::Transform,
                crate::TransitionProperty::Translation,
            ])
        });
        self.next_frame.scene.insert_primitive(MonochromeSprite {
            order: 0,
            pad: MonochromeSpriteSampling::Linear as u32,
            animation_id,
            bounds: svg_bounds,
            content_mask,
            color: color.opacity(element_opacity).into(),
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
            // Animated image slots intentionally reuse the same atlas allocation. `refresh_tile_with`
            // therefore changes GPU-visible pixels without changing the retained PolychromeSprite
            // descriptor. Scene diff alone cannot discover that mutation. Damage every previous
            // sprite sampling the refreshed allocation plus this occurrence, so a concurrent local
            // animation can never restrict Present1 to an unrelated dirty rectangle and leave the
            // rest of the window on the old image frame.
            let current_damage = image_sprite_bounds(bounds, context.visual_transform)
                .intersect(&context.content_mask.bounds);
            let previous_damage = self
                .rendered_frame
                .scene
                .polychrome_sprites
                .iter()
                .filter(|sprite| {
                    sprite.tile.texture_id == tile.texture_id && sprite.tile.tile_id == tile.tile_id
                })
                .map(|sprite| sprite.bounds.intersect(&sprite.content_mask.bounds))
                .collect::<SmallVec<[_; 4]>>();
            self.record_external_texture_damage(current_damage);
            for damage in previous_damage {
                self.record_external_texture_damage(damage);
            }
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
            animation_id: self.scene_animation_id_for(&[
                crate::TransitionProperty::Opacity,
                crate::TransitionProperty::Scale,
                crate::TransitionProperty::Transform,
                crate::TransitionProperty::Translation,
            ]),
            bounds: image_sprite_bounds(bounds, context.visual_transform),
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
        let opacity = self.element_opacity();
        self.next_frame.scene.insert_primitive(PaintBackdropBlur {
            order: 0,
            animation_id: self.scene_animation_id_for(&[
                crate::TransitionProperty::Opacity,
                crate::TransitionProperty::Scale,
                crate::TransitionProperty::Transform,
                crate::TransitionProperty::Translation,
            ]),
            bounds,
            content_mask,
            corner_radii: corner_radii.scale(scale_factor * visual_scale),
            radius: ScaledPixels::from(f32::from(style.radius) * scale_factor * visual_scale),
            downsample: style.downsample.max(1),
            levels: style.levels.clamp(1, 6),
            saturation: style.saturation.max(0.0),
            opacity,
            tint: style.tint,
            recompute_overlap: matches!(
                style.overlap_mode,
                crate::BackdropBlurOverlapMode::Recompute
            ),
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

    /// Drops this window's decoded-image lookup state and retires its atlas residency.
    ///
    /// The atlas backend defers physical deallocation until a GPU-safe point. A retained Scene can
    /// still reference the old [`AtlasTile`] after the decoded-image cache releases its strong
    /// image handle, so the next platform frame is forced through a real view rebuild before Nova
    /// applies pending atlas removals. If the image is still visible, its paint path calls
    /// `ensure_tile_with` during that CPU draw and cancels the pending retirement; if it has left
    /// the UI, the removal remains pending and is safely applied before presentation.
    pub fn drop_image(&mut self, data: Arc<RenderImage>) -> Result<()> {
        let image_id = data.id;
        let had_window_residency = self
            .animated_image_slots
            .keys()
            .any(|slot_key| slot_key.image_id == image_id)
            || self
                .image_paint_tile_cache
                .keys()
                .any(|cache_key| cache_key.image_id == image_id);

        self.animated_image_slots
            .retain(|slot_key, _| slot_key.image_id != image_id);
        self.image_paint_tile_cache
            .retain(|cache_key, _| cache_key.image_id != image_id);
        self.sprite_atlas.remove_image(image_id);
        record_image_drop(1);

        if had_window_residency {
            // Do not call `refresh()` synchronously here. Image-cache eviction can happen while an
            // element is already painting; `finish_completed_draw` would then clear the one-shot
            // force-view-cache flag before the recovery frame. Running the refresh as a frame
            // callback guarantees it is installed immediately before the next draw decision.
            self.on_next_frame(|window, _cx| {
                window.force_full_redraw.set(true);
                window.refresh();
            });
        }

        Ok(())
    }

    /// Schedules a delayed moderate memory trim after this window loses focus.
    ///
    /// Reclaiming idle image and GPU scratch state on every transient deactivation causes
    /// unnecessary churn, so the trim only runs once the window has stayed inactive for
    /// [`Self::DEACTIVATION_TRIM_DELAY`]. The moderate trim preserves images retained by the
    /// current frame so restoring a hidden window never waits for them to decode again.
    /// Becoming active again drops the pending task, which cancels it. Aggressive trims driven
    /// by system memory pressure are unaffected by this path.
    pub(crate) fn schedule_deactivation_memory_trim(&mut self) {
        let handle = self.handle;
        let mut cx = self.async_app.clone();
        let executor = cx.foreground_executor().clone();
        self.deactivation_trim_task = Some(executor.spawn(async move {
            cx.background_executor()
                .timer(Self::DEACTIVATION_TRIM_DELAY)
                .await;
            let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                window.deactivation_trim_task = None;
                if window.active.get() {
                    return;
                }
                let any_other_window_active = cx
                    .windows
                    .values()
                    .flatten()
                    .any(|other_window| other_window.active.get());
                if !any_other_window_active {
                    cx.trim_image_memory(ImageMemoryTrimLevel::Moderate);
                }
                window.trim_gpui_memory(GpuiMemoryTrimLevel::Moderate);
                // Moderate renderer/backend trimming is allowed to discard scratch or compositor
                // resources while the window is inactive. Keep the window asleep, but never let a
                // later activation present the old retained frame as though those dependencies were
                // still resident. The normal activation frame observes these flags and performs one
                // complete CPU/GPU rebuild before returning to retained replay.
                window.force_full_redraw.set(true);
                window.force_view_cache_refresh = true;
            }));
        }));
    }

    /// Delay between a window losing focus and its deferred memory trim running.
    const DEACTIVATION_TRIM_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

    /// Hints the platform renderer backing this window to release idle GPUI resources.
    pub(crate) fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        if releases_resident_image_element_bitmaps(level) {
            self.rendered_frame.release_image_element_bitmaps();
            self.next_frame.release_image_element_bitmaps();
        }
        if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            self.image_paint_tile_cache.clear();
            self.force_full_redraw.set(true);
            self.force_view_cache_refresh = true;
            self.refresh();
        }
        self.rendered_frame.trim_retained_capacity_for_level(level);
        self.next_frame.trim_retained_capacity_for_level(level);
        if let Some(layout_engine) = self.layout_engine.as_mut() {
            layout_engine.trim_retained_capacity(level);
        }
        self.text_system.trim_retained_capacity_for_level(level);
        self.platform_window.trim_gpui_memory(level);
        crate::assets::trim_global_bitmap_pool(level);
    }
}

fn releases_resident_image_element_bitmaps(level: GpuiMemoryTrimLevel) -> bool {
    matches!(level, GpuiMemoryTrimLevel::Aggressive)
}

#[cfg(test)]
mod tests {
    use super::{
        ElementVisualTransform, image_sprite_bounds, releases_resident_image_element_bitmaps,
        source_crop_axis,
    };
    use crate::{GpuiMemoryTrimLevel, ScaledPixels, bounds, point, px, size};

    #[test]
    fn transformed_image_bounds_keep_subpixel_precision() {
        let input_bounds = bounds(
            point(ScaledPixels(10.25), ScaledPixels(20.5)),
            size(ScaledPixels(31.25), ScaledPixels(41.5)),
        );

        assert_eq!(
            image_sprite_bounds(
                input_bounds,
                ElementVisualTransform::identity().then_scale(0.97, point(px(100.0), px(50.0))),
            ),
            input_bounds,
        );
    }

    #[test]
    fn untransformed_image_bounds_remain_pixel_aligned() {
        let input_bounds = bounds(
            point(ScaledPixels(10.25), ScaledPixels(20.5)),
            size(ScaledPixels(31.25), ScaledPixels(41.5)),
        );

        assert_eq!(
            image_sprite_bounds(input_bounds, ElementVisualTransform::identity()),
            bounds(
                point(ScaledPixels(10.0), ScaledPixels(20.0)),
                size(ScaledPixels(32.0), ScaledPixels(42.0)),
            ),
        );
    }

    #[test]
    fn only_aggressive_trim_releases_resident_image_element_bitmaps() {
        assert!(!releases_resident_image_element_bitmaps(
            GpuiMemoryTrimLevel::Light
        ));
        assert!(!releases_resident_image_element_bitmaps(
            GpuiMemoryTrimLevel::Moderate
        ));
        assert!(releases_resident_image_element_bitmaps(
            GpuiMemoryTrimLevel::Aggressive
        ));
    }

    #[test]
    fn source_crop_axis_selects_the_visible_center_of_a_cover_image() {
        assert_eq!(
            source_crop_axis(200, px(-25.0), px(100.0), px(0.0), px(50.0)),
            (50, 100),
        );
    }
}
