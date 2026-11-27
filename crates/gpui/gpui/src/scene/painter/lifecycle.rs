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

        if self.frame_builder {
            self.scene.shadows.release_draw_aliases();
            self.scene.quads.release_draw_aliases();
            self.scene.monochrome_sprites.release_draw_aliases();
            self.scene.polychrome_sprites.release_draw_aliases();
        }
        self.scene.paint_operations.clear();
        self.draw_payload_pending = false;
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
        self.scene.capture_groups.clear();
        self.scene.renderer_extensions.clear();
        self.scene.animation_values.clear();
        self.next_scene_animation_id = 0;
        self.scene.prepared_batches.clear();
        self.scene.replayed_primitives = 0;
        self.retained_prefix_invalid = false;
        self.retained_prefix_verified_len = 0;
        self.blur_captures.clear();
        self.scroll_capture = None;

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
        self.finish_with_previous(None);
    }

    pub(crate) fn finish_against(&mut self, previous: &Scene) -> bool {
        self.finish_with_previous(Some(previous)).0
    }

    /// (same paint, certified placement-only change). Chunk qualification
    /// consumes the same physical proof instead of repeating Scene scans.
    pub(crate) fn finish_against_with_storage(&mut self, previous: &Scene) -> (bool, bool) {
        self.finish_with_previous(Some(previous))
    }

    fn finish_with_previous(&mut self, previous: Option<&Scene>) -> (bool, bool) {
        if self.frame_builder {
            self.flush_draw_payloads();
        } else {
            self.scene.paint_operations.publish_draw_payloads(
                &mut self.scene.shadows,
                &mut self.scene.quads,
                &mut self.scene.monochrome_sprites,
                &mut self.scene.polychrome_sprites,
                &mut self.draw_permutation,
                false,
            );
        }
        self.scene.paint_operations.finish();
        debug_assert!(
            self.blur_captures.is_empty(),
            "scene capture must be closed before finishing a scene"
        );
        self.scene.shadows.finish();
        self.scene.quads.finish();
        self.scene.paths.sort_unstable_by_key(|path| path.order);
        self.scene
            .underlines
            .sort_unstable_by_key(|underline| underline.order);
        self.scene.monochrome_sprites.finish();
        self.scene.polychrome_sprites.finish();
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
        // Prove identity before compaction can replace immutable pages.
        // A stable replay of the previously compacted allocation set has
        // exactly the same live coverage, so rebuilding maps is pure waste.
        let physical_reuse = previous.and_then(|previous| {
            if previous.revision == 0 { return None; }
            let same_operations = self.scene.paint_operations.same_physical_ranges(&previous.paint_operations);
            // Do not add draw-stream identity work for unrelated fresh frames.
            if !same_operations && !self.scene.paint_operations.has_deferred_scroll() { return None; }
            if !self.scene.shadows.same_physical_ranges(&previous.shadows)
                || !self.scene.quads.same_physical_ranges(&previous.quads)
                || !self.scene.monochrome_sprites.same_physical_ranges(&previous.monochrome_sprites)
                || !self.scene.polychrome_sprites.same_physical_ranges(&previous.polychrome_sprites)
            {
                return None;
            }
            if same_operations { Some(true) }
            else {
                self.scene.paint_operations.placement_storage_matches(&previous.paint_operations)
            }
        });
        let reuse = previous.map(|previous| {
            if let Some(same_paint) = physical_reuse {
                // Deferred markers do not contribute ordinary draw batches.
                // Layer ordering and coverage are separately certified by
                // SceneLayers when the resulting Paint is published.
                (same_paint, true)
            } else {
                self.scene.paint_operations.scene_reuse_matches(
                    &previous.paint_operations, self.scene.replayed_primitives > 0,
                )
            }
        });
        if physical_reuse.is_none() {
            self.scene.paint_operations.compact_with_draws(
                &mut self.scene.shadows,
                &mut self.scene.quads,
                &mut self.scene.monochrome_sprites,
                &mut self.scene.polychrome_sprites,
            );
        }
         if let Some((previous, (same_paint, same_batch_topology))) = previous.zip(reuse) {
            if same_paint || same_batch_topology {
                self.scene
                    .prepared_batches
                    .share_from(&previous.prepared_batches);
                self.scene.revision = if same_paint {
                    previous.revision
                } else {
                    super::super::display_list::next_scene_revision()
                };
            } else {
                self.prepare_batches();
                self.scene.revision = super::super::display_list::next_scene_revision();
            }
            (same_paint, physical_reuse == Some(false))
        } else {
            self.prepare_batches();
            self.scene.revision = super::super::display_list::next_scene_revision();
            (false, false)
        }
    }

    pub(crate) fn trim_for_reuse(&mut self, current: &Self, paint: &Scene) {
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
        trim_vec_against!(capture_groups);
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
        self.scene.prepared_batches.trim_scratch(batch_target);

        self.primitive_bounds.trim_for_reuse(
            &current.primitive_bounds,
            SCENE_MIN_RETAINED_CAPACITY,
            SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );

        // The old frame just became scratch. Seed its high-water history from the frame that is
        // actually visible now, not from a page that may have disappeared during this transition.
        self.recent_peak_paint_operations = paint.paint_operations.len();
        self.recent_peak_primitives = paint.primitive_count();
        // Avoid shrinking and regrowing the scratch during alternating
        // large/small interactive frames. Require a sustained low watermark.
        let floor = self.recent_peak_primitives.max(SCENE_MIN_RETAINED_CAPACITY);
        if self.draw_permutation.needs_trim(floor) {
            self.draw_scratch_low_frames = self.draw_scratch_low_frames.saturating_add(1);
            if self.draw_scratch_low_frames >= SCENE_IDLE_TRIM_FRAMES {
                self.draw_permutation.trim(floor);
                self.draw_scratch_low_frames = 0;
            }
        } else {
            self.draw_scratch_low_frames = 0;
        }
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
        if self.scene.monochrome_sprites.capacity()
            > primitive_floor.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.scene.monochrome_sprites.shrink_to(primitive_floor);
        }
        if self.scene.polychrome_sprites.capacity()
            > primitive_floor.saturating_mul(SCENE_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.scene.polychrome_sprites.shrink_to(primitive_floor);
        }
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
        let group_floor = SCENE_MIN_RETAINED_CAPACITY.max(self.scene.capture_groups.len());
        trim_vec_capacity(
            &mut self.scene.capture_groups,
            group_floor,
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
        self.scene.prepared_batches.trim_scratch(primitive_floor);
        self.draw_permutation.trim(primitive_floor);
        self.draw_scratch_low_frames = 0;
        self.recent_peak_paint_operations = self.scene.paint_operations.len();
        self.recent_peak_primitives = self.primitive_count();
    }

    fn prepare_batches(&mut self) {
        let mut prepared = self.scene.prepared_batches.take_scratch();
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

            if let PrimitiveBatch::MonochromeSprites { texture_id, sampling, ref range, .. } = batch {
                // A physical page boundary is not a native draw boundary.
                if let Some(PreparedSceneBatch::MonochromeSprites {
                    texture_id: previous_texture,
                    sampling: previous_sampling,
                    range: previous,
                }) = prepared.last_mut()
                    && *previous_texture == texture_id && *previous_sampling == sampling
                    && previous.end == range.start
                {
                    previous.end = range.end;
                } else {
                    prepared.push(PreparedSceneBatch::MonochromeSprites {
                        texture_id, sampling, range: range.clone(),
                    });
                }
                continue;
            }
            if let PrimitiveBatch::PolychromeSprites { texture_id, ref range, .. } = batch {
                if let Some(PreparedSceneBatch::PolychromeSprites {
                    texture_id: previous_texture, range: previous,
                }) = prepared.last_mut()
                    && *previous_texture == texture_id && previous.end == range.start
                {
                    previous.end = range.end;
                } else {
                    prepared.push(PreparedSceneBatch::PolychromeSprites {
                        texture_id, range: range.clone(),
                    });
                }
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
                PrimitiveBatch::MonochromeSprites { .. } => {
                    unreachable!("glyph segments are merged before this match")
                }
                PrimitiveBatch::PolychromeSprites { .. } => {
                    unreachable!("color sprite segments are merged before this match")
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

        self.scene
            .prepared_batches
            .finish(prepared, self.primitive_count());
    }
}
