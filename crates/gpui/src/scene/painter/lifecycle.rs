use super::*;
impl Painter {
    pub fn clear(&mut self) {
        self.scene.revision = 0;
        let primitive_count_before_clear = self.primitive_count();
        self.recent_peak_paint_operations = self
            .recent_peak_paint_operations
            .max(self.scene.paint_operations.len());
        self.recent_peak_primitives = self
            .recent_peak_primitives
            .max(primitive_count_before_clear);

        self.scene.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.scene.paths.clear();
        self.scene.shadows.clear();
        self.scene.quads.clear();
        self.scene.underlines.clear();
        self.scene.monochrome_sprites.clear();
        self.scene.polychrome_sprites.clear();
        self.scene.surfaces.clear();
        self.scene.backdrop_blurs.clear();
        self.scene.blurs.clear();
        self.scene.renderer_extensions.clear();
        self.scene.animation_values.clear();
        self.next_scene_animation_id = 0;
        self.scene.prepared_batches.clear();
        self.scene.replayed_primitives = 0;
        self.retained_prefix_invalid = false;
        self.retained_prefix_verified_len = 0;
        self.blur_captures.clear();

        if primitive_count_before_clear == 0 {
            self.idle_clear_frames = self.idle_clear_frames.saturating_add(1);
        } else {
            self.idle_clear_frames = 0;
        }

        if self.idle_clear_frames >= SCENE_IDLE_TRIM_FRAMES {
            self.trim_retained_capacity();
            self.idle_clear_frames = 0;
        }
    }

    pub fn finish(&mut self) {
        self.scene.paint_operations.finish();
        debug_assert!(
            self.blur_captures.is_empty(),
            "element blur capture must be closed before finishing a scene"
        );
        self.scene.shadows.finish();
        self.scene.shadows.compact();
        self.scene.quads.finish();
        self.scene.paths.sort_unstable_by_key(|path| path.order);
        self.scene
            .underlines
            .sort_unstable_by_key(|underline| underline.order);
        self.scene
            .monochrome_sprites
            .sort_unstable_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.scene
            .polychrome_sprites
            .sort_unstable_by_key(|sprite| (sprite.order, sprite.tile.tile_id));
        self.scene
            .surfaces
            .sort_unstable_by_key(|surface| surface.order);
        self.scene
            .backdrop_blurs
            .sort_unstable_by_key(|blur| blur.order);
        self.scene.blurs.sort_unstable_by_key(|blur| blur.order);
        self.scene
            .renderer_extensions
            .sort_unstable_by_key(|extension| extension.order);
        self.prepare_batches();
        static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
        self.scene.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn trim_for_reuse_against(&mut self, current: &Self, paint: &Scene) {
        macro_rules! trim_vec_against {
            ($field:ident) => {{
                let target = SCENE_MIN_RETAINED_CAPACITY.max(paint.$field.len());
                if self.$field.capacity()
                    > target.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
                {
                    self.$field.shrink_to(target);
                }
            }};
        }

        trim_vec_against!(paint_operations);

        trim_vec_against!(shadows);
        trim_vec_against!(quads);
        trim_vec_against!(paths);
        trim_vec_against!(underlines);
        trim_vec_against!(monochrome_sprites);
        trim_vec_against!(polychrome_sprites);
        trim_vec_against!(surfaces);
        trim_vec_against!(backdrop_blurs);
        trim_vec_against!(blurs);
        trim_vec_against!(renderer_extensions);
        trim_vec_against!(animation_values);

        trim_vec_capacity(
            &mut self.layer_stack,
            SCENE_MIN_RETAINED_CAPACITY.max(current.layer_stack.len()),
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.blur_captures,
            SCENE_MIN_RETAINED_CAPACITY.max(current.blur_captures.len()),
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );

        let batch_target = SCENE_MIN_RETAINED_CAPACITY.max(paint.prepared_batches.batches.len());
        if self.scene.prepared_batches.batches.capacity()
            > batch_target.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.scene.prepared_batches.batches.shrink_to(batch_target);
            self.scene.prepared_batches.retained_capacity =
                self.scene.prepared_batches.batches.capacity();
        }

        self.primitive_bounds.trim_for_reuse_against(
            &current.primitive_bounds,
            SCENE_MIN_RETAINED_CAPACITY,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );

        // The old frame just became scratch. Seed its high-water history from the frame that is
        // actually visible now, not from a page that may have disappeared during this transition.
        self.recent_peak_paint_operations = paint.paint_operations.len();
        self.recent_peak_primitives = paint.primitive_count();
    }

    fn trim_retained_capacity(&mut self) {
        let primitive_floor = self.recent_peak_primitives.max(SCENE_MIN_RETAINED_CAPACITY);
        let paint_floor = self
            .recent_peak_paint_operations
            .max(SCENE_MIN_RETAINED_CAPACITY);

        if self.scene.paint_operations.capacity()
            > paint_floor.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.scene.paint_operations.shrink_to(paint_floor);
        }
        if self.scene.shadows.capacity()
            > primitive_floor.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.scene.shadows.shrink_to(primitive_floor);
        }
        if self.scene.quads.capacity()
            > primitive_floor.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.scene.quads.shrink_to(primitive_floor);
        }
        trim_vec_capacity(
            &mut self.scene.paths,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.underlines,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.monochrome_sprites,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.polychrome_sprites,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.surfaces,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.backdrop_blurs,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.blurs,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.renderer_extensions,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.animation_values,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_vec_capacity(
            &mut self.scene.prepared_batches.batches,
            primitive_floor,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );

        self.scene.prepared_batches.retained_capacity =
            self.scene.prepared_batches.batches.capacity();
        self.recent_peak_paint_operations = self.scene.paint_operations.len();
        self.recent_peak_primitives = self.primitive_count();
    }

    fn prepare_batches(&mut self) {
        let mut prepared = std::mem::take(&mut self.scene.prepared_batches.batches);
        prepared.clear();
        for batch in self.batches() {
            if let PrimitiveBatch::Shadows { range, .. } = batch {
                if let Some(PreparedSceneBatch::Shadows(previous)) = prepared.last_mut()
                    && previous.end == range.start
                {
                    previous.end = range.end;
                } else {
                    prepared.push(PreparedSceneBatch::Shadows(range));
                }
                continue;
            }
            if let PrimitiveBatch::Quads { quads, range } = batch {
                let mut append_run = |range: Range<usize>, is_solid| {
                    if let Some(PreparedSceneBatch::Quads(previous)) = prepared.last_mut()
                        && previous.is_solid == is_solid
                        && previous.range.end == range.start
                    {
                        previous.range.end = range.end;
                    } else {
                        prepared.push(PreparedSceneBatch::Quads(PreparedQuadRun {
                            range,
                            is_solid,
                        }));
                    }
                };
                let mut run_start = range.start;
                let mut run_is_solid = quads.first().is_some_and(is_solid_quad);
                for (offset, quad) in quads.iter().enumerate().skip(1) {
                    let is_solid = is_solid_quad(quad);
                    if is_solid == run_is_solid {
                        continue;
                    }
                    let run_end = range.start + offset;
                    append_run(run_start..run_end, run_is_solid);
                    run_start = run_end;
                    run_is_solid = is_solid;
                }
                append_run(run_start..range.end, run_is_solid);
                continue;
            }

            prepared.push(match batch {
                PrimitiveBatch::Shadows { .. } => {
                    unreachable!("shadow segments are merged before this match")
                }
                PrimitiveBatch::Quads { .. } => {
                    unreachable!("quad batches are split before this match")
                }
                PrimitiveBatch::Paths(paths) => {
                    PreparedSceneBatch::Paths(slice_range(&self.scene.paths, paths))
                }
                PrimitiveBatch::Underlines(underlines) => {
                    PreparedSceneBatch::Underlines(slice_range(&self.scene.underlines, underlines))
                }
                PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    sampling,
                    sprites,
                } => PreparedSceneBatch::MonochromeSprites {
                    texture_id,
                    sampling,
                    range: slice_range(&self.scene.monochrome_sprites, sprites),
                },
                PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    sprites,
                } => PreparedSceneBatch::PolychromeSprites {
                    texture_id,
                    range: slice_range(&self.scene.polychrome_sprites, sprites),
                },
                PrimitiveBatch::Surfaces(surfaces) => {
                    PreparedSceneBatch::Surfaces(slice_range(&self.scene.surfaces, surfaces))
                }
                PrimitiveBatch::BackdropBlurs(blurs) => {
                    PreparedSceneBatch::BackdropBlurs(PreparedBackdropBlurGroup {
                        range: slice_range(&self.scene.backdrop_blurs, blurs),
                    })
                }
                PrimitiveBatch::Blurs(blurs) => {
                    PreparedSceneBatch::Blurs(slice_range(&self.scene.blurs, blurs))
                }
                PrimitiveBatch::RendererExtensions(extensions) => {
                    PreparedSceneBatch::RendererExtensions(slice_range(
                        &self.scene.renderer_extensions,
                        extensions,
                    ))
                }
            });
        }

        self.scene.prepared_batches.batches = prepared;
        self.scene.prepared_batches.batch_count = self.scene.prepared_batches.batches.len();
        self.scene.prepared_batches.primitive_count = self.primitive_count();
        self.scene.prepared_batches.retained_capacity =
            self.scene.prepared_batches.batches.capacity();
    }
}
