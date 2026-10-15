use super::*;
use std::{hash::Hasher, ops::Range};

mod backdrop;
mod paths;
mod primitives;
mod retained;

/// Primitives clipped to a zero-area mask are invisible on screen but can produce
/// undefined shader coverage (white garbage) in the rasterizer, so they are culled
/// before packing instead of being handed to the GPU.
fn static_raster_run_visual_bounds(
    primitives: impl IntoIterator<Item = (
        crate::Bounds<crate::ScaledPixels>,
        crate::Bounds<crate::ScaledPixels>,
        Option<crate::SceneAnimationId>,
        bool,
    )>,
) -> Option<crate::Bounds<crate::ScaledPixels>> {
    let mut union: Option<crate::Bounds<crate::ScaledPixels>> = None;
    for (bounds, mask, animation_id, has_unbounded_transform) in primitives {
        // GPU transitions and nontrivial glyph transforms can move sampling
        // outside the original AABB. Neither may produce a cullable batch.
        if animation_id.is_some() || has_unbounded_transform {
            return None;
        }
        // The guard band accounts for SDF and sprite sampling at the edges.
        let visual = bounds.intersect(&mask).dilate(crate::ScaledPixels(2.0));
        let values = [
            visual.origin.x.0, visual.origin.y.0,
            visual.size.width.0, visual.size.height.0,
        ];
        if !values.into_iter().all(f32::is_finite) {
            return None;
        }
        union = Some(match union {
            Some(previous) => previous.union(&visual),
            None => visual,
        });
    }
    union
}

#[cfg(test)]
fn static_quad_run_visual_bounds(
    quads: &[crate::Quad],
) -> Option<crate::Bounds<crate::ScaledPixels>> {
    static_raster_run_visual_bounds(
        quads.iter().map(|quad| (
            quad.bounds,
            quad.content_mask.bounds,
            quad.animation_id,
            false,
        )),
    )
}

/// A Quad's conservative visible bounds, computed as the primitive is packed.
/// Never cull GPU-motion or non-finite geometry based on its static AABB.
#[inline]
fn static_quad_visual_bounds(quad: &crate::Quad) -> Option<crate::Bounds<crate::ScaledPixels>> {
    if quad.animation_id.is_some() {
        return None;
    }
    let visual = quad
        .bounds
        .intersect(&quad.content_mask.bounds)
        .dilate(crate::ScaledPixels(2.0));
    let values = [
        visual.origin.x.0,
        visual.origin.y.0,
        visual.size.width.0,
        visual.size.height.0,
    ];
    values.into_iter().all(f32::is_finite).then_some(visual)
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
        presentation_animation_values: &[crate::SceneAnimationValue],
        drawable_size: DrawableSize,
        rendering_parameters: &RenderingParameters,
        premultiplied_alpha: bool,
        backdrop_blur_quality: BackdropBlurQuality,
    ) -> FrameUploadSummary {
        cache.begin_frame();
        self.encode_scene(
            cache,
            scene,
            presentation_animation_values,
            drawable_size,
            rendering_parameters,
            premultiplied_alpha,
            backdrop_blur_quality,
            true,
        )
    }

    /// Wraps an already-encoded frame in a zero-radius, full-viewport retained layer.
    /// The normal blur texture and composite shaders also implement scene-color retention;
    /// no backend-specific swapchain contents or image-copy extension is required.
    pub(in crate::platform::nova) fn append_retained_root(
        &mut self,
        drawable_size: DrawableSize,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        if self.retained_root_blur.is_some()
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
        write_paint_blur_fields(
            &mut self.backdrop_blurs,
            0,
            bounds,
            crate::ContentMask::new(bounds),
            crate::ScaledPixels(0.0),
            1.0,
            drawable_size,
        );
        self.batches.insert(0, UploadedBatch::BeginBlur { index: root_index });
        self.batch_visual_bounds.insert(0, None);
        self.batches.push(UploadedBatch::EndBlur { index: root_index });
        self.batches.push(UploadedBatch::CompositeBlur { index: root_index });
        self.batch_visual_bounds.extend([None, None]);
        self.retained_root_blur = Some(root_index);
        self.retained_root_clear_quad = Some(clear_index);
        summary.quad_count = summary.quad_count.saturating_add(1);
        summary.backdrop_blur_count = summary.backdrop_blur_count.saturating_add(1);
        // The original encode already built the filter configs. Add this
        // new compositor without re-encoding scene primitives.
        self.refresh_backdrop_blur_configs();
        self.rebuild_backdrop_blur_passes();
        true
    }

    /// Wrap the existing packed static prefix in a zero-radius GPU color
    /// target, without replaying or re-encoding an independent foreground Scene.
    ///
    /// The original Scene is finalized: Quad/Shadow streams are authoritative.
    /// FilterRegistry compares this source's packed content fingerprint; no
    /// Background or Foreground Scene is retained for automatic capture.
    /// Reject any unsorted or unsupported batch topology before mutation.
    pub(in crate::platform::nova) fn capture_encoded_static_prefix(
        &mut self,
        scene: &crate::Scene,
        bounds: crate::Bounds<crate::ScaledPixels>,
        content_mask: crate::ContentMask<crate::ScaledPixels>,
        first_dynamic_order: crate::DrawOrder,
        drawable_size: DrawableSize,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        if self.retained_root_blur.is_some()
            || self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES >= MAX_BACKDROP_BLURS
            || self.batches.is_empty()
        {
            return false;
        }

        // Encoder drops zero-area clips, so source array positions are NOT
        // packed instance indices. Boundaries count only visible primitives.
        // The auto-capture admission proof excludes retained chunk reuse,
        // paths, textures, and nested filters before the cut.
        let static_quads = scene
            .quads
            .iter()
            .take_while(|quad| quad.order < first_dynamic_order)
            .filter(|quad| !clip_is_degenerate(&quad.content_mask))
            .count();
        let static_shadows = scene
            .shadows
            .iter()
            .take_while(|shadow| shadow.order < first_dynamic_order)
            .filter(|shadow| !clip_is_degenerate(&shadow.content_mask))
            .count();
        if static_quads + static_shadows == 0 {
            return false;
        }

        // Find the first dynamic packed instance; a mixed Quad/Shadow batch
        // may need ONE split. All later batches must contain no static part.
        let mut boundary = None::<(usize, usize)>;
        let mut saw_static = false;
        let mut saw_dynamic = false;
        for (index, batch) in self.batches.iter().enumerate() {
            let (first, count, limit) = match *batch {
                UploadedBatch::Quads { first, count }
                | UploadedBatch::SolidQuads { first, count } => {
                    (first as usize, count as usize, static_quads)
                }
                UploadedBatch::Shadows { first, count } => {
                    (first as usize, count as usize, static_shadows)
                }
                _ => (0, 1, 0),
            };
            let static_count = limit.saturating_sub(first).min(count);
            if static_count != 0 && saw_dynamic {
                return false;
            }
            saw_static |= static_count != 0;
            if static_count < count {
                saw_dynamic = true;
                if boundary.is_none() {
                    boundary = Some((index, static_count));
                }
            }
        }
        if !saw_static || !saw_dynamic {
            return false;
        }
        let Some((mut end_index, retained_count)) = boundary else {
            return false;
        };
        // Fingerprint the actual ordered packed source, not just raw Scene
        // revision or unordered Quad/Shadow arrays. Clip-degenerate instances
        // have already been removed and no temporary flattened buffer is made.
        // Preflight all ranges before mutating batches so rejection is atomic.
        let mut fingerprint = collections::FxHasher::default();
        let mut content_bytes = 0usize;
        for (position, batch) in self.batches.iter().enumerate().take(end_index + 1) {
            let take = if position == end_index {
                retained_count
            } else {
                match *batch {
                    UploadedBatch::Quads { count, .. }
                    | UploadedBatch::SolidQuads { count, .. }
                    | UploadedBatch::Shadows { count, .. } => count as usize,
                    _ => return false,
                }
            };
            if take == 0 {
                continue;
            }
            match *batch {
                UploadedBatch::Quads { first, .. }
                | UploadedBatch::SolidQuads { first, .. } => {
                    fingerprint.write_u8(u8::from(matches!(
                        *batch,
                        UploadedBatch::SolidQuads { .. }
                    )));
                    fingerprint.write_usize(take);
                    let start = (first as usize).saturating_mul(PACKED_QUAD_BYTES);
                    let size = take.saturating_mul(PACKED_QUAD_BYTES);
                    let end = start.saturating_add(size);
                    if end > self.quads.len() || end - start != size {
                        return false;
                    }
                    for (_, bytes) in self.quads.slices(start..end) {
                        fingerprint.write(bytes);
                    }
                    content_bytes = content_bytes.saturating_add(size);
                }
                UploadedBatch::Shadows { first, .. } => {
                    fingerprint.write_u8(2);
                    fingerprint.write_usize(take);
                    let start = (first as usize).saturating_mul(PACKED_SHADOW_BYTES);
                    let size = take.saturating_mul(PACKED_SHADOW_BYTES);
                    let end = start.saturating_add(size);
                    let Some(bytes) = self.shadows.get(start..end) else {
                        return false;
                    };
                    fingerprint.write(bytes);
                    content_bytes = content_bytes.saturating_add(size);
                }
                _ => return false,
            }
        }
        if content_bytes == 0 {
            return false;
        }

        if retained_count != 0 {
            // A single prepared run may straddle the static/dynamic boundary.
            // Preserve its original instance order and exact buffer offsets.
            let old = self.batches[end_index];
            let retained_count = retained_count as u32;
            let (prefix, suffix) = match old {
                UploadedBatch::Quads { first, count } => (
                    UploadedBatch::Quads {
                        first,
                        count: retained_count,
                    },
                    UploadedBatch::Quads {
                        first: first + retained_count,
                        count: count - retained_count,
                    },
                ),
                UploadedBatch::SolidQuads { first, count } => (
                    UploadedBatch::SolidQuads {
                        first,
                        count: retained_count,
                    },
                    UploadedBatch::SolidQuads {
                        first: first + retained_count,
                        count: count - retained_count,
                    },
                ),
                UploadedBatch::Shadows { first, count } => (
                    UploadedBatch::Shadows {
                        first,
                        count: retained_count,
                    },
                    UploadedBatch::Shadows {
                        first: first + retained_count,
                        count: count - retained_count,
                    },
                ),
                _ => unreachable!("only Quad/Shadow batches can straddle the capture boundary"),
            };
            self.batches[end_index] = prefix;
            self.batches.insert(end_index + 1, suffix);
            // Bounds from a combined animated run are not independently
            // cullable. Keep the split conservative until each component has
            // a proven isolated visibility region.
            self.batch_visual_bounds[end_index] = None;
            self.batch_visual_bounds.insert(end_index + 1, None);
            end_index += 1;
        }

        let index = (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
        write_paint_blur_fields(
            &mut self.backdrop_blurs,
            0,
            bounds,
            content_mask,
            crate::ScaledPixels(0.0),
            1.0,
            drawable_size,
        );
        self.element_blur_inputs.push((
            index,
            FilterSource::AutoStatic {
                bounds,
                content_mask,
                content_hash: fingerprint.finish(),
                content_bytes,
            },
        ));
        self.batches.insert(0, UploadedBatch::BeginBlur { index });
        self.batch_visual_bounds.insert(0, None);
        // The leading BeginBlur shifts the first foreground batch by one.
        self.batches.insert(end_index + 1, UploadedBatch::EndBlur { index });
        self.batches.insert(end_index + 2, UploadedBatch::CompositeBlur { index });
        self.batch_visual_bounds.insert(end_index + 1, None);
        self.batch_visual_bounds.insert(end_index + 2, None);
        summary.backdrop_blur_count = summary.backdrop_blur_count.saturating_add(1);
        self.refresh_backdrop_blur_configs();
        self.rebuild_backdrop_blur_passes();
        true
    }

    fn encode_scene(
        &mut self,
        cache: &mut RetainedQuadChunkCache,
        scene: &crate::Scene,
        presentation_animation_values: &[crate::SceneAnimationValue],
        drawable_size: DrawableSize,
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
            self.shadows.clear();
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
            self.retained_root_blur = None;
            self.retained_root_clear_quad = None;
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
            let first_batch = self.batches.len();
            // Quad batches publish bounds during packing, including shared
            // resident chunks; no second traversal of the Scene is required.
            let static_quad_bounds = match batch {
                PreparedSceneBatch::Quads(_) => None,
                PreparedSceneBatch::MonochromeSprites { range, .. } => {
                    let sprites = &scene.monochrome_sprites[range.clone()];
                    // A transformed glyph may rotate outside its rectangle.
                    // Check the transform while unioning each glyph once,
                    // rather than scanning the whole glyph batch twice.
                    static_raster_run_visual_bounds(sprites.iter().map(|sprite| (
                        sprite.bounds,
                        sprite.content_mask.bounds,
                        sprite.animation_id,
                        sprite.transformation != crate::TransformationMatrix::unit(),
                    )))
                }
                PreparedSceneBatch::PolychromeSprites { range, .. } => {
                    static_raster_run_visual_bounds(
                        scene.polychrome_sprites[range.clone()].iter().map(|sprite| (
                            sprite.bounds,
                            sprite.content_mask.bounds,
                            sprite.animation_id,
                            false,
                        )),
                    )
                }
                PreparedSceneBatch::Underlines(range) => {
                    static_raster_run_visual_bounds(
                        scene.underlines[range.clone()].iter().map(|line| (
                            line.bounds,
                            line.content_mask.bounds,
                            line.animation_id,
                            false,
                        )),
                    )
                }
                // Shadows have kernel support extending beyond their logical
                // bounds. Paths may have geometry/clip expansion. Until both
                // provide conservative visibility regions, do not cull them.
                _ => None,
            };
            match batch {
                PreparedSceneBatch::Quads(quad_run) => {
                    self.encode_retained_quads(
                        cache,
                        scene,
                        quad_run.range.clone(),
                        quad_run.is_solid,
                        &mut summary,
                    );
                }
                PreparedSceneBatch::Shadows(range) => {
                    self.encode_shadows(&scene.shadows[range.clone()], &mut summary);
                }
                PreparedSceneBatch::MonochromeSprites {
                    texture_id, range, ..
                } => {
                    self.encode_monochrome_sprites(
                        &scene.monochrome_sprites[range.clone()],
                        *texture_id,
                        &mut summary,
                    );
                }
                PreparedSceneBatch::PolychromeSprites { texture_id, range } => {
                    self.encode_polychrome_sprites(
                        &scene.polychrome_sprites[range.clone()],
                        *texture_id,
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
                        drawable_size,
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
                        write_paint_blur(&mut self.backdrop_blurs, blur, drawable_size);
                        self.batches
                            .push(UploadedBatch::BeginBlur { index: blur_index });
                        // Recursive Quad encoding appends bounds in global batch order.
                        // Fill all preceding structural slots (including End/Composite
                        // from a sibling blur) before entering the next child.
                        self.batch_visual_bounds.resize(self.batches.len(), None);
                        let child_summary = self.encode_scene(
                            cache,
                            &blur.content,
                            &[],
                            drawable_size,
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
            // Preserve bounds emitted by the Quad writer, including child
            // blur batches. Other primitive kinds only populate conservative
            // bounds when their encoding rules prove those bounds safe.
            let last_batch = self.batches.len();
            self.batch_visual_bounds.resize(last_batch, None);
            if let Some(bounds) = static_quad_bounds {
                for slot in &mut self.batch_visual_bounds[first_batch..last_batch] {
                    *slot = Some(bounds);
                }
            }
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
        let scene = crate::Scene::default();
        let mut upload = FrameUpload::default();
        let mut cache = RetainedQuadChunkCache::default();
        let drawable = DrawableSize {
            width: 320,
            height: 240,
        };
        let mut summary = upload.encode(
            &mut cache,
            &scene,
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
            &scene,
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
            let mut scene = crate::Scene::default();
            for index in 0..100 {
                let bounds = crate::bounds(
                    crate::point(crate::ScaledPixels(index as f32), crate::ScaledPixels(0.0)),
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
                crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(20.0)),
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
                &[],
                size,
                &RenderingParameters::system(),
                false,
                BackdropBlurQuality::Full,
            );
            assert!(upload.capture_encoded_static_prefix(
                &scene,
                viewport,
                crate::ContentMask::new(viewport),
                scene.quads.last().unwrap().order,
                size,
                &mut summary,
            ));
            let FilterSource::AutoStatic { content_hash, content_bytes, .. } =
                &upload.element_blur_inputs[0].1
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
        let mut scene = crate::Scene::default();
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
            &scene,
            &[],
            size,
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        let old_quad_len = upload.quads.len();
        assert!(upload.capture_encoded_static_prefix(
            &scene,
            capture.bounds,
            capture.content_mask,
            scene.quads.last().unwrap().order,
            size,
            &mut summary,
        ));
        assert_eq!(upload.quads.len(), old_quad_len);
        assert_eq!(summary.quad_count, 101);
        assert_eq!(summary.backdrop_blur_count, 1);
        assert_eq!(upload.element_blur_inputs.len(), 1);
        assert_eq!(upload.batches.len(), 5);
        assert!(matches!(upload.batches[0], UploadedBatch::BeginBlur { index: 0 }));
        assert!(matches!(upload.batches[1], UploadedBatch::Quads { count: 100, .. }
            | UploadedBatch::SolidQuads { count: 100, .. }));
        assert!(matches!(upload.batches[2], UploadedBatch::EndBlur { index: 0 }));
        assert!(matches!(upload.batches[3], UploadedBatch::CompositeBlur { index: 0 }));
        assert!(matches!(upload.batches[4], UploadedBatch::Quads { count: 1, .. }
            | UploadedBatch::SolidQuads { count: 1, .. }));
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
        let mut scene = crate::Scene::default();
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
            &scene,
            &[],
            size,
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        let original_bytes = upload.shadows.clone();
        assert!(upload.capture_encoded_static_prefix(
            &scene,
            blur.bounds,
            blur.content_mask,
            scene.shadows.last().unwrap().order,
            size,
            &mut summary,
        ));
        assert_eq!(upload.shadows, original_bytes);
        assert_eq!(summary.shadow_count, 101);
        assert!(matches!(upload.batches[1], UploadedBatch::Shadows { count: 100, .. }));
        assert!(matches!(upload.batches[4], UploadedBatch::Shadows { count: 1, .. }));
    }

    #[test]
    fn auto_static_capture_rejects_missing_visible_foreground_without_mutation() {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(18.0), crate::ScaledPixels(18.0)),
        );
        let mut scene = crate::Scene::default();
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
            &scene,
            &[],
            size,
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        let previous_len = upload.batches.len();
        assert!(!upload.capture_encoded_static_prefix(
            &scene,
            blur.bounds,
            blur.content_mask,
            scene.quads.last().unwrap().order,
            size,
            &mut summary,
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
    fn synthetic_root_reuses_zero_filter_target_and_records_transparent_clear() {
        let mut upload = FrameUpload::default();
        let mut chunk_cache = RetainedQuadChunkCache::default();
        let size = DrawableSize { width: 640, height: 480 };
        let mut summary = FrameUploadSummary::default();
        assert!(upload.append_retained_root(size, &mut summary));
        assert_eq!(upload.retained_root_blur, Some(0));
        assert_eq!(upload.retained_root_clear_quad, Some(0));
        assert_eq!(upload.quads.len(), PACKED_QUAD_BYTES);
        assert_eq!(upload.backdrop_blurs.len(), PACKED_BACKDROP_BLUR_BYTES);
        assert_eq!(summary.quad_count, 1);
        assert_eq!(summary.backdrop_blur_count, 1);
        assert!(matches!(upload.batches[0], UploadedBatch::BeginBlur { index: 0 }));
        assert!(matches!(upload.batches[1], UploadedBatch::EndBlur { index: 0 }));
        assert!(matches!(upload.batches[2], UploadedBatch::CompositeBlur { index: 0 }));
        assert_eq!(upload.batch_visual_bounds.len(), upload.batches.len());
        assert!(upload.batch_visual_bounds.iter().all(Option::is_none));
        upload.refresh_blur_content_ranges();
        assert_eq!(upload.blur_content_ranges().len(), 1);
        assert_eq!(upload.blur_content_ranges()[0].index, 0);
        assert_eq!(upload.backdrop_blur_configs().len(), 1);
        assert_eq!(upload.backdrop_blur_configs()[0].radius(), 0.0);
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
        let mut scene = crate::Scene::default();
        let paint_quads = |scene: &mut crate::Scene, x: f32, count: usize| {
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
            start..scene.len()
        };
        let left = paint_quads(&mut scene, 10.0, 32);
        let gap = paint_quads(&mut scene, 150.0, 1);
        let right = paint_quads(&mut scene, 280.0, 32);
        assert_eq!(gap.len(), 1);
        scene.record_retained_chunk(
            crate::GlobalElementId::from_path(&["left-quad-batch".into()]),
            1,
            left,
        );
        scene.record_retained_chunk(
            crate::GlobalElementId::from_path(&["right-quad-batch".into()]),
            1,
            right,
        );
        scene.finish();
        assert_eq!(scene.prepared_retained_quad_chunks().len(), 2);

        let mut cache = RetainedQuadChunkCache::default();
        let mut upload = FrameUpload::default();
        upload.encode(
            &mut cache,
            &scene,
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
        let mut scene = crate::Scene::default();
        for i in 0..25 {
            let bounds = crate::bounds(
                crate::point(crate::ScaledPixels(i as f32 * 16.0), crate::ScaledPixels(8.0)),
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
            &scene,
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
        assert!(!upload.append_retained_root(DrawableSize { width: 8, height: 8 }, &mut summary));
        assert_eq!(summary.backdrop_blur_count, 0);
        assert!(upload.retained_root_blur.is_none());
    }
}
