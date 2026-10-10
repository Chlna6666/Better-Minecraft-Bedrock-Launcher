use super::*;
use std::ops::Range;

mod backdrop;
mod paths;
mod primitives;
mod retained;

/// Primitives clipped to a zero-area mask are invisible on screen but can produce
/// undefined shader coverage (white garbage) in the rasterizer, so they are culled
/// before packing instead of being handed to the GPU.
fn static_quad_run_visual_bounds(
    quads: &[crate::Quad],
) -> Option<crate::Bounds<crate::ScaledPixels>> {
    let mut union: Option<crate::Bounds<crate::ScaledPixels>> = None;
    for quad in quads {
        // Translation/scale can happen later on the GPU without another
        // static encode; static AABB culling would incorrectly drop it.
        if quad.animation_id.is_some() {
            return None;
        }
        // Include the SDF/antialias guard band around static geometry.
        let visual = quad
            .bounds
            .intersect(&quad.content_mask.bounds)
            .dilate(crate::ScaledPixels(2.0));
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

fn clip_is_degenerate(mask: &crate::ContentMask<crate::ScaledPixels>) -> bool {
    mask.bounds.size.width <= crate::ScaledPixels(0.)
        || mask.bounds.size.height <= crate::ScaledPixels(0.)
}

impl FrameUpload {
    pub(in crate::platform::nova) fn encode(
        &mut self,
        scene: &crate::Scene,
        presentation_animation_values: &[crate::SceneAnimationValue],
        drawable_size: DrawableSize,
        rendering_parameters: &RenderingParameters,
        premultiplied_alpha: bool,
        backdrop_blur_quality: BackdropBlurQuality,
    ) -> FrameUploadSummary {
        self.encode_scene(
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
        let root_blur = crate::PaintBlur {
            order: 0,
            animation_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        write_paint_blur(&mut self.backdrop_blurs, &root_blur, drawable_size);
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

    fn encode_scene(
        &mut self,
        scene: &crate::Scene,
        presentation_animation_values: &[crate::SceneAnimationValue],
        drawable_size: DrawableSize,
        rendering_parameters: &RenderingParameters,
        premultiplied_alpha: bool,
        backdrop_blur_quality: BackdropBlurQuality,
        reset: bool,
    ) -> FrameUploadSummary {
        if reset {
            self.renderer_extension_frame_id = self.renderer_extension_frame_id.wrapping_add(1);
            self.globals.clear();
            self.text_raster_params.clear();
            self.quads.clear();
            self.shadows.clear();
            self.path_rasterization_vertices.clear();
            self.path_sprites.clear();
            self.mono_sprites.clear();
            self.poly_sprites.clear();
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
            let static_quad_bounds = if let PreparedSceneBatch::Quads(quad_run) = batch {
                static_quad_run_visual_bounds(&scene.quads[quad_run.range.clone()])
            } else {
                None
            };
            match batch {
                PreparedSceneBatch::Quads(quad_run) => {
                    self.encode_retained_quads(
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

                        self.element_blur_inputs.push((blur_index, blur.clone()));
                        write_paint_blur(&mut self.backdrop_blurs, blur, drawable_size);
                        self.batches
                            .push(UploadedBatch::BeginBlur { index: blur_index });
                        let child_summary = self.encode_scene(
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
            // Preserve metadata generated recursively by nested blur capture.
            // Only static quad batches receive known bounds; all other
            // batches conservatively continue through normal rendering.
            let last_batch = self.batches.len();
            self.batch_visual_bounds.resize(last_batch, None);
            if let Some(bounds) = static_quad_bounds {
                for slot in &mut self.batch_visual_bounds[first_batch..last_batch] {
                    *slot = Some(bounds);
                }
            }
        }
        if reset {
            self.prune_retained_quads(scene);
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
    fn synthetic_root_reuses_zero_filter_target_and_records_transparent_clear() {
        let mut upload = FrameUpload::default();
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
    fn retained_root_never_overruns_filter_or_quad_buffers() {
        let mut upload = FrameUpload::default();
        upload.backdrop_blurs.resize(MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES, 0);
        let mut summary = FrameUploadSummary::default();
        assert!(!upload.append_retained_root(DrawableSize { width: 8, height: 8 }, &mut summary));
        assert_eq!(summary.backdrop_blur_count, 0);
        assert!(upload.retained_root_blur.is_none());
    }
}
