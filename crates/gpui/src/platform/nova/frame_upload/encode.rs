use super::*;
use std::ops::Range;

mod backdrop;
mod paths;
mod primitives;
mod retained;
mod static_prefix;
pub(in crate::platform::nova) use static_prefix::StaticPrefix;

/// Reference union for tests of the per-instance bounds emitted while packing.
#[cfg(test)]
fn static_raster_run_visual_bounds(
    primitives: impl IntoIterator<
        Item = (
            crate::Bounds<crate::ScaledPixels>,
            crate::Bounds<crate::ScaledPixels>,
            Option<crate::SceneAnimationId>,
            bool,
        ),
    >,
) -> Option<crate::Bounds<crate::ScaledPixels>> {
    let mut union: Option<crate::Bounds<crate::ScaledPixels>> = None;
    for (bounds, mask, animation_id, has_unbounded_transform) in primitives {
        let visual =
            static_raster_visual_bounds(bounds, mask, animation_id, has_unbounded_transform)?;
        union = Some(match union {
            Some(previous) => previous.union(&visual),
            None => visual,
        });
    }
    union
}

/// Coverage is attached to the actual encoded draw, after clipping and atlas
/// relocation. Animated/non-finite geometry and transformed glyphs stay uncullable.
fn static_raster_visual_bounds(
    bounds: crate::Bounds<crate::ScaledPixels>,
    mask: crate::Bounds<crate::ScaledPixels>,
    animation_id: Option<crate::SceneAnimationId>,
    has_unbounded_transform: bool,
) -> Option<crate::Bounds<crate::ScaledPixels>> {
    if animation_id.is_some() || has_unbounded_transform {
        return None;
    }
    // Intersection's min/max comparisons can replace NaN coordinates with
    // finite mask edges. Reject invalid input before that normalization.
    if ![
        bounds.origin.x.0,
        bounds.origin.y.0,
        bounds.size.width.0,
        bounds.size.height.0,
        mask.origin.x.0,
        mask.origin.y.0,
        mask.size.width.0,
        mask.size.height.0,
    ]
    .into_iter()
    .all(f32::is_finite)
    {
        return None;
    }
    // Account for SDF and sprite sampling at the edges.
    let visual = bounds.intersect(&mask).dilate(crate::ScaledPixels(2.0));
    [
        visual.origin.x.0,
        visual.origin.y.0,
        visual.size.width.0,
        visual.size.height.0,
    ]
    .into_iter()
    .all(f32::is_finite)
    .then_some(visual)
}

/// Once a nonempty draw contains unknown coverage, later static instances must
/// not restore a finite bound and incorrectly cull the animated instances.
fn merge_batch_bounds(
    bounds: &mut Option<crate::Bounds<crate::ScaledPixels>>,
    next: Option<crate::Bounds<crate::ScaledPixels>>,
) {
    *bounds = bounds
        .zip(next)
        .map(|(previous, next)| previous.union(&next));
}

#[cfg(test)]
fn static_quad_run_visual_bounds(
    quads: &[crate::Quad],
) -> Option<crate::Bounds<crate::ScaledPixels>> {
    static_raster_run_visual_bounds(quads.iter().map(|quad| {
        (
            quad.bounds,
            quad.content_mask.bounds,
            quad.animation_id,
            false,
        )
    }))
}

/// A Quad's conservative visible bounds, computed as the primitive is packed.
/// Never cull GPU-motion or non-finite geometry based on its static AABB.
#[inline]
fn static_quad_visual_bounds(quad: &crate::Quad) -> Option<crate::Bounds<crate::ScaledPixels>> {
    static_raster_visual_bounds(
        quad.bounds,
        quad.content_mask.bounds,
        quad.animation_id,
        false,
    )
}

/// Extends one batch's bounds while packing. False forces the entire batch
/// uncullable; callers must not recover from a previous animated/invalid Quad.
#[inline]
fn merge_static_quad_bounds(
    bounds: &mut Option<crate::Bounds<crate::ScaledPixels>>,
    quad: &crate::Quad,
) -> bool {
    let Some(visual) = static_quad_visual_bounds(quad) else {
        return false;
    };
    *bounds = Some(match bounds.take() {
        Some(previous) => previous.union(&visual),
        None => visual,
    });
    true
}

#[cfg(test)]
/// Preserve independent static visibility bounds after packing. A single
/// animated Quad in a large prepared Scene run must not turn *all* the
/// independent static GPU batches into unbounded commands. Each encoded
/// batch corresponds to consecutive VISIBLE quads (degenerate clips were
/// skipped by the packer), including reused retained quad chunks.
fn static_packed_quad_batch_bounds(
    original_quads: &[crate::Quad],
    batches: &[UploadedBatch],
    result: &mut [Option<crate::Bounds<crate::ScaledPixels>>],
) {
    let mut encoded = original_quads
        .iter()
        .filter(|quad| !clip_is_degenerate(&quad.content_mask));
    for (batch, bounds) in batches.iter().zip(result.iter_mut()) {
        let count = match *batch {
            UploadedBatch::Quads { count, .. } | UploadedBatch::SolidQuads { count, .. } => {
                count as usize
            }
            _ => continue,
        };
        *bounds = static_raster_run_visual_bounds(encoded.by_ref().take(count).map(|quad| {
            (quad.bounds, quad.content_mask.bounds, quad.animation_id, false)
        }));
    }
}

fn clip_is_degenerate(mask: &crate::ContentMask<crate::ScaledPixels>) -> bool {
    mask.bounds.size.width <= crate::ScaledPixels(0.)
        || mask.bounds.size.height <= crate::ScaledPixels(0.)
}

impl FrameUpload {
    pub(in crate::platform::nova) fn encode(
        &mut self,
        cache: &mut RetainedQuadChunkCache,
        scene: &crate::Scene,
        retained_paint: &crate::window::retained::RetainedPaint,
        presentation_animation_values: &[crate::SceneAnimationValue],
        drawable_size: DrawableSize,
        rendering_parameters: &RenderingParameters,
        premultiplied_alpha: bool,
        backdrop_blur_quality: BackdropBlurQuality,
    ) -> FrameUploadSummary {
        self.encode_for_target_size(
            cache,
            scene,
            retained_paint,
            presentation_animation_values,
            drawable_size,
            drawable_size,
            rendering_parameters,
            premultiplied_alpha,
            backdrop_blur_quality,
        )
    }

    pub(in crate::platform::nova) fn encode_for_target_size(
        &mut self,
        cache: &mut RetainedQuadChunkCache,
        scene: &crate::Scene,
        retained_paint: &crate::window::retained::RetainedPaint,
        presentation_animation_values: &[crate::SceneAnimationValue],
        drawable_size: DrawableSize,
        target_size: DrawableSize,
        rendering_parameters: &RenderingParameters,
        premultiplied_alpha: bool,
        backdrop_blur_quality: BackdropBlurQuality,
    ) -> FrameUploadSummary {
        cache.begin_frame();
        self.reused_shadow_bytes = if scene.blurs.is_empty()
            && self
                .shadow_source
                .as_ref()
                .is_some_and(|source| source.matches(&scene.shadows))
        {
            self.shadows.len()
        } else {
            0
        };
        let summary = self.encode_scene(
            cache,
            scene,
            retained_paint,
            presentation_animation_values,
            drawable_size,
            target_size,
            rendering_parameters,
            premultiplied_alpha,
            backdrop_blur_quality,
            true,
        );
        if self.reused_shadow_bytes == 0 {
            // Packed indices equal logical indices only when every source
            // instance was encoded. Animated records may later be patched.
            self.shadow_source = if scene.blurs.is_empty()
                && scene.shadows.len() == self.shadows.len() / PACKED_SHADOW_BYTES
                && scene
                    .shadows
                    .iter()
                    .all(|shadow| shadow.animation_id.is_none())
            {
                scene.shadows.identity()
            } else {
                None
            };
        }
        summary
    }

    /// Records independent window-color raster work and its final compositor descriptor.
    /// The descriptor shares the compositor shader ABI; it creates no Filter scope,
    /// Gaussian work, Filter target or texture copy.
    pub(in crate::platform::nova) fn append_root_color(
        &mut self,
        drawable_size: DrawableSize,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        self.append_root_color_for_target_size(drawable_size, drawable_size, summary)
    }

    pub(in crate::platform::nova) fn append_root_color_for_target_size(
        &mut self,
        drawable_size: DrawableSize,
        target_size: DrawableSize,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        if self.root_color.is_some()
            || self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES >= MAX_BACKDROP_BLURS
            || self.quads.len() / PACKED_QUAD_BYTES >= MAX_QUADS
            || drawable_size.width == 0
            || drawable_size.height == 0
        {
            return false;
        }
        let root_index = (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
        let clear_index = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
        let bounds = crate::Bounds::new(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(
                crate::ScaledPixels(drawable_size.width as f32),
                crate::ScaledPixels(drawable_size.height as f32),
            ),
        );
        // Clear coverage is expanded to avoid SDF antialiasing leaving stale
        // pixels at the outermost viewport edge on partial redraws.
        let clear_bounds = bounds.dilate(crate::ScaledPixels(2.0));
        let clear_quad = crate::Quad {
            bounds: clear_bounds,
            content_mask: crate::ContentMask::new(clear_bounds),
            background: crate::Hsla::transparent_black().into(),
            ..Default::default()
        };
        self.quads.write(|bytes| write_quad(bytes, &clear_quad));
        // Root color is already GPU-resident; constructing an empty Scene
        // solely to encode its compositor descriptor wastes a separate Arc
        // allocation and confuses capture ownership with presentation data.
        write_paint_blur_fields_for_target_size(
            &mut self.backdrop_blurs,
            0,
            bounds,
            crate::ContentMask::new(bounds),
            crate::ScaledPixels(0.0),
            1.0,
            target_size,
        );
        let content_end = self.batches.len();
        self.batches.push(UploadedBatch::CompositeBlur { index: root_index });
        self.batch_visual_bounds.push(None);
        self.root_color = Some(RootColorPass { index: root_index, clear_quad: clear_index, content_end });
        summary.quad_count = summary.quad_count.saturating_add(1);
        summary.backdrop_blur_count = summary.backdrop_blur_count.saturating_add(1);
        // The original encode already built the filter configs. Add this
        // new compositor without re-encoding scene primitives.
        self.refresh_backdrop_blur_configs();
        self.rebuild_backdrop_blur_passes();
        true
    }

    fn encode_scene(
        &mut self,
        cache: &mut RetainedQuadChunkCache,
        scene: &crate::Scene,
        retained_paint: &crate::window::retained::RetainedPaint,
        presentation_animation_values: &[crate::SceneAnimationValue],
        drawable_size: DrawableSize,
        target_size: DrawableSize,
        rendering_parameters: &RenderingParameters,
        premultiplied_alpha: bool,
        backdrop_blur_quality: BackdropBlurQuality,
        reset: bool,
    ) -> FrameUploadSummary {
        if reset {
            self.encode_generation = self.encode_generation.wrapping_add(1);
            self.globals.clear();
            self.text_raster_params.clear();
            self.quads.clear();
            if self.reused_shadow_bytes == 0 {
                self.shadows.clear();
            }
            self.path_rasterization_vertices.clear();
            self.path_sprites.clear();
            self.mono_sprites.clear();
            self.poly_sprites.clear();
            self.mono_atlas_tiles.clear();
            self.poly_atlas_tiles.clear();
            self.underlines.clear();
            self.backdrop_blur_passes.clear();
            self.backdrop_blurs.clear();
            self.backdrop_blur_configs.clear();
            self.element_blur_inputs.clear();
            self.root_color = None;
            #[cfg(test)]
            self.animation_bindings.clear();
            self.animation_values.clear();
            self.animated_primitives.clear();
            self.sampled_animation_values.clear();
            self.gpu_indexed_source_animation_ids.clear();
            self.gpu_indexed_composite_animation_ids.clear();
            self.gpu_indexed_composite_element_blur_animation_ids
                .clear();
            self.renderer_extensions.clear();
            for steps in &mut self.renderer_extension_steps {
                steps.clear();
            }
            self.batches.clear();
            self.batch_visual_bounds.clear();
            self.resident_quad_spans.clear();
            self.globals.reserve(GLOBAL_UPLOAD_BYTES);
            self.text_raster_params.reserve(TEXT_RASTER_UPLOAD_BYTES);
            self.path_rasterization_vertices
                .reserve(PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
            self.path_sprites.reserve(PACKED_PATH_SPRITE_BYTES);
            self.backdrop_blur_passes.reserve(BACKDROP_BLUR_PASS_BYTES);
            self.backdrop_blurs.reserve(PACKED_BACKDROP_BLUR_BYTES);
            #[cfg(test)]
            self.animation_bindings
                .reserve(PACKED_ANIMATION_BINDING_BYTES);
            self.animation_values.reserve(PACKED_ANIMATION_VALUE_BYTES);
            write_f32_vec(&mut self.globals, drawable_size.width as f32);
            write_f32_vec(&mut self.globals, drawable_size.height as f32);
            write_u32_vec(&mut self.globals, u32::from(premultiplied_alpha));
            write_u32_vec(&mut self.globals, 0);
            // Offsets 16..24 are updated again immediately before every GPU submission, including
            // retained framebuffer-only presents. Keep the packed static snapshot ABI-complete so
            // a full static upload can never overwrite the live clock with a shorter buffer.
            write_f32_vec(&mut self.globals, 0.0);
            write_u32_vec(&mut self.globals, 0);
            debug_assert_eq!(self.globals.len(), GLOBAL_UPLOAD_BYTES);
            for value in rendering_parameters.gamma_ratios {
                write_f32_vec(&mut self.text_raster_params, value);
            }
            write_f32_vec(
                &mut self.text_raster_params,
                rendering_parameters.grayscale_enhanced_contrast,
            );
            write_f32_vec(
                &mut self.text_raster_params,
                rendering_parameters.subpixel_enhanced_contrast,
            );
            write_u32_vec(
                &mut self.text_raster_params,
                u32::from(rendering_parameters.is_bgr),
            );
            write_u32_vec(&mut self.text_raster_params, 0);
        }

        let mut summary = FrameUploadSummary::default();
        for value in scene
            .animation_values
            .iter()
            .chain(presentation_animation_values)
        {
            write_scene_animation_value(self, &mut summary, value);
        }

        for batch in scene.prepared_batches() {
            match batch {
                PreparedSceneBatch::Quads(quad_run) => {
                    self.encode_retained_quads(
                        cache,
                        scene,
                        retained_paint.chunks_for(scene),
                        quad_run.range.clone(),
                        quad_run.is_solid,
                        &mut summary,
                    );
                }
                PreparedSceneBatch::Shadows(range) => {
                    if self.reused_shadow_bytes > 0 {
                        self.batches.push(UploadedBatch::Shadows {
                            first: range.start as u32,
                            count: range.len() as u32,
                        });
                        summary.shadow_count += range.len() as u32;
                    } else {
                        self.encode_shadows(
                            scene.shadows.slices(range.clone()).flatten(),
                            &mut summary,
                        );
                    }
                }
                PreparedSceneBatch::MonochromeSprites { range, .. } => {
                    self.encode_monochrome_sprites(
                        &scene.monochrome_sprites[range.clone()],
                        &mut summary,
                    );
                }
                PreparedSceneBatch::PolychromeSprites { range, .. } => {
                    self.encode_polychrome_sprites(
                        &scene.polychrome_sprites[range.clone()],
                        &mut summary,
                    );
                }
                PreparedSceneBatch::Underlines(range) => {
                    self.encode_underlines(&scene.underlines[range.clone()], &mut summary);
                }
                PreparedSceneBatch::Paths(range) => {
                    self.encode_paths(&scene.paths[range.clone()], &mut summary);
                }
                PreparedSceneBatch::Surfaces(_) => {
                    summary.unsupported_batches.surfaces =
                        summary.unsupported_batches.surfaces.saturating_add(1);
                }
                PreparedSceneBatch::BackdropBlurs(group) => {
                    self.encode_backdrop_blurs(
                        &scene.backdrop_blurs[group.range.clone()],
                        target_size,
                        backdrop_blur_quality,
                        &mut summary,
                    );
                }
                PreparedSceneBatch::Blurs(range) => {
                    for blur in &scene.blurs[range.clone()] {
                        let blur_index =
                            (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
                        if self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES
                            >= MAX_BACKDROP_BLURS
                        {
                            summary.unsupported_batches.backdrop_blurs =
                                summary.unsupported_batches.backdrop_blurs.saturating_add(1);
                            continue;
                        }

                        self.element_blur_inputs.push((blur_index, FilterSource::Element(blur.clone())));
                        write_paint_blur_for_target_size(
                            &mut self.backdrop_blurs,
                            blur,
                            target_size,
                        );
                        self.batches
                            .push(UploadedBatch::BeginBlur { index: blur_index });
                        // Recursive Quad encoding appends bounds in global batch order.
                        // Fill all preceding structural slots (including End/Composite
                        // from a sibling blur) before entering the next child.
                        self.batch_visual_bounds.resize(self.batches.len(), None);
                        let child_summary = self.encode_scene(
                            cache,
                            &blur.content, retained_paint,
                            &[],
                            drawable_size,
                            target_size,
                            rendering_parameters,
                            premultiplied_alpha,
                            backdrop_blur_quality,
                            false,
                        );
                        summary.accumulate(child_summary);
                        self.batches
                            .push(UploadedBatch::EndBlur { index: blur_index });
                        self.batches
                            .push(UploadedBatch::CompositeBlur { index: blur_index });
                        summary.backdrop_blur_count = summary.backdrop_blur_count.saturating_add(1);
                    }
                }
                PreparedSceneBatch::RendererExtensions(range) => {
                    let first = self.renderer_extensions.len() as u32;
                    let extensions = &scene.renderer_extensions[range.clone()];
                    self.renderer_extensions.extend(extensions.iter().cloned());
                    self.batches.push(UploadedBatch::RendererExtensions {
                        first,
                        count: extensions.len() as u32,
                    });
                }
            }
            // Writers maintain bounds for the final draws, including atlas
            // splits/merges and child captures, without a second Scene scan.
            // Unknown geometry and structural/filter slots remain uncullable.
            let last_batch = self.batches.len();
            self.batch_visual_bounds.resize(last_batch, None);
            debug_assert_eq!(
                self.batch_visual_bounds.len(),
                self.batches.len(),
                "encoded batch bounds must match the global DrawStep batch order",
            );
        }
        if reset {
            cache.prune_retained_quads();
            self.refresh_backdrop_blur_configs();
            self.rebuild_backdrop_blur_passes();
        }
        summary
    }
}

fn register_scene_animated_primitive(
    upload: &mut FrameUpload,
    summary: &mut FrameUploadSummary,
    primitive: Option<crate::Primitive>,
    primitive_kind: AnimatedPrimitiveKind,
    primitive_index: u32,
) {
    let Some(primitive) = primitive else {
        return;
    };
    if upload.animated_primitives.len() >= MAX_ANIMATION_VALUES {
        return;
    }
    #[cfg(test)]
    {
        let animation_id = primitive
            .animation_id()
            .expect("animated primitive registration requires ownership");
        write_animation_binding(
            &mut upload.animation_bindings,
            animation_id,
            primitive_kind,
            primitive_index,
        );
    }
    summary.animation_binding_count = summary.animation_binding_count.saturating_add(1);
    upload.animated_primitives.push(AnimatedUpload::new(
        primitive,
        primitive_kind,
        primitive_index,
    ));
}

fn write_scene_animation_value(
    upload: &mut FrameUpload,
    summary: &mut FrameUploadSummary,
    value: &crate::SceneAnimationValue,
) {
    let Some(property) = AnimationProperty::from_transition_property(value.property) else {
        return;
    };
    if upload.animation_values.len() / PACKED_ANIMATION_VALUE_BYTES >= MAX_ANIMATION_VALUES {
        return;
    }
    write_animation_value(
        &mut upload.animation_values,
        value.animation_id,
        property,
        value.progress,
        value.from,
        value.to,
    );
    summary.animation_value_count = summary.animation_value_count.saturating_add(1);
    upload.sampled_animation_values.push(*value);
}

#[cfg(test)]
mod retained_root_tests {
    use super::*;

    #[test]
    fn scene_encode_generation_advances_only_when_repacking_the_scene() {
        let scene = crate::scene::Painter::default();
        let mut upload = FrameUpload::default();
        let mut cache = RetainedQuadChunkCache::default();
        let drawable = DrawableSize {
            width: 320,
            height: 240,
        };
        let mut summary = upload.encode(
            &mut cache,
            &scene, &crate::window::retained::RetainedPaint::default(),
            &[],
            drawable,
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        assert_eq!(upload.encode_generation, 1);
        // Animation-only refresh must not invalidate the DrawStep descriptors.
        upload.refresh_retained_animation_values(&scene, &[], &mut summary);
        assert_eq!(upload.encode_generation, 1);
        upload.encode(
            &mut cache,
            &scene, &crate::window::retained::RetainedPaint::default(),
            &[],
            drawable,
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        assert_eq!(upload.encode_generation, 2);
    }

    #[test]
    fn auto_static_fingerprint_tracks_source_pixels_not_foreground_animation() {
        fn encoded_source(static_width: f32, foreground_width: f32) -> (u64, usize) {
            let mut scene = crate::scene::Painter::default();
            for index in 0..100 {
                let bounds = crate::bounds(
                    crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
                    crate::size(
                        crate::ScaledPixels(if index == 50 { static_width } else { 12.0 }),
                        crate::ScaledPixels(12.0),
                    ),
                );
                scene.insert_primitive(crate::Quad {
                    bounds,
                    content_mask: crate::ContentMask::new(bounds),
                    ..Default::default()
                });
            }
            let animated_bounds = crate::bounds(
                crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
                crate::size(crate::ScaledPixels(foreground_width), crate::ScaledPixels(12.0)),
            );
            scene.insert_primitive(crate::Quad {
                bounds: animated_bounds,
                content_mask: crate::ContentMask::new(animated_bounds),
                animation_id: Some(crate::SceneAnimationId(99)),
                ..Default::default()
            });
            scene.finish();
            let size = DrawableSize {
                width: 640,
                height: 480,
            };
            let viewport = crate::bounds(
                crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
                crate::size(crate::ScaledPixels(640.0), crate::ScaledPixels(480.0)),
            );
            let mut chunk_cache = RetainedQuadChunkCache::default();
            let mut upload = FrameUpload::default();
            let mut summary = upload.encode(
                &mut chunk_cache,
                &scene,
                &crate::window::retained::RetainedPaint::default(),
                &[],
                size,
                &RenderingParameters::system(),
                false,
                BackdropBlurQuality::Full,
            );
            assert!(upload.capture_encoded_static_prefix(
                &StaticPrefix {
                    bounds: viewport,
                    content_mask: crate::ContentMask::new(viewport),
                    quads: 100,
                    shadows: 0
                },
                size,
                &mut summary
            ));
            let FilterSource::AutoStatic {
                content_hash,
                content_bytes,
                ..
            } = &upload.element_blur_inputs[0].1
            else {
                panic!("automatic capture must not allocate an Element Scene");
            };
            (*content_hash, *content_bytes)
        }

        let initial = encoded_source(12.0, 20.0);
        assert_eq!(initial, encoded_source(12.0, 24.0));
        assert_ne!(initial, encoded_source(18.0, 20.0));
    }

    #[test]
    fn auto_static_capture_splits_packed_quads_without_reencoding_foreground() {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(20.0), crate::ScaledPixels(20.0)),
        );
        let mut scene = crate::scene::Painter::default();
        for _ in 0..100 {
            scene.insert_primitive(crate::Quad {
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                ..Default::default()
            });
        }
        let mut animated = crate::Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            ..Default::default()
        };
        animated.animation_id = Some(crate::SceneAnimationId(17));
        scene.insert_primitive(animated);
        scene.finish();
        let size = DrawableSize {
            width: 640,
            height: 480,
        };
        let capture = crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        let mut chunk_cache = RetainedQuadChunkCache::default();
        let mut upload = FrameUpload::default();
        let mut summary = upload.encode(
            &mut chunk_cache,
            &scene, &crate::window::retained::RetainedPaint::default(),
            &[],
            size,
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        let old_quad_len = upload.quads.len();
        assert!(upload.capture_encoded_static_prefix(
            &StaticPrefix {
                bounds: capture.bounds,
                content_mask: capture.content_mask,
                quads: 100,
                shadows: 0
            },
            size,
            &mut summary
        ));
        assert_eq!(upload.quads.len(), old_quad_len);
        assert_eq!(summary.quad_count, 101);
        assert_eq!(summary.backdrop_blur_count, 1);
        assert_eq!(upload.element_blur_inputs.len(), 1);
        assert_eq!(upload.batches.len(), 5);
        assert!(matches!(
            upload.batches[0],
            UploadedBatch::BeginBlur { index: 0 }
        ));
        assert!(matches!(
            upload.batches[1],
            UploadedBatch::Quads { count: 100, .. } | UploadedBatch::SolidQuads { count: 100, .. }
        ));
        assert!(matches!(
            upload.batches[2],
            UploadedBatch::EndBlur { index: 0 }
        ));
        assert!(matches!(
            upload.batches[3],
            UploadedBatch::CompositeBlur { index: 0 }
        ));
        assert!(matches!(
            upload.batches[4],
            UploadedBatch::Quads { count: 1, .. } | UploadedBatch::SolidQuads { count: 1, .. }
        ));
        assert_eq!(upload.batches.len(), upload.batch_visual_bounds.len());
        upload.refresh_blur_content_ranges();
        assert_eq!(upload.blur_content_ranges().len(), 1);
        assert_eq!(upload.blur_content_ranges()[0].content_start, 1);
        assert_eq!(upload.blur_content_ranges()[0].content_end, 2);
    }

    #[test]
    fn auto_static_capture_splits_mixed_shadow_batch_at_exact_instance_boundary() {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(24.0), crate::ScaledPixels(24.0)),
        );
        let mut scene = crate::scene::Painter::default();
        for _ in 0..100 {
            scene.insert_primitive(crate::Shadow {
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                ..Default::default()
            });
        }
        let mut animated = crate::Shadow {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            ..Default::default()
        };
        animated.animation_id = Some(crate::SceneAnimationId(77));
        scene.insert_primitive(animated);
        scene.finish();
        let size = DrawableSize {
            width: 320,
            height: 240,
        };
        let blur = crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        let mut upload = FrameUpload::default();
        let mut chunk_cache = RetainedQuadChunkCache::default();
        let mut summary = upload.encode(
            &mut chunk_cache,
            &scene, &crate::window::retained::RetainedPaint::default(),
            &[],
            size,
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        let original_bytes = upload.shadows.clone();
        assert!(upload.capture_encoded_static_prefix(
            &StaticPrefix {
                bounds: blur.bounds,
                content_mask: blur.content_mask,
                quads: 0,
                shadows: 100
            },
            size,
            &mut summary
        ));
        assert_eq!(upload.shadows, original_bytes);
        assert_eq!(summary.shadow_count, 101);
        assert!(matches!(
            upload.batches[1],
            UploadedBatch::Shadows { count: 100, .. }
        ));
        assert!(matches!(
            upload.batches[4],
            UploadedBatch::Shadows { count: 1, .. }
        ));
    }

    #[test]
    fn auto_static_capture_rejects_missing_visible_foreground_without_mutation() {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(18.0), crate::ScaledPixels(18.0)),
        );
        let mut scene = crate::scene::Painter::default();
        for _ in 0..100 {
            scene.insert_primitive(crate::Quad {
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                ..Default::default()
            });
        }
        let mut animated = crate::Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            ..Default::default()
        };
        animated.animation_id = Some(crate::SceneAnimationId(3));
        animated.content_mask.bounds.size.width = crate::ScaledPixels(0.0);
        scene.insert_primitive(animated);
        scene.finish();
        let size = DrawableSize {
            width: 320,
            height: 240,
        };
        let blur = crate::PaintBlur {
            order: 0,
            animation_id: None,
            layer_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        let mut upload = FrameUpload::default();
        let mut chunk_cache = RetainedQuadChunkCache::default();
        let mut summary = upload.encode(
            &mut chunk_cache,
            &scene, &crate::window::retained::RetainedPaint::default(),
            &[],
            size,
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        let previous_len = upload.batches.len();
        assert!(!upload.capture_encoded_static_prefix(
            &StaticPrefix {
                bounds: blur.bounds,
                content_mask: blur.content_mask,
                quads: 100,
                shadows: 0
            },
            size,
            &mut summary
        ));
        assert_eq!(upload.batches.len(), previous_len);
        assert!(upload.element_blur_inputs.is_empty());
        assert_eq!(summary.backdrop_blur_count, 0);
    }

    #[test]
    fn static_batch_bounds_never_cull_gpu_animated_geometry() {
        let area = crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
            crate::size(crate::ScaledPixels(50.0), crate::ScaledPixels(30.0)),
        );
        let mut quad = crate::Quad {
            bounds: area,
            content_mask: crate::ContentMask::new(area),
            ..Default::default()
        };
        let bounds = static_quad_run_visual_bounds(&[quad.clone()])
            .expect("non-animated batch has conservative bounds");
        assert!(bounds.origin.x <= area.origin.x);
        assert!(bounds.origin.y <= area.origin.y);
        assert!(bounds.right() >= area.right());
        assert!(bounds.bottom() >= area.bottom());
        quad.animation_id = Some(crate::SceneAnimationId(42));
        assert!(static_quad_run_visual_bounds(&[quad]).is_none());
    }

    #[test]
    fn static_sprite_batches_conservatively_union_visible_content() {
        let bounds_a = crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(10.0)),
            crate::size(crate::ScaledPixels(5.0), crate::ScaledPixels(5.0)),
        );
        let bounds_b = crate::bounds(
            crate::point(crate::ScaledPixels(40.0), crate::ScaledPixels(40.0)),
            crate::size(crate::ScaledPixels(6.0), crate::ScaledPixels(7.0)),
        );
        let bounds = static_raster_run_visual_bounds([
            (bounds_a, bounds_a, None, false),
            (bounds_b, bounds_b, None, false),
        ]).expect("static sprite coverage");
        assert!(bounds.origin.x <= bounds_a.origin.x);
        assert!(bounds.right() >= bounds_b.right());
        assert!(bounds.bottom() >= bounds_b.bottom());
        assert!(static_raster_run_visual_bounds([
            (bounds_a, bounds_a, Some(crate::SceneAnimationId(1)), false),
            (bounds_b, bounds_b, None, false),
        ]).is_none());
        assert!(static_raster_run_visual_bounds([
            (bounds_a, bounds_a, None, false),
            (bounds_b, bounds_b, None, true),
        ]).is_none(), "transformed glyph batches must not be culled using untransformed AABBs");
    }

    #[test]
    fn root_color_records_composite_without_filter_scope_or_gaussian_work() {
        let mut upload = FrameUpload::default();
        let size = DrawableSize { width: 640, height: 480 };
        let mut summary = FrameUploadSummary::default();
        assert!(upload.append_root_color(size, &mut summary));
        assert_eq!(upload.root_color.map(|root| root.index), Some(0));
        assert_eq!(upload.root_color.map(|root| root.clear_quad), Some(0));
        assert_eq!(upload.quads.len(), PACKED_QUAD_BYTES);
        assert_eq!(upload.backdrop_blurs.len(), PACKED_BACKDROP_BLUR_BYTES);
        assert_eq!(summary.quad_count, 1);
        assert_eq!(summary.backdrop_blur_count, 1);
        assert_eq!(upload.batches.len(), 1);
        assert!(matches!(upload.batches[0], UploadedBatch::CompositeBlur { index: 0 }));
        assert_eq!(upload.batch_visual_bounds.len(), upload.batches.len());
        assert!(upload.batch_visual_bounds.iter().all(Option::is_none));
        upload.refresh_blur_content_ranges();
        assert!(upload.blur_content_ranges().is_empty());
        assert!(upload.isolated_blur_source_indices().is_empty());
        assert!(upload.backdrop_blur_configs().is_empty());
        assert!(upload.backdrop_blur_passes.is_empty());
        assert_eq!(upload.root_color.unwrap().content_range().content_end, 0);
    }

    #[test]
    fn animated_quad_does_not_poison_neighbor_static_batch_culling() {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(20.0), crate::ScaledPixels(15.0)),
            crate::size(crate::ScaledPixels(18.0), crate::ScaledPixels(14.0)),
        );
        let static_quad = crate::Quad {
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            ..Default::default()
        };
        let mut animated = static_quad;
        animated.animation_id = Some(crate::SceneAnimationId(1));
        let quads = [static_quad, animated, static_quad];
        let batches = [
            UploadedBatch::Quads { first: 0, count: 1 },
            UploadedBatch::Quads { first: 1, count: 1 },
            UploadedBatch::Quads { first: 2, count: 1 },
        ];
        assert!(static_quad_run_visual_bounds(&quads).is_none());
        let mut packed_bounds = [None; 3];
        static_packed_quad_batch_bounds(&quads, &batches, &mut packed_bounds);
        assert!(packed_bounds[0].is_some());
        assert!(packed_bounds[1].is_none());
        assert!(packed_bounds[2].is_some());
    }

    #[test]
    fn retained_quad_batches_receive_independent_bounds_after_single_packing_pass() {
        let mut scene = crate::window::retained::PaintFixture::default();
        let paint_quads = |scene: &mut crate::scene::Painter, x: f32, count: usize| {
            scene.push_layer(crate::bounds(
                crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
                crate::size(crate::ScaledPixels(640.0), crate::ScaledPixels(480.0)),
            ));
            let start = scene.len();
            for _ in 0..count {
                let bounds = crate::bounds(
                    crate::point(crate::ScaledPixels(x), crate::ScaledPixels(10.0)),
                    crate::size(crate::ScaledPixels(8.0), crate::ScaledPixels(8.0)),
                );
                scene.insert_primitive(crate::Quad {
                    bounds,
                    content_mask: crate::ContentMask::new(bounds),
                    ..Default::default()
                });
            }
            let end = scene.len();
            scene.pop_layer();
            start..end
        };
        let left = paint_quads(&mut scene, 10.0, 32);
        let gap = paint_quads(&mut scene, 150.0, 1);
        let right = paint_quads(&mut scene, 280.0, 32);
        assert_eq!(gap.len(), 1);
        scene.record(
            crate::GlobalElementId::from_path(&["left-quad-batch".into()]),
            1,
            left,
        );
        scene.record(
            crate::GlobalElementId::from_path(&["right-quad-batch".into()]),
            1,
            right,
        );
        scene.finish();
        assert_eq!(scene.chunks().len(), 2);

        let mut cache = RetainedQuadChunkCache::default();
        let mut upload = FrameUpload::default();
        upload.encode(
            &mut cache,
            &scene, &scene.retention,
            &[],
            DrawableSize {
                width: 640,
                height: 480,
            },
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        let quad_bounds: Vec<_> = upload
            .batches
            .iter()
            .zip(&upload.batch_visual_bounds)
            .filter_map(|(batch, bounds)| match batch {
                UploadedBatch::Quads { .. } | UploadedBatch::SolidQuads { .. } => Some(*bounds),
                _ => None,
            })
            .collect();
        assert_eq!(quad_bounds.len(), 3);
        let left = quad_bounds[0].expect("left packed chunk is static");
        let center = quad_bounds[1].expect("unretained gap is static");
        let right = quad_bounds[2].expect("right packed chunk is static");
        assert!(left.right() < center.origin.x);
        assert!(center.right() < right.origin.x);
    }

    #[test]
    fn encoded_animation_between_static_quads_keeps_independent_bounds() {
        let mut scene = crate::scene::Painter::default();
        for i in 0..25 {
            let bounds = crate::bounds(
                crate::point(
                    crate::ScaledPixels(i as f32 * 16.0),
                    crate::ScaledPixels(8.0),
                ),
                crate::size(crate::ScaledPixels(8.0), crate::ScaledPixels(8.0)),
            );
            scene.insert_primitive(crate::Quad {
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                animation_id: (i == 12).then_some(crate::SceneAnimationId(4)),
                ..Default::default()
            });
        }
        scene.finish();
        let mut upload = FrameUpload::default();
        let mut cache = RetainedQuadChunkCache::default();
        upload.encode(
            &mut cache,
            &scene, &crate::window::retained::RetainedPaint::default(),
            &[],
            DrawableSize {
                width: 640,
                height: 480,
            },
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        assert_eq!(upload.batches.len(), 3);
        assert_eq!(upload.batch_visual_bounds.len(), upload.batches.len());
        assert!(upload.batch_visual_bounds[0].is_some());
        assert!(upload.batch_visual_bounds[1].is_none());
        assert!(upload.batch_visual_bounds[2].is_some());
    }

    #[test]
    fn retained_root_never_overruns_filter_or_quad_buffers() {
        let mut upload = FrameUpload::default();
        let mut chunk_cache = RetainedQuadChunkCache::default();
        upload.backdrop_blurs.resize(MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES, 0);
        let mut summary = FrameUploadSummary::default();
        assert!(!upload.append_root_color(DrawableSize { width: 8, height: 8 }, &mut summary));
        assert_eq!(summary.backdrop_blur_count, 0);
        assert!(upload.root_color.is_none());
    }
}
