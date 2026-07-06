use super::*;

impl NovaFrameUpload {
    pub(in crate::platform::nova) fn encode(
        &mut self,
        scene: &crate::Scene,
        drawable_size: DrawableSize,
        rendering_parameters: &NovaRenderingParameters,
        premultiplied_alpha: bool,
        backdrop_blur_quality: NovaBackdropBlurQuality,
    ) -> FrameUploadSummary {
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
        self.animation_bindings.clear();
        self.animation_values.clear();
        self.custom_mesh_3d_parameters.clear();
        self.custom_mesh_3d_meshes.clear();
        self.custom_mesh_3d_shaders.clear();
        self.batches.clear();
        self.backdrop_blur_downsample = DEFAULT_BACKDROP_BLUR_DOWNSAMPLE;
        self.backdrop_blur_levels = 1;
        self.globals.reserve(GLOBAL_UPLOAD_BYTES);
        self.text_raster_params.reserve(TEXT_RASTER_UPLOAD_BYTES);
        self.path_rasterization_vertices
            .reserve(PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
        self.path_sprites.reserve(PACKED_PATH_SPRITE_BYTES);
        self.backdrop_blur_passes.reserve(BACKDROP_BLUR_PASS_BYTES);
        self.backdrop_blurs.reserve(PACKED_BACKDROP_BLUR_BYTES);
        self.animation_bindings
            .reserve(PACKED_ANIMATION_BINDING_BYTES);
        self.animation_values.reserve(PACKED_ANIMATION_VALUE_BYTES);
        self.custom_mesh_3d_parameters
            .reserve(PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES);
        write_backdrop_blur_pass(&mut self.backdrop_blur_passes, 1.0);
        write_f32_vec(&mut self.globals, drawable_size.width as f32);
        write_f32_vec(&mut self.globals, drawable_size.height as f32);
        write_u32_vec(&mut self.globals, u32::from(premultiplied_alpha));
        write_u32_vec(&mut self.globals, 0);
        for value in rendering_parameters.gamma_ratios {
            write_f32_vec(&mut self.text_raster_params, value);
        }
        write_f32_vec(
            &mut self.text_raster_params,
            rendering_parameters.grayscale_enhanced_contrast,
        );
        write_f32_vec(&mut self.text_raster_params, 0.0);
        write_f32_vec(&mut self.text_raster_params, 0.0);
        write_f32_vec(&mut self.text_raster_params, 0.0);

        let mut summary = FrameUploadSummary::default();
        for value in &scene.animation_values {
            write_scene_animation_value(self, &mut summary, value);
        }

        let mut custom_mesh_vertex_count = 0_usize;
        let mut custom_mesh_index_count = 0_usize;
        for batch in scene.prepared_batches() {
            match batch {
                PreparedSceneBatch::Quads(quad_run) => {
                    let first = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
                    let mut count = 0_u32;
                    for quad in &scene.quads[quad_run.range.clone()] {
                        if self.quads.len() / PACKED_QUAD_BYTES >= MAX_QUADS {
                            break;
                        }
                        let primitive_index = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
                        write_quad(&mut self.quads, quad);
                        write_scene_animation_binding(
                            self,
                            &mut summary,
                            quad.animation_id,
                            NovaAnimatedPrimitiveKind::Quad,
                            primitive_index,
                        );
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches.push(if quad_run.is_solid {
                            NovaUploadedBatch::SolidQuads { first, count }
                        } else {
                            NovaUploadedBatch::Quads { first, count }
                        });
                        summary.quad_count = summary.quad_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::Shadows(range) => {
                    let first = (self.shadows.len() / PACKED_SHADOW_BYTES) as u32;
                    let mut count = 0_u32;
                    for shadow in &scene.shadows[range.clone()] {
                        if self.shadows.len() / PACKED_SHADOW_BYTES >= MAX_SHADOWS {
                            break;
                        }
                        let primitive_index = (self.shadows.len() / PACKED_SHADOW_BYTES) as u32;
                        write_shadow(&mut self.shadows, shadow);
                        write_scene_animation_binding(
                            self,
                            &mut summary,
                            shadow.animation_id,
                            NovaAnimatedPrimitiveKind::Shadow,
                            primitive_index,
                        );
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::Shadows { first, count });
                        summary.shadow_count = summary.shadow_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::MonochromeSprites {
                    texture_id, range, ..
                } => {
                    let first = (self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES) as u32;
                    let mut count = 0_u32;
                    for sprite in &scene.monochrome_sprites[range.clone()] {
                        if self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES >= MAX_MONO_SPRITES {
                            break;
                        }
                        let primitive_index =
                            (self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES) as u32;
                        write_monochrome_sprite(&mut self.mono_sprites, sprite);
                        write_scene_animation_binding(
                            self,
                            &mut summary,
                            sprite.animation_id,
                            NovaAnimatedPrimitiveKind::MonochromeSprite,
                            primitive_index,
                        );
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches.push(NovaUploadedBatch::MonoSprites {
                            texture_id: *texture_id,
                            first,
                            count,
                        });
                        summary.mono_sprite_count = summary.mono_sprite_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::PolychromeSprites { texture_id, range } => {
                    let first = (self.poly_sprites.len() / PACKED_POLY_SPRITE_BYTES) as u32;
                    let mut count = 0_u32;
                    for sprite in &scene.polychrome_sprites[range.clone()] {
                        if self.poly_sprites.len() / PACKED_POLY_SPRITE_BYTES >= MAX_POLY_SPRITES {
                            break;
                        }
                        let primitive_index =
                            (self.poly_sprites.len() / PACKED_POLY_SPRITE_BYTES) as u32;
                        write_polychrome_sprite(&mut self.poly_sprites, sprite);
                        write_scene_animation_binding(
                            self,
                            &mut summary,
                            sprite.animation_id,
                            NovaAnimatedPrimitiveKind::PolychromeSprite,
                            primitive_index,
                        );
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches.push(NovaUploadedBatch::PolySprites {
                            texture_id: *texture_id,
                            first,
                            count,
                        });
                        summary.poly_sprite_count = summary.poly_sprite_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::Underlines(range) => {
                    let first = (self.underlines.len() / PACKED_UNDERLINE_BYTES) as u32;
                    let mut count = 0_u32;
                    for underline in &scene.underlines[range.clone()] {
                        if self.underlines.len() / PACKED_UNDERLINE_BYTES >= MAX_UNDERLINES {
                            break;
                        }
                        write_underline(&mut self.underlines, underline);
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::Underlines { first, count });
                        summary.underline_count = summary.underline_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::Paths(range) => {
                    let paths = &scene.paths[range.clone()];
                    let first_vertex = (self.path_rasterization_vertices.len()
                        / PACKED_PATH_RASTERIZATION_VERTEX_BYTES)
                        as u32;
                    let mut vertex_count = 0_u32;
                    for path in paths {
                        let Some(encoded) = self.encoded_path_rasterization(path) else {
                            continue;
                        };
                        let remaining_vertices = MAX_PATH_VERTICES.saturating_sub(
                            self.path_rasterization_vertices.len()
                                / PACKED_PATH_RASTERIZATION_VERTEX_BYTES,
                        );
                        let encoded_vertex_count = encoded.vertex_count as usize;
                        if encoded_vertex_count > remaining_vertices {
                            break;
                        }
                        self.path_rasterization_vertices
                            .extend_from_slice(&encoded.bytes);
                        vertex_count = vertex_count.saturating_add(encoded.vertex_count);
                    }
                    if vertex_count > 0 {
                        self.batches.push(NovaUploadedBatch::PathRasterization {
                            first_vertex,
                            vertex_count,
                        });
                        summary.path_vertex_count =
                            summary.path_vertex_count.saturating_add(vertex_count);
                    }

                    let Some(first_path) = paths.first() else {
                        continue;
                    };
                    let first = (self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES) as u32;
                    let mut count = 0_u32;
                    if paths
                        .last()
                        .is_some_and(|path| path.order == first_path.order)
                    {
                        for path in paths {
                            if self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES
                                >= MAX_PATH_SPRITES
                            {
                                break;
                            }
                            write_path_sprite(&mut self.path_sprites, &path.clipped_bounds());
                            count = count.saturating_add(1);
                        }
                    } else {
                        let mut bounds = first_path.clipped_bounds();
                        for path in paths.iter().skip(1) {
                            bounds = bounds.union(&path.clipped_bounds());
                        }
                        if self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES < MAX_PATH_SPRITES {
                            write_path_sprite(&mut self.path_sprites, &bounds);
                            count = 1;
                        }
                    }
                    if count > 0 {
                        self.batches.push(NovaUploadedBatch::Paths { first, count });
                        summary.path_sprite_count = summary.path_sprite_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::Surfaces(_) => {
                    summary.unsupported_batches.surfaces =
                        summary.unsupported_batches.surfaces.saturating_add(1);
                }
                PreparedSceneBatch::BackdropBlurs(group) => {
                    if backdrop_blur_quality == NovaBackdropBlurQuality::Disabled {
                        let first = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
                        let mut count = 0_u32;
                        for blur in &scene.backdrop_blurs[group.range.clone()] {
                            if self.quads.len() / PACKED_QUAD_BYTES >= MAX_QUADS {
                                break;
                            }
                            let Some(tint) = blur.tint.filter(|tint| !tint.is_transparent()) else {
                                continue;
                            };
                            let primitive_index = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
                            let quad = Quad {
                                order: blur.order,
                                border_style: crate::BorderStyle::Solid,
                                animation_id: blur.animation_id,
                                bounds: blur.bounds,
                                content_mask: blur.content_mask.clone(),
                                background: tint.into(),
                                border_color: crate::Hsla::transparent_black(),
                                corner_radii: blur.corner_radii,
                                border_widths: Default::default(),
                            };
                            write_quad(&mut self.quads, &quad);
                            write_scene_animation_binding(
                                self,
                                &mut summary,
                                quad.animation_id,
                                NovaAnimatedPrimitiveKind::Quad,
                                primitive_index,
                            );
                            count = count.saturating_add(1);
                        }
                        if count > 0 {
                            self.batches.push(NovaUploadedBatch::Quads { first, count });
                            summary.quad_count = summary.quad_count.saturating_add(count);
                        }
                        continue;
                    }
                    let first = (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
                    let mut count = 0_u32;
                    for blur in &scene.backdrop_blurs[group.range.clone()] {
                        if self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES
                            >= MAX_BACKDROP_BLURS
                        {
                            break;
                        }
                        let Some(blur) = backdrop_blur_quality.adjusted_blur(blur) else {
                            continue;
                        };
                        let blur = blur.as_ref();
                        let primitive_index =
                            (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
                        if count == 0 {
                            self.backdrop_blur_passes.clear();
                            self.backdrop_blur_downsample = blur.downsample.max(1);
                            self.backdrop_blur_levels =
                                blur.levels.clamp(1, MAX_BACKDROP_BLUR_LEVELS);
                            write_backdrop_blur_pass(
                                &mut self.backdrop_blur_passes,
                                backdrop_blur_offset(blur.radius.0, blur.downsample, blur.levels),
                            );
                        }
                        write_backdrop_blur(&mut self.backdrop_blurs, blur, drawable_size);
                        write_scene_animation_binding(
                            self,
                            &mut summary,
                            blur.animation_id,
                            NovaAnimatedPrimitiveKind::BackdropBlur,
                            primitive_index,
                        );
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::BackdropBlurs { first, count });
                    }
                }
                PreparedSceneBatch::GpuMeshes3d(group) => {
                    for painted in &scene.gpu_meshes_3d[group.range.clone()] {
                        if painted.mesh.vertices.is_empty() || painted.mesh.indices.is_empty() {
                            continue;
                        }
                        if self.custom_mesh_3d_parameters.len()
                            / PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES
                            >= MAX_CUSTOM_MESH_3D_DRAWS
                        {
                            summary.unsupported_batches.gpu_meshes_3d =
                                summary.unsupported_batches.gpu_meshes_3d.saturating_add(1);
                            break;
                        }

                        if painted.mesh.vertices.len() > MAX_CUSTOM_MESH_3D_VERTICES
                            || painted.mesh.indices.len() > MAX_CUSTOM_MESH_3D_INDICES
                        {
                            summary.unsupported_batches.gpu_meshes_3d =
                                summary.unsupported_batches.gpu_meshes_3d.saturating_add(1);
                            continue;
                        }
                        let validated_ranges = [
                            mesh_range_within_vertices(
                                painted.mesh.ranges.opaque,
                                &painted.mesh.indices,
                                painted.mesh.vertices.len(),
                            ),
                            mesh_range_within_vertices(
                                painted.mesh.ranges.glass,
                                &painted.mesh.indices,
                                painted.mesh.vertices.len(),
                            ),
                            mesh_range_within_vertices(
                                painted.mesh.ranges.water,
                                &painted.mesh.indices,
                                painted.mesh.vertices.len(),
                            ),
                        ];
                        let requested_range_count = [
                            painted.mesh.ranges.opaque,
                            painted.mesh.ranges.glass,
                            painted.mesh.ranges.water,
                        ]
                        .into_iter()
                        .filter(|range| range.count > 0)
                        .count();
                        let valid_range_count = validated_ranges
                            .iter()
                            .filter(|range| range.is_some())
                            .count();
                        if valid_range_count == 0 {
                            if requested_range_count > 0 {
                                summary.unsupported_batches.gpu_meshes_3d =
                                    summary.unsupported_batches.gpu_meshes_3d.saturating_add(1);
                            }
                            continue;
                        }
                        if valid_range_count < requested_range_count {
                            summary.unsupported_batches.gpu_meshes_3d =
                                summary.unsupported_batches.gpu_meshes_3d.saturating_add(1);
                        }
                        let mesh_already_listed = self
                            .custom_mesh_3d_meshes
                            .iter()
                            .any(|mesh| mesh.id == painted.mesh.id);
                        if !mesh_already_listed {
                            let Some(next_vertex_count) =
                                custom_mesh_vertex_count.checked_add(painted.mesh.vertices.len())
                            else {
                                summary.unsupported_batches.gpu_meshes_3d =
                                    summary.unsupported_batches.gpu_meshes_3d.saturating_add(1);
                                continue;
                            };
                            let Some(next_index_count) =
                                custom_mesh_index_count.checked_add(painted.mesh.indices.len())
                            else {
                                summary.unsupported_batches.gpu_meshes_3d =
                                    summary.unsupported_batches.gpu_meshes_3d.saturating_add(1);
                                continue;
                            };
                            if next_vertex_count > MAX_CUSTOM_MESH_3D_VERTICES
                                || next_index_count > MAX_CUSTOM_MESH_3D_INDICES
                            {
                                summary.unsupported_batches.gpu_meshes_3d =
                                    summary.unsupported_batches.gpu_meshes_3d.saturating_add(1);
                                continue;
                            }
                            custom_mesh_vertex_count = next_vertex_count;
                            custom_mesh_index_count = next_index_count;
                            self.custom_mesh_3d_meshes.push(painted.mesh.clone());
                        }
                        if !self
                            .custom_mesh_3d_shaders
                            .iter()
                            .any(|shader| shader.id == painted.mesh.shader.id)
                        {
                            self.custom_mesh_3d_shaders
                                .push(painted.mesh.shader.clone());
                        }
                        let first_parameter_index = (self.custom_mesh_3d_parameters.len()
                            / PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES)
                            as u32;
                        write_custom_mesh_3d_parameters(
                            &mut self.custom_mesh_3d_parameters,
                            painted,
                        );
                        for range in validated_ranges.into_iter().flatten() {
                            self.batches.push(NovaUploadedBatch::CustomMesh3d {
                                mesh_id: painted.mesh.id,
                                generation: painted.mesh.generation,
                                shader_id: painted.mesh.shader.id,
                                range,
                                first_parameter_index,
                            });
                        }
                    }
                }
            }
        }
        summary
    }

    fn encoded_path_rasterization(
        &mut self,
        path: &crate::Path<crate::ScaledPixels>,
    ) -> Option<NovaPathRasterizationCacheEntry> {
        let vertex_count = u32::try_from(path.vertices.len()).ok()?;
        if vertex_count == 0 {
            return None;
        }

        let bounds = path.clipped_bounds();
        let mut paint_key = Vec::with_capacity(PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
        write_bounds_scaled(&mut paint_key, &bounds);
        write_background(&mut paint_key, &path.color);
        let key = NovaPathRasterizationCacheKey {
            path_id: path.cache_id,
            generation: path.geometry_generation,
            vertex_count: path.vertices.len(),
            geometry_hash: path_geometry_hash(&path.vertices),
            paint_key,
        };
        if let Some(entry) = self.path_rasterization_cache.get(&key) {
            self.path_rasterization_cache_hits =
                self.path_rasterization_cache_hits.saturating_add(1);
            return Some(entry.clone());
        }

        let mut bytes =
            Vec::with_capacity(path.vertices.len() * PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
        for vertex in &path.vertices {
            write_path_rasterization_vertex(&mut bytes, vertex, &path.color, &bounds);
        }
        let entry = NovaPathRasterizationCacheEntry {
            bytes: Arc::<[u8]>::from(bytes.into_boxed_slice()),
            vertex_count,
        };
        if self.path_rasterization_cache.len() >= MAX_PATH_RASTERIZATION_CACHE_ENTRIES {
            self.path_rasterization_cache.clear();
        }
        self.path_rasterization_cache.insert(key, entry.clone());
        self.path_rasterization_cache_misses =
            self.path_rasterization_cache_misses.saturating_add(1);
        Some(entry)
    }
}

fn path_geometry_hash(vertices: &[crate::PathVertex_ScaledPixels]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for vertex in vertices {
        hash = fnv1a_u32(hash, vertex.xy_position.x.0.to_bits());
        hash = fnv1a_u32(hash, vertex.xy_position.y.0.to_bits());
        hash = fnv1a_u32(hash, vertex.st_position.x.to_bits());
        hash = fnv1a_u32(hash, vertex.st_position.y.to_bits());
    }
    hash
}

fn fnv1a_u32(hash: u64, value: u32) -> u64 {
    let hash = hash ^ u64::from(value);
    hash.wrapping_mul(0x0000_0100_0000_01b3)
}

fn mesh_range_within_indices(
    range: crate::GpuMesh3dRange,
    index_count: usize,
) -> Option<crate::GpuMesh3dRange> {
    if range.count == 0 {
        return None;
    }
    let index_count = u32::try_from(index_count).ok()?;
    let end = range.start.checked_add(range.count)?;
    if end > index_count {
        return None;
    }
    Some(range)
}

fn mesh_range_within_vertices(
    range: crate::GpuMesh3dRange,
    indices: &[u32],
    vertex_count: usize,
) -> Option<crate::GpuMesh3dRange> {
    let range = mesh_range_within_indices(range, indices.len())?;
    let vertex_count = u32::try_from(vertex_count).ok()?;
    let start = usize::try_from(range.start).ok()?;
    let count = usize::try_from(range.count).ok()?;
    let end = start.checked_add(count)?;
    let indices = indices.get(start..end)?;
    if indices.iter().any(|index| *index >= vertex_count) {
        return None;
    }
    Some(range)
}

fn write_scene_animation_binding(
    upload: &mut NovaFrameUpload,
    summary: &mut FrameUploadSummary,
    animation_id: Option<crate::SceneAnimationId>,
    primitive_kind: NovaAnimatedPrimitiveKind,
    primitive_index: u32,
) {
    let Some(animation_id) = animation_id else {
        return;
    };
    if upload.animation_bindings.len() / PACKED_ANIMATION_BINDING_BYTES >= MAX_ANIMATION_BINDINGS {
        return;
    }
    write_animation_binding(
        &mut upload.animation_bindings,
        animation_id,
        primitive_kind,
        primitive_index,
    );
    summary.animation_binding_count = summary.animation_binding_count.saturating_add(1);
}

fn write_scene_animation_value(
    upload: &mut NovaFrameUpload,
    summary: &mut FrameUploadSummary,
    value: &crate::SceneAnimationValue,
) {
    let Some(property) = NovaAnimationProperty::from_transition_property(value.property) else {
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
}
