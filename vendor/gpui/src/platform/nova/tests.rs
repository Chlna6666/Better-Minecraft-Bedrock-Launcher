use super::*;
use crate::{
    FontId, GlyphId, GpuMesh3dDrawParameters, GpuMesh3dDrawRanges, GpuMesh3dVertex, ImageId,
    PaintGpuMesh3d, RenderGlyphParams, RenderImageParams, RenderImagePixelFormat, TileId,
    WgslShaderSource, px, size,
};
use gfx_core::{DrawIndexedStepDescriptor, IndexBufferBinding, IndexFormat, RenderStepDescriptor};
use std::cell::Cell;

fn force_atlas_full(atlas: &NovaAtlas) {
    let mut state = atlas.state.lock().expect("nova atlas lock poisoned");
    for texture_kind in [
        AtlasTextureKind::Monochrome,
        AtlasTextureKind::Bgra,
        AtlasTextureKind::Rgba,
    ] {
        state.disable_allocator_for_test(texture_kind);
    }
}

fn fallback_tile(atlas: &NovaAtlas, texture_kind: AtlasTextureKind) -> AtlasTile {
    atlas
        .state
        .lock()
        .expect("nova atlas lock poisoned")
        .fallback_tile(texture_kind)
        .expect("fallback tile should be initialized")
}

fn test_sprite_resource_set(
    texture_id: AtlasTextureId,
    mono_set: ResourceSetId,
    poly_set: ResourceSetId,
) -> Option<ResourceSetId> {
    Some(match texture_id.kind {
        AtlasTextureKind::Monochrome | AtlasTextureKind::Subpixel => mono_set,
        AtlasTextureKind::Bgra | AtlasTextureKind::Rgba => poly_set,
    })
}

fn test_gpu_mesh_3d_shader() -> Arc<GpuMesh3dShader> {
    let source = WgslShaderSource::from_source(
        "nova-test-gpu-mesh-3d-shader",
        r#"
struct MeshVertex {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct MeshOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_test_mesh(vertex: MeshVertex) -> MeshOut {
    var out: MeshOut;
    out.position = vec4<f32>(vertex.position, 1.0);
    out.color = vertex.color;
    return out;
}

@fragment
fn fs_test_mesh(input: MeshOut) -> @location(0) vec4<f32> {
    return input.color;
}
"#,
    )
    .expect("test shader should validate");
    Arc::new(GpuMesh3dShader::new(
        Arc::new(source),
        "vs_test_mesh",
        "fs_test_mesh",
    ))
}

#[test]
fn dx12_and_vulkan_auto_vsync_prefer_non_blocking_present_mode() {
    let options = RendererOptions::default();

    assert_eq!(
        nova_present_mode_for_backend(RendererBackend::NovaDx12, &options),
        gfx_core::PresentMode::Mailbox
    );
    assert_eq!(
        nova_present_mode_for_backend(RendererBackend::NovaVulkan, &options),
        gfx_core::PresentMode::Mailbox
    );
}

#[test]
fn glyph_atlas_insert_update_remove() {
    let atlas = NovaAtlas::new();
    atlas.clear_pending_uploads_for_test();
    let key = AtlasKey::Glyph(RenderGlyphParams {
        font_id: FontId(1),
        glyph_id: GlyphId(2),
        font_size: px(14.0),
        subpixel_variant: Point { x: 0, y: 0 },
        scale_factor: 1.0,
        is_emoji: false,
        is_cjk: false,
    });
    let inserted = atlas
        .ensure_tile_with(&key, &mut || {
            Ok(Some((
                size(DevicePixels(2), DevicePixels(2)),
                Cow::Borrowed(&[255; 4]),
            )))
        })
        .expect("insert should succeed");
    assert!(inserted.is_some());
    let updated = atlas
        .refresh_tile_with(&key, &mut || {
            Ok(Some((
                size(DevicePixels(4), DevicePixels(4)),
                Cow::Borrowed(&[128; 16]),
            )))
        })
        .expect("update should succeed")
        .expect("update should return tile");
    assert_eq!(updated.bounds.size, size(DevicePixels(4), DevicePixels(4)));
    assert_eq!(atlas.pending_upload_count_for_test(), 2);
    atlas.remove(&key);
    let missing = atlas
        .ensure_tile_with(&key, &mut || Ok(None))
        .expect("lookup should succeed");
    assert!(missing.is_none());
}

#[test]
fn glyph_atlas_preserves_existing_tiles_when_full() {
    let atlas = NovaAtlas::new();
    atlas.clear_pending_uploads_for_test();
    let existing_key = AtlasKey::Glyph(RenderGlyphParams {
        font_id: FontId(1),
        glyph_id: GlyphId(1),
        font_size: px(14.0),
        subpixel_variant: Point { x: 0, y: 0 },
        scale_factor: 1.0,
        is_emoji: false,
        is_cjk: false,
    });
    atlas
        .ensure_tile_with(&existing_key, &mut || {
            Ok(Some((
                size(DevicePixels(2), DevicePixels(2)),
                Cow::Borrowed(&[255; 4]),
            )))
        })
        .expect("existing glyph insert should not error")
        .expect("existing glyph should allocate");

    force_atlas_full(&atlas);

    let small_params = RenderGlyphParams {
        font_id: FontId(1),
        glyph_id: GlyphId(2),
        font_size: px(14.0),
        subpixel_variant: Point { x: 0, y: 0 },
        scale_factor: 1.0,
        is_emoji: false,
        is_cjk: false,
    };
    let missing = atlas
        .ensure_glyph_with(&small_params, &mut || {
            Ok(GlyphRasterization::Bitmap {
                size: size(DevicePixels(2), DevicePixels(2)),
                bytes: vec![128; 4],
            })
        })
        .expect("full atlas should not error");

    let fallback = missing.expect("full atlas should return fallback glyph tile");
    assert_eq!(
        fallback,
        fallback_tile(&atlas, AtlasTextureKind::Monochrome)
    );
    assert!(
        atlas
            .ensure_tile_with(&existing_key, &mut || Ok(None))
            .expect("existing tile lookup should not error")
            .is_some()
    );
}

#[test]
fn image_atlas_preserves_existing_tiles_when_full() {
    let atlas = NovaAtlas::new();
    atlas.clear_pending_uploads_for_test();
    let existing_key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(11),
        frame_slot: 0,
        pixel_format: RenderImagePixelFormat::Rgba8,
    });
    atlas
        .ensure_tile_with(&existing_key, &mut || {
            Ok(Some((
                size(DevicePixels(2), DevicePixels(2)),
                Cow::Borrowed(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
            )))
        })
        .expect("existing image insert should not error")
        .expect("existing image should allocate");

    force_atlas_full(&atlas);

    let small_key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(12),
        frame_slot: 0,
        pixel_format: RenderImagePixelFormat::Rgba8,
    });
    let missing = atlas
        .ensure_tile_with(&small_key, &mut || {
            Ok(Some((
                size(DevicePixels(2), DevicePixels(2)),
                Cow::Borrowed(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
            )))
        })
        .expect("full atlas should not error");

    let fallback = missing.expect("full atlas should return fallback image tile");
    assert_eq!(fallback, fallback_tile(&atlas, AtlasTextureKind::Rgba));
    assert_eq!(
        atlas
            .ensure_tile_with(&small_key, &mut || {
                Ok(Some((
                    size(DevicePixels(2), DevicePixels(2)),
                    Cow::Borrowed(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
                )))
            })
            .expect("fallback lookup should not error"),
        Some(fallback)
    );
    assert!(
        atlas
            .ensure_tile_with(&existing_key, &mut || Ok(None))
            .expect("existing tile lookup should not error")
            .is_some()
    );
}

#[test]
fn atlas_fallback_tiles_are_not_deallocated_through_cached_keys() {
    let atlas = NovaAtlas::new();
    atlas.clear_pending_uploads_for_test();
    force_atlas_full(&atlas);

    let first_parameters = RenderGlyphParams {
        font_id: FontId(1),
        glyph_id: GlyphId(10),
        font_size: px(14.0),
        subpixel_variant: Point { x: 0, y: 0 },
        scale_factor: 1.0,
        is_emoji: false,
        is_cjk: false,
    };
    let second_params = RenderGlyphParams {
        glyph_id: GlyphId(11),
        ..first_parameters.clone()
    };
    let first_key = AtlasKey::from(first_parameters.clone());
    let second_key = AtlasKey::from(second_params.clone());

    let first = atlas
        .ensure_glyph_with(&first_parameters, &mut || {
            Ok(GlyphRasterization::Bitmap {
                size: size(DevicePixels(2), DevicePixels(2)),
                bytes: vec![128; 4],
            })
        })
        .expect("full atlas should return fallback")
        .expect("fallback tile should exist");
    let second = atlas
        .ensure_glyph_with(&second_params, &mut || {
            Ok(GlyphRasterization::Bitmap {
                size: size(DevicePixels(2), DevicePixels(2)),
                bytes: vec![128; 4],
            })
        })
        .expect("full atlas should return fallback")
        .expect("fallback tile should exist");

    assert_eq!(first, fallback_tile(&atlas, AtlasTextureKind::Monochrome));
    assert_eq!(second, first);
    atlas.remove(&first_key);
    atlas.remove(&second_key);

    assert_eq!(fallback_tile(&atlas, AtlasTextureKind::Monochrome), first);
}

#[test]
fn full_color_atlas_does_not_starve_monochrome_glyphs() {
    let atlas = NovaAtlas::new();
    atlas.clear_pending_uploads_for_test();
    {
        let mut state = atlas.state.lock().expect("nova atlas lock poisoned");
        state.disable_allocator_for_test(AtlasTextureKind::Bgra);
    }

    let glyph_params = RenderGlyphParams {
        font_id: FontId(1),
        glyph_id: GlyphId(9),
        font_size: px(14.0),
        subpixel_variant: Point { x: 0, y: 0 },
        scale_factor: 1.0,
        is_emoji: false,
        is_cjk: false,
    };
    let glyph_tile = atlas
        .ensure_glyph_with(&glyph_params, &mut || {
            Ok(GlyphRasterization::Bitmap {
                size: size(DevicePixels(2), DevicePixels(2)),
                bytes: vec![255; 4],
            })
        })
        .expect("monochrome glyph insert should not error")
        .expect("monochrome glyph should still allocate");

    assert_eq!(glyph_tile.texture_id.kind, AtlasTextureKind::Monochrome);
    assert_ne!(
        glyph_tile,
        fallback_tile(&atlas, AtlasTextureKind::Monochrome)
    );
}

#[test]
fn monochrome_atlas_upload_uses_red_channel_coverage() {
    let mut pixels = [0_u8; 12];

    encode_bgra_upload(
        &mut pixels,
        size(DevicePixels(3), DevicePixels(1)),
        &[0, 128, 255],
        AtlasTextureKind::Monochrome,
    )
    .expect("monochrome upload should encode");

    assert_eq!(pixels, [0, 0, 0, 255, 0, 0, 128, 255, 0, 0, 255, 255]);
}

#[test]
fn subpixel_atlas_fallback_upload_uses_grayscale_red_channel_coverage() {
    let mut pixels = [0_u8; 8];

    encode_bgra_upload(
        &mut pixels,
        size(DevicePixels(2), DevicePixels(1)),
        &[255, 0, 0, 255, 0, 255, 255, 128],
        AtlasTextureKind::Subpixel,
    )
    .expect("subpixel fallback upload should encode");

    assert_eq!(pixels, [0, 0, 85, 255, 0, 0, 85, 255]);
}

#[test]
fn monochrome_atlas_upload_rejects_short_source_data() {
    let mut pixels = [0_u8; 8];

    let encoded = encode_bgra_upload(
        &mut pixels,
        size(DevicePixels(2), DevicePixels(1)),
        &[255],
        AtlasTextureKind::Monochrome,
    );

    assert!(encoded.is_none());
}

#[test]
fn image_atlas_insert_returns_tile_for_rgba_and_bgra() {
    let atlas = NovaAtlas::new();
    atlas.clear_pending_uploads_for_test();
    let rgba_key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(1),
        frame_slot: 0,
        pixel_format: RenderImagePixelFormat::Rgba8,
    });
    let rgba_tile = atlas
        .ensure_tile_with(&rgba_key, &mut || {
            Ok(Some((
                size(DevicePixels(1), DevicePixels(1)),
                Cow::Borrowed(&[10, 20, 30, 40]),
            )))
        })
        .expect("rgba insert should succeed")
        .expect("rgba image should allocate a tile");
    assert_eq!(rgba_tile.texture_id.kind, AtlasTextureKind::Rgba);
    assert_eq!(rgba_tile.padding, NOVA_ATLAS_TILE_PADDING);
    assert_eq!(
        rgba_tile.bounds.size,
        size(DevicePixels(1), DevicePixels(1))
    );
    assert_eq!(
        &atlas.pending_upload_bytes_for_test()[0..4],
        &[30, 20, 10, 40]
    );
    let rgba_center_offset = (NOVA_ATLAS_SIZE.min(3) as usize + 1) * NOVA_ATLAS_BYTES_PER_PIXEL;
    assert_eq!(
        &atlas.pending_upload_bytes_for_test()[rgba_center_offset..rgba_center_offset + 4],
        &[30, 20, 10, 40]
    );

    let bgra_key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(2),
        frame_slot: 0,
        pixel_format: RenderImagePixelFormat::Bgra8,
    });
    let bgra_tile = atlas
        .ensure_tile_with(&bgra_key, &mut || {
            Ok(Some((
                size(DevicePixels(1), DevicePixels(1)),
                Cow::Borrowed(&[1, 2, 3, 4]),
            )))
        })
        .expect("bgra insert should succeed")
        .expect("bgra image should allocate a tile");
    assert_eq!(bgra_tile.texture_id.kind, AtlasTextureKind::Bgra);
    assert_eq!(
        bgra_tile.bounds.size,
        size(DevicePixels(1), DevicePixels(1))
    );

    let pixels = atlas.pending_upload_bytes_for_test();
    let padded_rgba_bytes = (rgba_tile.bounds.size.width.0.max(1) as usize
        + (rgba_tile.padding as usize * 2))
        * (rgba_tile.bounds.size.height.0.max(1) as usize + (rgba_tile.padding as usize * 2))
        * NOVA_ATLAS_BYTES_PER_PIXEL;
    let bgra_offset = padded_rgba_bytes;
    assert_eq!(&pixels[bgra_offset..bgra_offset + 4], &[1, 2, 3, 4]);
}

#[test]
fn pending_atlas_upload_borrows_pixels_without_repeating_clean_upload() {
    let atlas = NovaAtlas::new();
    atlas.clear_pending_uploads_for_test();
    let uploads = Cell::new(0);
    let key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(3),
        frame_slot: 0,
        pixel_format: RenderImagePixelFormat::Rgba8,
    });
    let tile = atlas
        .ensure_tile_with(&key, &mut || {
            Ok(Some((
                size(DevicePixels(2), DevicePixels(2)),
                Cow::Borrowed(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
            )))
        })
        .expect("insert should succeed")
        .expect("image should allocate a tile");

    atlas
        .upload_pending_rgba_pixels(
            |_| Ok(TextureId::from_parts(0, 0)),
            |writes| {
                let [write] = writes else {
                    panic!("expected exactly one atlas upload");
                };
                let descriptor = write.descriptor;
                let pixels = write.data;
                assert_eq!(
                    descriptor.origin.x,
                    tile.bounds.origin.x.0.saturating_sub(tile.padding as i32) as u32
                );
                assert_eq!(
                    descriptor.origin.y,
                    tile.bounds.origin.y.0.saturating_sub(tile.padding as i32) as u32
                );
                assert_eq!(descriptor.size.width(), 2 + tile.padding * 2);
                assert_eq!(descriptor.size.height(), 2 + tile.padding * 2);
                assert_eq!(
                    descriptor.layout.bytes_per_row.get(),
                    descriptor.size.width() * NOVA_ATLAS_BYTES_PER_PIXEL as u32
                );
                assert_eq!(
                    pixels.len(),
                    descriptor.layout.bytes_per_row.get() as usize
                        * descriptor.size.height() as usize
                );
                uploads.set(uploads.get() + 1);
                Ok(())
            },
        )
        .expect("initial dirty upload should succeed");
    atlas
        .upload_pending_rgba_pixels(
            |_| Ok(TextureId::from_parts(0, 0)),
            |writes| {
                assert!(writes.is_empty());
                uploads.set(uploads.get() + 1);
                Ok(())
            },
        )
        .expect("clean upload should be skipped");

    assert_eq!(uploads.get(), 1);
}

#[test]
fn pending_atlas_upload_updates_existing_tile_in_place() {
    let atlas = NovaAtlas::new();
    atlas.clear_pending_uploads_for_test();
    let key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(7),
        frame_slot: 0,
        pixel_format: RenderImagePixelFormat::Rgba8,
    });
    atlas
        .ensure_tile_with(&key, &mut || {
            Ok(Some((
                size(DevicePixels(1), DevicePixels(1)),
                Cow::Borrowed(&[1, 2, 3, 4]),
            )))
        })
        .expect("initial insert should succeed")
        .expect("tile should be allocated");
    atlas
        .refresh_tile_with(&key, &mut || {
            Ok(Some((
                size(DevicePixels(1), DevicePixels(1)),
                Cow::Borrowed(&[5, 6, 7, 8]),
            )))
        })
        .expect("first update should succeed");
    atlas
        .refresh_tile_with(&key, &mut || {
            Ok(Some((
                size(DevicePixels(1), DevicePixels(1)),
                Cow::Borrowed(&[9, 10, 11, 12]),
            )))
        })
        .expect("second update should succeed");

    assert_eq!(atlas.pending_upload_count_for_test(), 1);
    atlas
        .upload_pending_rgba_pixels(
            |_| Ok(TextureId::from_parts(0, 0)),
            |writes| {
                let [write] = writes else {
                    panic!("expected one coalesced atlas upload");
                };
                let pixels = write.data;
                assert!(
                    pixels
                        .chunks_exact(NOVA_ATLAS_BYTES_PER_PIXEL)
                        .any(|pixel| pixel == [11, 10, 9, 12])
                );
                assert!(
                    !pixels
                        .chunks_exact(NOVA_ATLAS_BYTES_PER_PIXEL)
                        .any(|pixel| pixel == [7, 6, 5, 8])
                );
                Ok(())
            },
        )
        .expect("coalesced upload should succeed");
}

#[test]
fn quad_packer_matches_shader_storage_stride() {
    let mut bytes = Vec::new();

    write_quad(&mut bytes, &Quad::default());

    assert_eq!(bytes.len(), PACKED_QUAD_BYTES);
}

#[test]
fn animation_binding_packer_matches_shader_storage_stride() {
    let mut bytes = Vec::new();

    write_animation_binding(
        &mut bytes,
        crate::SceneAnimationId(7),
        NovaAnimatedPrimitiveKind::Quad,
        3,
    );

    assert_eq!(bytes.len(), PACKED_ANIMATION_BINDING_BYTES);
}

#[test]
fn animation_value_packer_matches_shader_storage_stride() {
    let mut bytes = Vec::new();

    write_animation_value(
        &mut bytes,
        crate::SceneAnimationId(7),
        NovaAnimationProperty::Opacity,
        1.25,
        [0.0, 1.0, 2.0, 3.0],
        [4.0, 5.0, 6.0, 7.0],
    );

    assert_eq!(bytes.len(), PACKED_ANIMATION_VALUE_BYTES);
    assert_eq!(read_u32_at(&bytes, 0), 7);
    assert_eq!(
        read_u32_at(&bytes, 4),
        NovaAnimationProperty::Opacity as u32
    );
    assert_eq!(read_f32_at(&bytes, 8), 1.0);
    assert_eq!(read_f32_at(&bytes, 16), 0.0);
    assert_eq!(read_f32_at(&bytes, 28), 3.0);
    assert_eq!(read_f32_at(&bytes, 32), 4.0);
    assert_eq!(read_f32_at(&bytes, 44), 7.0);
    assert_eq!(read_u32_at(&bytes, 48), 0);
}

#[test]
fn nova_animation_property_maps_only_gpu_eligible_transitions() {
    assert_eq!(
        NovaAnimationProperty::from_transition_property(crate::TransitionProperty::Opacity),
        Some(NovaAnimationProperty::Opacity)
    );
    assert_eq!(
        NovaAnimationProperty::from_transition_property(crate::TransitionProperty::Transform),
        Some(NovaAnimationProperty::Transform)
    );
    assert_eq!(
        NovaAnimationProperty::from_transition_property(crate::TransitionProperty::Color),
        Some(NovaAnimationProperty::SolidColor)
    );
    assert_eq!(
        NovaAnimationProperty::from_transition_property(crate::TransitionProperty::Width),
        None
    );
}

#[test]
fn monochrome_sprite_packer_matches_shader_storage_stride() {
    let sprite = MonochromeSprite {
        order: 0,
        pad: 0,
        animation_id: None,
        bounds: Bounds::default(),
        content_mask: Default::default(),
        color: Default::default(),
        tile: test_atlas_tile(),
        transformation: Default::default(),
    };
    let mut bytes = Vec::new();

    write_monochrome_sprite(&mut bytes, &sprite);

    assert_eq!(bytes.len(), PACKED_MONO_SPRITE_BYTES);
}

#[test]
fn shadow_packer_matches_shader_storage_stride() {
    let mut bytes = Vec::new();

    write_shadow(
        &mut bytes,
        &Shadow {
            order: 0,
            blur_radius: crate::ScaledPixels(1.0),
            animation_id: None,
            bounds: Bounds::default(),
            corner_radii: Default::default(),
            content_mask: Default::default(),
            color: Default::default(),
        },
    );

    assert_eq!(bytes.len(), PACKED_SHADOW_BYTES);
}

#[test]
fn path_rasterization_vertex_packer_matches_shader_storage_stride() {
    let mut bytes = Vec::new();
    let vertex = crate::PathVertex_ScaledPixels {
        xy_position: Point {
            x: crate::ScaledPixels(1.0),
            y: crate::ScaledPixels(2.0),
        },
        st_position: Point { x: 0.25, y: 0.75 },
        content_mask: Default::default(),
    };

    write_path_rasterization_vertex(
        &mut bytes,
        &vertex,
        &crate::Background::default(),
        &Bounds::default(),
    );

    assert_eq!(bytes.len(), PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
}

#[test]
fn path_sprite_packer_matches_shader_storage_stride() {
    let mut bytes = Vec::new();

    write_path_sprite(&mut bytes, &Bounds::default());

    assert_eq!(bytes.len(), PACKED_PATH_SPRITE_BYTES);
}

#[test]
fn polychrome_sprite_packer_matches_shader_storage_stride() {
    let sprite = PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false,
        opacity: 1.0,
        animation_id: None,
        bounds: Bounds::default(),
        content_mask: Default::default(),
        corner_radii: Default::default(),
        tile: test_atlas_tile(),
    };
    let mut bytes = Vec::new();

    write_polychrome_sprite(&mut bytes, &sprite);

    assert_eq!(bytes.len(), PACKED_POLY_SPRITE_BYTES);
}

#[test]
fn underline_packer_matches_shader_storage_stride() {
    let mut bytes = Vec::new();

    write_underline(
        &mut bytes,
        &Underline {
            order: 0,
            pad: 0,
            bounds: Bounds::default(),
            content_mask: Default::default(),
            color: Default::default(),
            thickness: crate::ScaledPixels(1.0),
            wavy: 0,
        },
    );

    assert_eq!(bytes.len(), PACKED_UNDERLINE_BYTES);
}

#[test]
fn unsupported_batch_summary_counts_each_advanced_batch_kind() {
    let summary = UnsupportedBatchSummary {
        paths: 1,
        surfaces: 2,
        backdrop_blurs: 3,
        backdrop_blur_tint_fallbacks: 4,
        gpu_meshes_3d: 5,
    };

    assert_eq!(summary.total(), 15);
}

#[test]
fn frame_upload_globals_follow_surface_alpha_mode() {
    let scene = crate::Scene::default();
    let mut upload = NovaFrameUpload::default();
    let rendering_parameters = NovaRenderingParameters::from_env();
    let drawable_size = DrawableSize {
        width: 640,
        height: 480,
    };

    upload.encode(
        &scene,
        drawable_size,
        &rendering_parameters,
        false,
        NovaBackdropBlurQuality::Full,
    );
    assert_eq!(read_u32_at(&upload.globals, 8), 0);

    upload.encode(
        &scene,
        drawable_size,
        &rendering_parameters,
        true,
        NovaBackdropBlurQuality::Full,
    );
    assert_eq!(read_u32_at(&upload.globals, 8), 1);
}

#[test]
fn frame_upload_records_scene_animation_bindings() {
    let mut scene = crate::Scene::default();
    let bounds = Bounds {
        origin: Point {
            x: crate::ScaledPixels(0.0),
            y: crate::ScaledPixels(0.0),
        },
        size: Size {
            width: crate::ScaledPixels(16.0),
            height: crate::ScaledPixels(16.0),
        },
    };
    let animation_id = scene.allocate_animation_id();
    scene.insert_animated_primitive(
        Quad {
            bounds,
            content_mask: crate::ContentMask { bounds },
            ..Quad::default()
        },
        animation_id,
    );
    scene.finish();

    let mut upload = NovaFrameUpload::default();
    let summary = upload.encode(
        &scene,
        DrawableSize {
            width: 64,
            height: 64,
        },
        &NovaRenderingParameters::from_env(),
        true,
        NovaBackdropBlurQuality::Full,
    );

    assert_eq!(summary.quad_count, 1);
    assert_eq!(summary.animation_binding_count, 1);
    assert_eq!(
        upload.animation_bindings.len(),
        PACKED_ANIMATION_BINDING_BYTES
    );
    assert_eq!(read_u32_at(&upload.animation_bindings, 0), animation_id.0);
    assert_eq!(
        read_u32_at(&upload.animation_bindings, 4),
        NovaAnimatedPrimitiveKind::Quad as u32
    );
    assert_eq!(read_u32_at(&upload.animation_bindings, 8), 0);
}

#[test]
fn frame_upload_records_scene_animation_values() {
    let mut scene = crate::Scene::default();
    let animation_id = scene.allocate_animation_id();
    scene.push_animation_value(crate::SceneAnimationValue {
        animation_id,
        property: crate::TransitionProperty::Opacity,
        progress: 0.25,
        from: [0.0, 0.0, 0.0, 0.0],
        to: [1.0, 0.0, 0.0, 0.0],
    });

    let mut upload = NovaFrameUpload::default();
    let summary = upload.encode(
        &scene,
        DrawableSize {
            width: 64,
            height: 64,
        },
        &NovaRenderingParameters::from_env(),
        true,
        NovaBackdropBlurQuality::Full,
    );

    assert_eq!(summary.animation_value_count, 1);
    assert_eq!(upload.animation_values.len(), PACKED_ANIMATION_VALUE_BYTES);
    assert_eq!(read_u32_at(&upload.animation_values, 0), animation_id.0);
    assert_eq!(read_f32_at(&upload.animation_values, 8), 0.25);
    assert_eq!(read_f32_at(&upload.animation_values, 32), 1.0);
}

#[test]
fn frame_upload_reuses_static_path_rasterization_bytes() {
    let mut path = crate::Path::new(Point {
        x: px(0.0),
        y: px(0.0),
    });
    path.line_to(Point {
        x: px(8.0),
        y: px(0.0),
    });
    path.line_to(Point {
        x: px(8.0),
        y: px(8.0),
    });
    path.content_mask = crate::ContentMask {
        bounds: Bounds {
            origin: Point {
                x: px(0.0),
                y: px(0.0),
            },
            size: Size {
                width: px(16.0),
                height: px(16.0),
            },
        },
    };

    let mut scene = crate::Scene::default();
    scene.push_layer(Bounds {
        origin: Point {
            x: crate::ScaledPixels(0.0),
            y: crate::ScaledPixels(0.0),
        },
        size: Size {
            width: crate::ScaledPixels(16.0),
            height: crate::ScaledPixels(16.0),
        },
    });
    scene.insert_primitive(path.scale(1.0));
    scene.finish();

    let mut upload = NovaFrameUpload::default();
    let drawable_size = DrawableSize {
        width: 64,
        height: 64,
    };

    let first = upload.encode(
        &scene,
        drawable_size,
        &NovaRenderingParameters::from_env(),
        true,
        NovaBackdropBlurQuality::Full,
    );
    assert_eq!(first.path_vertex_count, 3);
    assert_eq!(upload.path_rasterization_cache_hits, 0);
    assert_eq!(upload.path_rasterization_cache_misses, 1);

    let second = upload.encode(
        &scene,
        drawable_size,
        &NovaRenderingParameters::from_env(),
        true,
        NovaBackdropBlurQuality::Full,
    );
    assert_eq!(second.path_vertex_count, 3);
    assert_eq!(upload.path_rasterization_cache_hits, 1);
    assert_eq!(upload.path_rasterization_cache_misses, 1);
}

#[test]
fn transparent_window_uses_premultiplied_surface_alpha_like_gpu() {
    let transparent = NovaSurfaceAlphaState::for_window_transparency(true);
    assert_eq!(
        transparent.swapchain_mode,
        CompositeAlphaMode::Premultiplied
    );
    assert!(transparent.outputs_premultiplied_alpha());

    let opaque = NovaSurfaceAlphaState::for_window_transparency(false);
    assert_eq!(opaque.swapchain_mode, CompositeAlphaMode::Opaque);
    assert!(!opaque.outputs_premultiplied_alpha());
}

#[cfg(target_os = "windows")]
#[test]
fn dx12_transparent_window_uses_premultiplied_swapchain_alpha() {
    let transparent = NovaRenderer::alpha_state_for_window_transparency_on_backend(
        RendererBackend::NovaDx12,
        true,
    );
    assert_eq!(
        transparent.swapchain_mode,
        CompositeAlphaMode::Premultiplied
    );
    assert_eq!(
        transparent.output_mode,
        NovaSurfaceOutputMode::Premultiplied
    );
    assert!(transparent.outputs_premultiplied_alpha());
}

#[test]
fn auto_surface_alpha_uses_straight_output_like_gpu() {
    let alpha = NovaSurfaceAlphaState::new(CompositeAlphaMode::Auto);
    assert_eq!(alpha.swapchain_mode, CompositeAlphaMode::Auto);
    assert_eq!(alpha.output_mode, NovaSurfaceOutputMode::Straight);
    assert!(!alpha.outputs_premultiplied_alpha());
}

#[test]
fn backdrop_blur_encodes_real_batch_without_tint_fallback() {
    let scene = backdrop_blur_scene(Some(crate::Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: 0.5,
    }));
    let mut upload = NovaFrameUpload::default();

    let summary = upload.encode(
        &scene,
        DrawableSize {
            width: 640,
            height: 480,
        },
        &NovaRenderingParameters::from_env(),
        true,
        NovaBackdropBlurQuality::Full,
    );

    assert_eq!(summary.unsupported_batches.backdrop_blurs, 0);
    assert_eq!(summary.unsupported_batches.backdrop_blur_tint_fallbacks, 0);
    assert_eq!(summary.quad_count, 0);
    assert_eq!(upload.quads.len(), 0);
    assert_eq!(upload.backdrop_blur_passes.len(), BACKDROP_BLUR_PASS_BYTES);
    assert_eq!(upload.backdrop_blurs.len(), PACKED_BACKDROP_BLUR_BYTES);
    assert!(matches!(
        upload.batches.as_slice(),
        [NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 }]
    ));
}

#[test]
fn reduced_backdrop_blur_quality_lowers_uploaded_blur_parameters() {
    let scene = backdrop_blur_scene(Some(crate::Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: 0.5,
    }));
    let mut upload = NovaFrameUpload::default();

    upload.encode(
        &scene,
        DrawableSize {
            width: 640,
            height: 480,
        },
        &NovaRenderingParameters::from_env(),
        true,
        NovaBackdropBlurQuality::Reduced,
    );

    assert_eq!(upload.backdrop_blur_downsample(), 4);
    assert_eq!(upload.backdrop_blur_levels(), 1);
    assert_eq!(read_f32_at(&upload.backdrop_blurs, 80), 6.0);
}

#[test]
fn disabled_backdrop_blur_quality_uses_tint_quad_fallback() {
    let scene = backdrop_blur_scene(Some(crate::Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: 0.5,
    }));
    let mut upload = NovaFrameUpload::default();

    let summary = upload.encode(
        &scene,
        DrawableSize {
            width: 640,
            height: 480,
        },
        &NovaRenderingParameters::from_env(),
        true,
        NovaBackdropBlurQuality::Disabled,
    );

    assert_eq!(summary.quad_count, 1);
    assert_eq!(upload.backdrop_blurs.len(), 0);
    assert!(matches!(
        upload.batches.as_slice(),
        [NovaUploadedBatch::Quads { first: 0, count: 1 }]
    ));
}

#[test]
fn frame_upload_lists_repeated_custom_gpu_mesh_once() {
    let mesh = Arc::new(GpuMesh3d::new(
        vec![
            GpuMesh3dVertex {
                position: [0.0, 0.0, 0.0],
                color: [1.0, 0.0, 0.0, 1.0],
            },
            GpuMesh3dVertex {
                position: [1.0, 0.0, 0.0],
                color: [0.0, 1.0, 0.0, 1.0],
            },
            GpuMesh3dVertex {
                position: [0.0, 1.0, 0.0],
                color: [0.0, 0.0, 1.0, 1.0],
            },
        ],
        vec![0, 1, 2],
        GpuMesh3dDrawRanges {
            opaque: GpuMesh3dRange { start: 0, count: 3 },
            glass: GpuMesh3dRange::default(),
            water: GpuMesh3dRange::default(),
        },
        [0.5, 0.5, 0.0],
        1.0,
        1.0,
        test_gpu_mesh_3d_shader(),
    ));
    let bounds = Bounds::new(
        crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
        crate::size(crate::ScaledPixels(10.0), crate::ScaledPixels(10.0)),
    );
    let content_mask = crate::ContentMask { bounds };
    let parameters = GpuMesh3dDrawParameters {
        view_projection_model: [[1.0, 0.0, 0.0, 0.0]; 4],
    };
    let mut scene = crate::Scene::default();
    for order in [0, 1] {
        scene.insert_primitive(PaintGpuMesh3d {
            order,
            bounds,
            content_mask: content_mask.clone(),
            mesh: mesh.clone(),
            parameters,
        });
    }
    scene.finish();

    let mut upload = NovaFrameUpload::default();
    let summary = upload.encode(
        &scene,
        DrawableSize {
            width: 640,
            height: 480,
        },
        &NovaRenderingParameters::from_env(),
        true,
        NovaBackdropBlurQuality::Full,
    );

    assert_eq!(summary.unsupported_batches.gpu_meshes_3d, 0);
    assert_eq!(upload.custom_mesh_3d_meshes.len(), 1);
    assert_eq!(upload.custom_mesh_3d_meshes[0].id, mesh.id);
    assert_eq!(
        upload.custom_mesh_3d_parameters.len(),
        PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES * 2
    );
    assert_eq!(
        upload
            .batches
            .iter()
            .filter(|batch| matches!(batch, NovaUploadedBatch::CustomMesh3d { .. }))
            .count(),
        2
    );
}

#[test]
fn draw_steps_preserve_supported_batch_order_and_resources() {
    let mut upload = NovaFrameUpload::default();
    upload
        .batches
        .push(NovaUploadedBatch::SolidQuads { first: 0, count: 2 });
    upload
        .batches
        .push(NovaUploadedBatch::Quads { first: 2, count: 3 });
    upload
        .batches
        .push(NovaUploadedBatch::Shadows { first: 0, count: 6 });
    upload.batches.push(NovaUploadedBatch::PathRasterization {
        first_vertex: 9,
        vertex_count: 12,
    });
    upload
        .batches
        .push(NovaUploadedBatch::Paths { first: 1, count: 2 });
    upload.batches.push(NovaUploadedBatch::MonoSprites {
        texture_id: AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Monochrome,
        },
        first: 0,
        count: 4,
    });
    upload.batches.push(NovaUploadedBatch::PolySprites {
        texture_id: AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Rgba,
        },
        first: 0,
        count: 5,
    });
    upload
        .batches
        .push(NovaUploadedBatch::Underlines { first: 0, count: 7 });
    upload
        .batches
        .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });
    let pipelines = test_pipelines();
    let blend_pipelines = pipelines.alpha;
    let quad_set = test_resource_set_id(10);
    let shadow_set = test_resource_set_id(11);
    let path_set = test_resource_set_id(12);
    let mono_set = test_resource_set_id(13);
    let poly_set = test_resource_set_id(14);
    let underline_set = test_resource_set_id(15);
    let backdrop_blur_set = test_resource_set_id(16);
    let gpu_mesh_set = test_resource_set_id(17);
    let gpu_mesh_indices_buffer = test_buffer_id(18);

    let steps = draw_steps_for_upload(
        &upload,
        &pipelines,
        blend_pipelines,
        quad_set,
        shadow_set,
        path_set,
        |texture_id| test_sprite_resource_set(texture_id, mono_set, poly_set),
        |_| None,
        |_, _| None,
        underline_set,
        backdrop_blur_set,
        gpu_mesh_set,
        gpu_mesh_indices_buffer,
        NovaDrawStepMode::Present,
    );

    assert_eq!(
        steps,
        vec![
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.solid_quads,
                resource_sets: resource_set_list([quad_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 2,
                first_instance: 0,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.quads,
                resource_sets: resource_set_list([quad_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 3,
                first_instance: 2,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.shadows,
                resource_sets: resource_set_list([shadow_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 6,
                first_instance: 0,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: pipelines.paths,
                resource_sets: resource_set_list([path_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 2,
                first_instance: 1,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.mono_sprites,
                resource_sets: resource_set_list([mono_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 4,
                first_instance: 0,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.poly_sprites,
                resource_sets: resource_set_list([poly_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 5,
                first_instance: 0,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.underlines,
                resource_sets: resource_set_list([underline_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 7,
                first_instance: 0,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.backdrop_blurs,
                resource_sets: resource_set_list([backdrop_blur_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }),
        ]
    );

    let mask_steps = path_mask_draw_steps_for_upload(&upload, &pipelines, path_set);
    assert_eq!(
        mask_steps,
        vec![DrawStepDescriptor {
            pipeline: pipelines.path_rasterization,
            resource_sets: resource_set_list([path_set]),
            vertex_count: 12,
            first_vertex: 9,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        }]
    );
}

#[test]
fn path_mask_draw_steps_merge_adjacent_contiguous_vertices() {
    let mut upload = NovaFrameUpload::default();
    upload.batches.push(NovaUploadedBatch::PathRasterization {
        first_vertex: 0,
        vertex_count: 6,
    });
    upload.batches.push(NovaUploadedBatch::PathRasterization {
        first_vertex: 6,
        vertex_count: 9,
    });
    upload.batches.push(NovaUploadedBatch::PathRasterization {
        first_vertex: 18,
        vertex_count: 3,
    });

    let pipelines = test_pipelines();
    let path_set = test_resource_set_id(12);
    let steps = path_mask_draw_steps_for_upload(&upload, &pipelines, path_set);

    assert_eq!(
        steps,
        vec![
            DrawStepDescriptor {
                pipeline: pipelines.path_rasterization,
                resource_sets: resource_set_list([path_set]),
                vertex_count: 15,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            },
            DrawStepDescriptor {
                pipeline: pipelines.path_rasterization,
                resource_sets: resource_set_list([path_set]),
                vertex_count: 3,
                first_vertex: 18,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            },
        ]
    );
}

#[test]
fn draw_steps_merge_adjacent_compatible_draw_batches() {
    let mono_texture_id = AtlasTextureId {
        index: 0,
        kind: AtlasTextureKind::Monochrome,
    };
    let mut upload = NovaFrameUpload::default();
    upload.batches.push(NovaUploadedBatch::MonoSprites {
        texture_id: mono_texture_id,
        first: 0,
        count: 2,
    });
    upload.batches.push(NovaUploadedBatch::MonoSprites {
        texture_id: mono_texture_id,
        first: 2,
        count: 3,
    });
    upload
        .batches
        .push(NovaUploadedBatch::Quads { first: 0, count: 1 });
    upload.batches.push(NovaUploadedBatch::MonoSprites {
        texture_id: mono_texture_id,
        first: 5,
        count: 1,
    });

    let pipelines = test_pipelines();
    let blend_pipelines = pipelines.alpha;
    let quad_set = test_resource_set_id(10);
    let mono_set = test_resource_set_id(11);

    let steps = draw_steps_for_upload(
        &upload,
        &pipelines,
        blend_pipelines,
        quad_set,
        test_resource_set_id(12),
        test_resource_set_id(13),
        |texture_id| test_sprite_resource_set(texture_id, mono_set, test_resource_set_id(14)),
        |_| None,
        |_, _| None,
        test_resource_set_id(15),
        test_resource_set_id(16),
        test_resource_set_id(17),
        test_buffer_id(18),
        NovaDrawStepMode::Present,
    );

    assert_eq!(
        steps,
        vec![
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.mono_sprites,
                resource_sets: resource_set_list([mono_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 5,
                first_instance: 0,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.quads,
                resource_sets: resource_set_list([quad_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }),
            RenderStepDescriptor::Draw(DrawStepDescriptor {
                pipeline: blend_pipelines.mono_sprites,
                resource_sets: resource_set_list([mono_set]),
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 5,
                scissor: None,
            }),
        ]
    );
}

#[test]
fn draw_steps_emit_zero_instance_clear_step_when_scene_is_empty() {
    let upload = NovaFrameUpload::default();
    let pipelines = test_pipelines();
    let blend_pipelines = pipelines.premultiplied;
    let quad_set = test_resource_set_id(10);

    let steps = draw_steps_for_upload(
        &upload,
        &pipelines,
        blend_pipelines,
        quad_set,
        test_resource_set_id(11),
        test_resource_set_id(12),
        |texture_id| {
            test_sprite_resource_set(
                texture_id,
                test_resource_set_id(11),
                test_resource_set_id(12),
            )
        },
        |_| None,
        |_, _| None,
        test_resource_set_id(13),
        test_resource_set_id(14),
        test_resource_set_id(15),
        test_buffer_id(16),
        NovaDrawStepMode::Present,
    );

    assert_eq!(
        steps,
        vec![RenderStepDescriptor::Draw(DrawStepDescriptor {
            pipeline: blend_pipelines.solid_quads,
            resource_sets: resource_set_list([quad_set]),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: 0,
            first_instance: 0,
            scissor: None,
        })]
    );
}

#[test]
fn draw_steps_emit_custom_gpu_mesh_3d_step() {
    let mut upload = NovaFrameUpload::default();
    let shader_id = GpuMesh3dShaderId(3);
    let mesh_pipeline = test_render_pipeline_id(90);
    let mesh_set = test_resource_set_id(91);
    let mesh_indices_buffer = test_buffer_id(92);
    let mesh_id = GpuMesh3dId(5);
    let generation = 7;
    upload.batches.push(NovaUploadedBatch::CustomMesh3d {
        mesh_id,
        generation,
        shader_id,
        range: GpuMesh3dRange {
            start: 7,
            count: 12,
        },
        first_parameter_index: 2,
    });

    let pipelines = test_pipelines();
    let blend_pipelines = pipelines.alpha;
    let steps = draw_steps_for_upload(
        &upload,
        &pipelines,
        blend_pipelines,
        test_resource_set_id(10),
        test_resource_set_id(11),
        test_resource_set_id(12),
        |_| None,
        |id| (id == shader_id).then_some(mesh_pipeline),
        |id, generation| {
            (id == mesh_id && generation == 7).then_some(NovaMeshCacheEntry {
                generation,
                vertex_offset: 3,
                vertex_count: 20,
                index_offset: 100,
                index_count: 32,
            })
        },
        test_resource_set_id(13),
        test_resource_set_id(14),
        mesh_set,
        mesh_indices_buffer,
        NovaDrawStepMode::Present,
    );

    assert_eq!(
        steps,
        vec![RenderStepDescriptor::DrawIndexed(
            DrawIndexedStepDescriptor {
                pipeline: mesh_pipeline,
                resource_sets: resource_set_list([mesh_set]),
                index_buffer: IndexBufferBinding {
                    buffer: mesh_indices_buffer,
                    format: IndexFormat::Uint32,
                    offset: 0,
                },
                index_count: 12,
                first_index: 107,
                base_vertex: 3,
                instance_count: 1,
                first_instance: 2,
                scissor: None,
            }
        )]
    );
}

#[test]
fn backdrop_blur_source_steps_stop_at_first_blur_batch() {
    let mut upload = NovaFrameUpload::default();
    upload
        .batches
        .push(NovaUploadedBatch::Quads { first: 0, count: 1 });
    upload
        .batches
        .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });
    upload.batches.push(NovaUploadedBatch::MonoSprites {
        texture_id: AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Monochrome,
        },
        first: 0,
        count: 1,
    });
    let pipelines = test_pipelines();
    let blend_pipelines = pipelines.alpha;
    let quad_set = test_resource_set_id(10);

    let steps = draw_steps_for_upload(
        &upload,
        &pipelines,
        blend_pipelines,
        quad_set,
        test_resource_set_id(11),
        test_resource_set_id(12),
        |texture_id| {
            test_sprite_resource_set(
                texture_id,
                test_resource_set_id(13),
                test_resource_set_id(14),
            )
        },
        |_| None,
        |_, _| None,
        test_resource_set_id(15),
        test_resource_set_id(16),
        test_resource_set_id(17),
        test_buffer_id(18),
        NovaDrawStepMode::BackdropSource,
    );

    assert_eq!(
        steps,
        vec![RenderStepDescriptor::Draw(DrawStepDescriptor {
            pipeline: blend_pipelines.quads,
            resource_sets: resource_set_list([quad_set]),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        })]
    );
}

#[test]
fn present_draw_steps_continue_after_backdrop_blur_batch() {
    let mut upload = NovaFrameUpload::default();
    upload
        .batches
        .push(NovaUploadedBatch::Quads { first: 0, count: 1 });
    upload
        .batches
        .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });
    upload.batches.push(NovaUploadedBatch::MonoSprites {
        texture_id: AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Monochrome,
        },
        first: 0,
        count: 1,
    });
    let pipelines = test_pipelines();
    let blend_pipelines = pipelines.alpha;

    let steps = draw_steps_for_upload(
        &upload,
        &pipelines,
        blend_pipelines,
        test_resource_set_id(10),
        test_resource_set_id(11),
        test_resource_set_id(12),
        |texture_id| {
            test_sprite_resource_set(
                texture_id,
                test_resource_set_id(13),
                test_resource_set_id(14),
            )
        },
        |_| None,
        |_, _| None,
        test_resource_set_id(15),
        test_resource_set_id(16),
        test_resource_set_id(17),
        test_buffer_id(18),
        NovaDrawStepMode::Present,
    );

    assert_eq!(
        steps.iter().map(render_step_pipeline).collect::<Vec<_>>(),
        vec![
            blend_pipelines.quads,
            blend_pipelines.backdrop_blurs,
            blend_pipelines.mono_sprites
        ]
    );
}

#[test]
fn partial_render_plan_produces_dirty_region_scissor() {
    let scene = crate::Scene::default();
    let mut dirty_region = crate::DirtyRegion::empty();
    dirty_region.push(crate::bounds(
        Point {
            x: crate::ScaledPixels(10.25),
            y: crate::ScaledPixels(20.75),
        },
        size(crate::ScaledPixels(30.1), crate::ScaledPixels(40.1)),
    ));
    dirty_region.push(crate::bounds(
        Point {
            x: crate::ScaledPixels(60.0),
            y: crate::ScaledPixels(70.0),
        },
        size(crate::ScaledPixels(10.0), crate::ScaledPixels(10.0)),
    ));
    let render_plan = FrameRenderPlan {
        scene: &scene,
        dirty_region: &dirty_region,
        partial_present_mode: PartialPresentMode::Partial,
        trim_policy: Default::default(),
        visual_effect_quality: FrameVisualEffectQuality::Full,
    };

    assert_eq!(
        partial_scissor_for_plan(
            render_plan,
            DrawableSize {
                width: 100,
                height: 100,
            },
        ),
        Some(ScissorRect {
            x: 10,
            y: 20,
            width: 60,
            height: 60,
        })
    );
}

#[test]
fn full_render_plan_does_not_produce_scissor() {
    let scene = crate::Scene::default();
    let dirty_region = crate::DirtyRegion::full(crate::bounds(
        Point {
            x: crate::ScaledPixels(0.0),
            y: crate::ScaledPixels(0.0),
        },
        size(crate::ScaledPixels(100.0), crate::ScaledPixels(100.0)),
    ));
    let render_plan = FrameRenderPlan::full_redraw(&scene, &dirty_region);

    assert_eq!(
        partial_scissor_for_plan(
            render_plan,
            DrawableSize {
                width: 100,
                height: 100,
            },
        ),
        None
    );
}

#[test]
fn surface_resize_forces_full_redraw_plan() {
    let scene = crate::Scene::default();
    let mut dirty_region = crate::DirtyRegion::empty();
    dirty_region.push(crate::bounds(
        Point {
            x: crate::ScaledPixels(10.0),
            y: crate::ScaledPixels(10.0),
        },
        size(crate::ScaledPixels(20.0), crate::ScaledPixels(20.0)),
    ));
    let partial_plan = FrameRenderPlan {
        scene: &scene,
        dirty_region: &dirty_region,
        partial_present_mode: PartialPresentMode::Partial,
        trim_policy: Default::default(),
        visual_effect_quality: FrameVisualEffectQuality::Disabled,
    };

    assert_eq!(
        resolve_surface_render_plan(partial_plan, true).partial_present_mode,
        PartialPresentMode::FullRedraw
    );
    assert_eq!(
        resolve_surface_render_plan(partial_plan, true).visual_effect_quality,
        FrameVisualEffectQuality::Disabled
    );
    assert_eq!(
        resolve_surface_render_plan(partial_plan, false).partial_present_mode,
        PartialPresentMode::Partial
    );
}

#[test]
fn backdrop_blur_render_passes_downsample_then_upsample_levels() {
    let pipelines = test_pipelines();
    let targets = NovaBackdropBlurTargets {
        downsample: 2,
        source: NovaTextureTarget {
            texture: test_texture_id(1),
            texture_view: test_texture_view_id(1),
        },
        levels: vec![
            NovaBackdropBlurLevelTarget {
                texture: test_texture_id(2),
                texture_view: test_texture_view_id(2),
                pass_resource_set: test_resource_set_id(12),
            },
            NovaBackdropBlurLevelTarget {
                texture: test_texture_id(3),
                texture_view: test_texture_view_id(3),
                pass_resource_set: test_resource_set_id(13),
            },
            NovaBackdropBlurLevelTarget {
                texture: test_texture_id(4),
                texture_view: test_texture_view_id(4),
                pass_resource_set: test_resource_set_id(14),
            },
        ],
        source_pass_resource_set: test_resource_set_id(11),
        target_resource_set: test_resource_set_id(15),
    };

    let passes = backdrop_blur_render_passes_for_targets(&pipelines, &targets, 3);

    assert_eq!(passes.len(), 5);
    assert_eq!(
        passes
            .iter()
            .map(|pass| pass.target_texture_view)
            .collect::<Vec<_>>(),
        vec![
            test_texture_view_id(2),
            test_texture_view_id(3),
            test_texture_view_id(4),
            test_texture_view_id(3),
            test_texture_view_id(2),
        ]
    );
    assert_eq!(
        passes
            .iter()
            .map(|pass| pass.step.pipeline)
            .collect::<Vec<_>>(),
        vec![
            pipelines.backdrop_blur_downsample,
            pipelines.backdrop_blur_downsample,
            pipelines.backdrop_blur_downsample,
            pipelines.backdrop_blur_upsample,
            pipelines.backdrop_blur_upsample,
        ]
    );
    assert_eq!(
        passes
            .iter()
            .map(|pass| pass.step.resource_sets.as_slice())
            .collect::<Vec<_>>(),
        vec![
            [test_resource_set_id(11)].as_slice(),
            [test_resource_set_id(12)].as_slice(),
            [test_resource_set_id(13)].as_slice(),
            [test_resource_set_id(14)].as_slice(),
            [test_resource_set_id(13)].as_slice(),
        ]
    );
}

#[test]
fn backdrop_blur_render_passes_are_empty_without_levels() {
    let pipelines = test_pipelines();
    let targets = NovaBackdropBlurTargets {
        downsample: 2,
        source: NovaTextureTarget {
            texture: test_texture_id(1),
            texture_view: test_texture_view_id(1),
        },
        levels: Vec::new(),
        source_pass_resource_set: test_resource_set_id(11),
        target_resource_set: test_resource_set_id(15),
    };

    assert!(backdrop_blur_render_passes_for_targets(&pipelines, &targets, 3).is_empty());
}

#[test]
fn nova_production_shader_entries_compile_for_enabled_backends() {
    #[cfg(all(
        feature = "nova-gfx-vulkan",
        any(target_os = "windows", target_os = "linux", target_os = "freebsd")
    ))]
    compile_nova_shader_binaries(compile_wgsl_to_spirv)
        .expect("nova production shaders should compile to SPIR-V");

    #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
    compile_nova_shader_binaries(compile_wgsl_to_hlsl)
        .expect("nova production shaders should compile to HLSL");

    #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
    compile_nova_shader_binaries(compile_wgsl_to_msl)
        .expect("nova production shaders should compile to MSL");
}

#[test]
fn nova_optional_shader_entries_compile_for_enabled_backends() {
    fn compile_optional_entries(
        mut compile: impl FnMut(
            &str,
            ShaderStage,
            &str,
        )
            -> std::result::Result<gfx_core::ShaderBinary, gfx_shader::ShaderError>,
        target: &str,
    ) {
        let entries = [
            (
                NOVA_SURFACE_SHADER_SOURCE,
                ShaderStage::Vertex,
                "vs_surface",
            ),
            (
                NOVA_SURFACE_SHADER_SOURCE,
                ShaderStage::Fragment,
                "fs_surface",
            ),
        ];

        for (source, stage, entry_point) in entries {
            compile(source, stage, entry_point).unwrap_or_else(|error| {
                panic!(
                    "nova optional shader entry {entry_point} should compile to {target}: {error}"
                );
            });
        }
    }

    #[cfg(all(
        feature = "nova-gfx-vulkan",
        any(target_os = "windows", target_os = "linux", target_os = "freebsd")
    ))]
    compile_optional_entries(compile_wgsl_to_spirv, "SPIR-V");

    #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
    compile_optional_entries(compile_wgsl_to_hlsl, "HLSL");

    #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
    compile_optional_entries(compile_wgsl_to_msl, "MSL");
}

#[test]
fn nova_shaders_do_not_guard_division_with_select() {
    for (shader_name, source) in nova_shader_sources() {
        let source_without_comments = source
            .lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n");

        for statement in source_without_comments.split(';') {
            let compact_statement = statement.split_whitespace().collect::<String>();
            assert!(
                !select_arguments_contain_division(&compact_statement),
                "{shader_name} contains a select() guarded division; use an explicit branch instead: {statement}"
            );
        }
    }
}

fn select_arguments_contain_division(statement: &str) -> bool {
    let mut search_start = 0;
    while let Some(relative_start) = statement[search_start..].find("select(") {
        let select_start = search_start + relative_start;
        let mut depth = 0_i32;
        for (offset, character) in statement[select_start..].char_indices() {
            match character {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        let select_expression = &statement[select_start..select_start + offset];
                        if select_expression.contains('/') {
                            return true;
                        }
                        search_start = select_start + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }

        if search_start == select_start {
            return false;
        }
    }

    false
}

#[test]
fn nova_shaders_use_explicit_lod_for_texture_sampling() {
    for (shader_name, source) in nova_shader_sources() {
        assert!(
            !source.contains("textureSample("),
            "{shader_name} uses implicit texture sampling; use textureSampleLevel(..., 0.0)"
        );
    }
}

#[test]
fn nova_fragment_shaders_clip_before_expensive_fragment_work() {
    for (shader_name, source) in nova_shader_sources() {
        let mut search_start = 0;
        while let Some(relative_start) = source[search_start..].find("@fragment") {
            let fragment_start = search_start + relative_start;
            let function_start = source[fragment_start..]
                .find("fn ")
                .map(|offset| fragment_start + offset)
                .unwrap_or_else(|| panic!("{shader_name} fragment entry is missing fn"));
            let function_name_start = function_start + "fn ".len();
            let function_name_end = source[function_name_start..]
                .find('(')
                .map(|offset| function_name_start + offset)
                .unwrap_or_else(|| panic!("{shader_name} fragment entry is missing argument list"));
            let function_name = &source[function_name_start..function_name_end];
            let function_source = shader_function_source(shader_name, source, function_name);

            if let Some(clip_offset) =
                function_source.find("if (any(input.clip_distances < vec4<f32>(0.0)))")
            {
                for expensive_fragment in [
                    "dpdx(",
                    "dpdy(",
                    "textureSampleLevel(",
                    "sample_backdrop_blur_texture(",
                ] {
                    if let Some(expensive_offset) = function_source.find(expensive_fragment) {
                        assert!(
                            clip_offset < expensive_offset,
                            "{shader_name}::{function_name} should clip before {expensive_fragment}"
                        );
                    }
                }
                for storage_buffer in storage_buffers_declared_by_shader(source) {
                    let storage_read = format!("{storage_buffer}[");
                    if let Some(storage_offset) = function_source.find(&storage_read) {
                        assert!(
                            clip_offset < storage_offset,
                            "{shader_name}::{function_name} should clip before reading {storage_buffer}"
                        );
                    }
                }
            }

            search_start = function_start + function_source.len();
        }
    }
}

#[test]
fn nova_fragment_shaders_skip_work_before_sampling_transparent_pixels() {
    assert_fragment_contains_in_order(
        "mono_sprite.wgsl",
        include_str!("shaders/mono_sprite.wgsl"),
        "fs_mono_sprite",
        &[
            "if (any(input.clip_distances < vec4<f32>(0.0)))",
            "if (input.color.a <= 0.0)",
            "let sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).r",
            "if (sample <= 0.0)",
            "let alpha_corrected = apply_contrast_and_gamma_correction(",
        ],
    );
    assert_fragment_contains_in_order(
        "poly_sprite.wgsl",
        include_str!("shaders/poly_sprite.wgsl"),
        "fs_poly_sprite",
        &[
            "if (any(input.clip_distances < vec4<f32>(0.0)))",
            "if (input.opacity <= 0.0)",
            "quad_sdf_from_packed(input.position.xy, input.bounds, input.corner_radii)",
            "let coverage = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
            "if (coverage <= 0.0)",
            "let sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0)",
            "if (sample.a <= 0.0)",
            "let grayscale = dot(sample.rgb, GRAYSCALE_FACTORS)",
        ],
    );
    assert_fragment_contains_in_order(
        "backdrop_blur.wgsl",
        include_str!("shaders/backdrop_blur.wgsl"),
        "fs_backdrop_blur",
        &[
            "if (any(input.clip_distances < vec4<f32>(0.0)))",
            "let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
            "if (alpha <= 0.0)",
            "var color = sample_backdrop_blur_texture(input.texture_coords)",
            "if (color.a <= 0.0 && input.tint.a <= 0.0)",
            "if (input.saturation != 1.0)",
            "color = vec4<f32>(saturate_color(color.rgb, input.saturation), color.a)",
        ],
    );
}

#[test]
fn nova_fragment_shaders_skip_fully_transparent_instances() {
    assert_fragment_contains_in_order(
        "solid_quad.wgsl",
        include_str!("shaders/solid_quad.wgsl"),
        "fs_solid_quad",
        &[
            "if (any(input.clip_distances < vec4<f32>(0.0)))",
            "if (input.color.a <= 0.0)",
            "return blend_color(input.color, 1.0)",
        ],
    );
    assert_fragment_contains_in_order(
        "quad.wgsl",
        include_str!("shaders/quad.wgsl"),
        "fs_quad",
        &[
            "if (input.background_tag == 0u &&",
            "input.background_solid.a <= 0.0 &&",
            "input.border_color.a <= 0.0)",
            "let quad = b_quads[input.quad_id]",
            "var background_color = input.background_solid",
            "if (background_color.a <= 0.0 && input.border_color.a <= 0.0)",
        ],
    );
    assert_fragment_contains_in_order(
        "path.wgsl",
        include_str!("shaders/path.wgsl"),
        "fs_path_rasterization",
        &[
            "var color = input.background_solid",
            "if (input.background_tag == 0u && color.a <= 0.0)",
            "if (input.background_tag != 0u)",
            "if (color.a <= 0.0)",
            "return vec4<f32>(color.rgb * color.a * alpha, color.a * alpha)",
        ],
    );
    assert_fragment_contains_in_order(
        "shadow.wgsl",
        include_str!("shaders/shadow.wgsl"),
        "fs_shadow",
        &[
            "if (any(input.clip_distances < vec4<f32>(0.0)))",
            "if (input.color.a <= 0.0)",
            "if (input.blur_radius <= SHADER_EPSILON)",
            "let inverse_sigma = 1.0 / input.blur_radius",
        ],
    );
    assert_fragment_contains_in_order(
        "underline.wgsl",
        include_str!("shaders/underline.wgsl"),
        "fs_underline",
        &[
            "if (any(input.clip_distances < vec4<f32>(0.0)))",
            "if (input.color.a <= 0.0)",
            "let underline_height = input.bounds.w",
            "if ((input.wavy & 0xFFu) == 0u)",
            "return blend_color(input.color, 1.0)",
        ],
    );
}

#[test]
fn nova_shader_discard_usage_matches_clip_strategy() {
    assert_shader_contains(
        "core.wgsl",
        include_str!("shaders/core.wgsl"),
        &[
            "Most Nova shaders pass software clip distances to the fragment stage",
            "return transparent outside the clip",
        ],
    );

    for (shader_name, source) in nova_shader_sources() {
        assert!(
            !source.contains("discard;"),
            "{shader_name} should return transparent for software clipping instead of discarding"
        );
    }
}

#[test]
fn nova_underline_alpha_is_applied_once() {
    let source = include_str!("shaders/underline.wgsl");
    assert_fragment_contains_in_order(
        "underline.wgsl",
        source,
        "fs_underline",
        &[
            "if ((input.wavy & 0xFFu) == 0u)",
            "return blend_color(input.color, 1.0)",
            "let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - stroke_distance)",
            "return blend_color(input.color, alpha)",
        ],
    );
    assert_fragment_function_omits("underline.wgsl", source, "fs_underline", "input.color.a)");
}

#[test]
fn nova_quad_struct_definitions_stay_in_sync() {
    let quad_struct = shader_struct_source("quad.wgsl", include_str!("shaders/quad.wgsl"), "Quad");
    let solid_quad_struct = shader_struct_source(
        "solid_quad.wgsl",
        include_str!("shaders/solid_quad.wgsl"),
        "Quad",
    );

    assert_eq!(
        solid_quad_struct, quad_struct,
        "solid_quad.wgsl and quad.wgsl must agree on the packed Quad buffer layout"
    );
    assert_shader_contains(
        "quad.wgsl",
        include_str!("shaders/quad.wgsl"),
        &["Keep in sync with solid_quad.wgsl"],
    );
    assert_shader_contains(
        "solid_quad.wgsl",
        include_str!("shaders/solid_quad.wgsl"),
        &["Keep in sync with quad.wgsl"],
    );
}

#[test]
fn nova_sdf_and_dash_thresholds_are_shared_constants() {
    assert_shader_contains(
        "core.wgsl",
        include_str!("shaders/core.wgsl"),
        &["const SDF_ANTIALIAS_THRESHOLD: f32 = 0.5"],
    );
    assert_shader_contains(
        "quad.wgsl",
        include_str!("shaders/quad.wgsl"),
        &[
            "const DASH_PERIOD_PER_WIDTH: f32 = DASH_LENGTH_PER_WIDTH + DASH_GAP_PER_WIDTH",
            "const DASH_LENGTH: f32 = DASH_LENGTH_PER_WIDTH / DASH_PERIOD_PER_WIDTH",
            "const DASH_VELOCITY_NUMERATOR: f32 = 1.0 / DASH_PERIOD_PER_WIDTH",
        ],
    );

    for (shader_name, source) in nova_shader_sources() {
        for forbidden_fragment in [
            "saturate(0.5 -",
            "let antialias_threshold = 0.5",
            "dash_period_per_width",
            "desired_dash_gap",
        ] {
            assert!(
                !source.contains(forbidden_fragment),
                "{shader_name} should use shared shader constants instead of {forbidden_fragment}"
            );
        }
    }
}

#[test]
fn nova_shader_divisions_are_guarded_or_constant() {
    const ALLOWED_DIVISIONS: &[&str] = &[
        "/ vec3<f32>(1.055)",
        "/ vec3<f32>(12.92)",
        "/ 1.055",
        "/ 12.92",
        "1.0 / 2.4",
        "1.0 / 3.0",
        "* M_PI_F / 180.0",
        "safe_size.y / safe_size.x",
        "safe_size.x / safe_size.y",
        "/ max(length(direction), SHADER_EPSILON)",
        "/ max(length(scaled_direction), SHADER_EPSILON)",
        "/ safe_size.x",
        "/ safe_size.y",
        "/ stop_range",
        "/ 65535.0f",
        "/ 255.0f",
        "M_PI_F / 4.0",
        "/ max(length(gradient), SHADER_EPSILON)",
        "pattern_width / pattern_height",
        "1.0 / source_size",
        "/ max(blur.blurred_size, vec2<f32>(1.0))",
        "/ 2.0",
        "1.0 / input.blur_radius",
        "/ sqrt(2.0 * M_PI_F)",
        "/ (r2 * r2)",
        "/ 4.0",
        "1.0 / DASH_PERIOD_PER_WIDTH",
        "/ DASH_PERIOD_PER_WIDTH",
        "/ dash_count",
        "/ safe_border_width",
        "/ 2",
        "/ 2.0)",
        "/ dash_velocity",
        "/ safe_radii",
        "/ b)",
        "/ viewport_size",
        "/ atlas_size",
        "/ alpha",
        "/ max(clip_position.w, 0.0001)",
        "/ max(globals.viewport_size, vec2<f32>(1.0))",
        "/ max(safe_alpha * safe_k + 1.0, SHADER_EPSILON)",
        "/ underline_height",
        "/ sqrt(1.0 + dSine * dSine)",
    ];

    for (shader_name, source) in nova_shader_sources() {
        for (line_number, line) in source.lines().enumerate() {
            let code = line.split_once("//").map_or(line, |(code, _)| code);
            if code.contains('/') {
                assert!(
                    ALLOWED_DIVISIONS
                        .iter()
                        .any(|division| code.contains(division)),
                    "{shader_name}:{} uses division without an explicit safety review: {line}",
                    line_number + 1
                );
            }
        }
    }
}

#[test]
fn nova_shader_modulo_uses_have_nonzero_divisors() {
    const ALLOWED_MODULO_DIVISORS: [&str; 4] =
        ["% 2.0", "% 360.0", "% 65535.0f", "% pattern_period"];

    assert_shader_contains(
        "quad_common.wgsl",
        include_str!("shaders/quad_common.wgsl"),
        &[
            "if (pattern_width <= SHADER_EPSILON || pattern_height <= SHADER_EPSILON || pattern_period <= SHADER_EPSILON)",
            "rotated_point.x % pattern_period",
        ],
    );

    for (shader_name, source) in nova_shader_sources() {
        for (line_number, line) in source.lines().enumerate() {
            let code = line.split_once("//").map_or(line, |(code, _)| code);
            if code.contains('%') {
                assert!(
                    ALLOWED_MODULO_DIVISORS
                        .iter()
                        .any(|divisor| code.contains(divisor)),
                    "{shader_name}:{} uses modulo without a known nonzero divisor: {line}",
                    line_number + 1
                );
            }
        }
    }
}

#[test]
fn nova_shader_edge_guards_cover_degenerate_inputs() {
    assert_shader_contains(
        "core.wgsl",
        include_str!("shaders/core.wgsl"),
        &[
            "const SHADER_EPSILON",
            "const SDF_ANTIALIAS_THRESHOLD",
            "let viewport_size = max(globals.viewport_size, vec2<f32>(1.0))",
            "fn viewport_texture_coords",
            "let atlas_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0))",
            "fn over",
            "if (alpha <= SHADER_EPSILON)",
            "return vec4<f32>(0.0)",
        ],
    );
    assert_shader_contains(
        "quad.wgsl",
        include_str!("shaders/quad.wgsl"),
        &[
            "dash_velocity_for_border_width(DASH_VELOCITY_NUMERATOR, border_width)",
            "fn dash_velocity_for_border_width",
            "let safe_border_width = max(border_width, SHADER_EPSILON)",
            "let velocity = dv_numerator / safe_border_width",
            "return select(0.0, velocity, has_border)",
            "const DASH_PERIOD_PER_WIDTH",
            "const DASH_LENGTH",
            "const DASH_VELOCITY_NUMERATOR",
            "let antialias_threshold = SDF_ANTIALIAS_THRESHOLD",
            "let dv1_or_min = select(min_nonzero_velocity, dv1, dv2 == 0.0)",
            "if (dash_velocity <= SHADER_EPSILON || period <= SHADER_EPSILON || length <= SHADER_EPSILON)",
            "let safe_radii = max(radii, vec2<f32>(SHADER_EPSILON))",
            "let outer_alpha = saturate(antialias_threshold - outer_sdf)",
            "if (outer_alpha <= 0.0)",
            "return blend_color(color, outer_alpha)",
        ],
    );
    assert_shader_contains(
        "quad_common.wgsl",
        include_str!("shaders/quad_common.wgsl"),
        &[
            "let safe_size = max(bounds.size, vec2<f32>(SHADER_EPSILON))",
            "let x_over_y = safe_size.x / safe_size.y",
            "let y_over_x = safe_size.y / safe_size.x",
            "let scaled_direction = vec2<f32>",
            "if (abs(stop_range) <= SHADER_EPSILON)",
            "if (pattern_width <= SHADER_EPSILON || pattern_height <= SHADER_EPSILON || pattern_period <= SHADER_EPSILON)",
            "background_color.a *= saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
        ],
    );
    assert_shader_contains(
        "shadow.wgsl",
        include_str!("shaders/shadow.wgsl"),
        &[
            "if (input.blur_radius <= SHADER_EPSILON)",
            "pick_corner_radius_from_packed(center_to_point, input.corner_radii)",
            "if (end <= start)",
            "let inverse_sigma = 1.0 / input.blur_radius",
            "let gaussian_scale = inverse_sigma / sqrt(2.0 * M_PI_F)",
            "let gaussian_exponent_scale = 0.5 * inverse_sigma * inverse_sigma",
            "gaussian_weight(y, gaussian_scale, gaussian_exponent_scale)",
        ],
    );
    assert_fragment_function_omits(
        "shadow.wgsl",
        include_str!("shaders/shadow.wgsl"),
        "fs_shadow",
        "gaussian(y, input.blur_radius)",
    );
    assert_shader_contains(
        "underline.wgsl",
        include_str!("shaders/underline.wgsl"),
        &[
            "if (underline_height <= SHADER_EPSILON || input.thickness <= SHADER_EPSILON)",
            "let stroke_distance = max(-distance_from_bottom_border, distance_from_top_border)",
            "let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - stroke_distance)",
        ],
    );
    assert_shader_contains(
        "backdrop_blur.wgsl",
        include_str!("shaders/backdrop_blur.wgsl"),
        &[
            "let source_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0))",
            "screen_position / max(blur.blurred_size, vec2<f32>(1.0))",
            "quad_sdf_from_packed(input.position.xy, input.bounds, input.corner_radii)",
            "let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
            "if (alpha <= 0.0)",
            "textureSampleLevel(t_sprite, s_sprite, texture_coords, 0.0)",
        ],
    );
    assert_shader_contains(
        "path.wgsl",
        include_str!("shaders/path.wgsl"),
        &[
            "let edge_gradient = vec2<f32>(dx.x, dy.x)",
            "if (length(edge_gradient) < 0.001)",
            "let distance = f / max(length(gradient), SHADER_EPSILON)",
            "alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
            "if (alpha <= 0.0)",
            "let texture_coords = viewport_texture_coords(screen_position)",
        ],
    );
    assert_shader_contains(
        "text.wgsl",
        include_str!("shaders/text.wgsl"),
        &[
            "let safe_alpha = saturate(alpha)",
            "return safe_alpha * (safe_k + 1.0) / max(safe_alpha * safe_k + 1.0, SHADER_EPSILON)",
        ],
    );
}

#[test]
fn nova_fragment_shaders_avoid_redundant_instance_ssbo_reads() {
    // `fs_quad` still reads `b_quads` because it needs the full quad shape,
    // border, and background metadata. Small preconverted colors are already
    // passed as flat varyings to keep this reviewed exception narrow.
    let allowed_fragment_reads = [("quad.wgsl", "fs_quad", "b_quads")];
    assert_fragment_storage_reads_are_allowlisted(&allowed_fragment_reads);

    assert_fragment_function_omits(
        "backdrop_blur.wgsl",
        include_str!("shaders/backdrop_blur.wgsl"),
        "fs_backdrop_blur",
        "b_backdrop_blurs",
    );
    assert_fragment_function_omits(
        "path.wgsl",
        include_str!("shaders/path.wgsl"),
        "fs_path_rasterization",
        "b_path_vertices",
    );
    assert_fragment_function_omits(
        "poly_sprite.wgsl",
        include_str!("shaders/poly_sprite.wgsl"),
        "fs_poly_sprite",
        "b_poly_sprites",
    );
    assert_fragment_function_omits(
        "shadow.wgsl",
        include_str!("shaders/shadow.wgsl"),
        "fs_shadow",
        "b_shadows",
    );
    assert_fragment_function_omits(
        "underline.wgsl",
        include_str!("shaders/underline.wgsl"),
        "fs_underline",
        "b_underlines",
    );
}

fn assert_fragment_storage_reads_are_allowlisted(allowed_reads: &[(&str, &str, &str)]) {
    for (shader_name, source) in nova_shader_sources() {
        let mut search_start = 0;
        while let Some(relative_start) = source[search_start..].find("@fragment") {
            let fragment_start = search_start + relative_start;
            let function_start = source[fragment_start..]
                .find("fn ")
                .map(|offset| fragment_start + offset)
                .unwrap_or_else(|| panic!("{shader_name} fragment entry is missing fn"));
            let function_name_start = function_start + "fn ".len();
            let function_name_end = source[function_name_start..]
                .find('(')
                .map(|offset| function_name_start + offset)
                .unwrap_or_else(|| panic!("{shader_name} fragment entry is missing argument list"));
            let function_name = &source[function_name_start..function_name_end];
            let function_end = source[function_start + 1..]
                .find("\n@")
                .map(|offset| function_start + 1 + offset)
                .unwrap_or(source.len());
            let function_source = &source[function_start..function_end];

            for storage_buffer in storage_buffers_declared_by_shader(source) {
                if function_source.contains(&format!("{storage_buffer}[")) {
                    assert!(
                        allowed_reads.iter().any(
                            |(allowed_shader, allowed_function, allowed_buffer)| {
                                *allowed_shader == shader_name
                                    && *allowed_function == function_name
                                    && *allowed_buffer == storage_buffer
                            }
                        ),
                        "{shader_name}::{function_name} reads {storage_buffer} in the fragment stage; pass small per-instance fields through flat varyings or add a reviewed exception"
                    );
                }
            }

            search_start = function_end;
        }
    }
}

fn storage_buffers_declared_by_shader(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter_map(|line| {
            let code = line.split_once("//").map_or(line, |(code, _)| code);
            if !code.contains("var<storage") {
                return None;
            }
            code.split_once("var<storage")
                .and_then(|(_, rest)| rest.split_once('>'))
                .and_then(|(_, rest)| rest.trim_start().split_once(':'))
                .map(|(name, _)| name.trim())
        })
        .collect()
}

#[test]
fn nova_quad_has_solid_background_fast_path() {
    assert_fragment_contains_in_order(
        "quad.wgsl",
        include_str!("shaders/quad.wgsl"),
        "fs_quad",
        &[
            "var background_color = input.background_solid",
            "if (input.background_tag != 0u)",
            "background_color = gradient_color(",
        ],
    );
    assert_shader_contains(
        "quad.wgsl",
        include_str!("shaders/quad.wgsl"),
        &[
            "@location(6) @interpolate(flat) background_tag: u32",
            "out.background_solid = gradient.solid",
            "out.background_color0 = gradient.color0",
            "out.background_color1 = gradient.color1",
            "out.background_tag = quad.background.tag",
        ],
    );
}

#[test]
fn nova_path_rasterization_has_solid_background_fast_path() {
    assert_shader_contains(
        "path.wgsl",
        include_str!("shaders/path.wgsl"),
        &[
            "let prepared_color = prepare_gradient_color(",
            "var color = input.background_solid",
            "if (input.background_tag != 0u)",
        ],
    );
    assert_fragment_contains_in_order(
        "path.wgsl",
        include_str!("shaders/path.wgsl"),
        "fs_path_rasterization",
        &[
            "alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
            "if (alpha <= 0.0)",
            "var color = input.background_solid",
            "if (input.background_tag != 0u)",
            "let background = Background(",
        ],
    );
    assert_fragment_function_omits(
        "path.wgsl",
        include_str!("shaders/path.wgsl"),
        "fs_path_rasterization",
        "prepare_gradient_color(",
    );
}

#[test]
fn nova_vertex_only_storage_buffers_are_not_visible_to_fragment_stage() {
    let renderer_source = nova_runtime_source();
    for binding in [2_u32, 3, 6, 7, 8, 9, 15, 16] {
        let binding_marker = format!("binding: {binding},");
        let entry_source = renderer_source
            .match_indices(&binding_marker)
            .filter_map(|(binding_start, _)| {
                let binding_source = &renderer_source[binding_start..];
                let entry_end = binding_source
                    .find("},")
                    .map_or(binding_source.len(), |end| end + 2);
                let entry_source = &binding_source[..entry_end];

                entry_source
                    .contains("ResourceBindingType::StorageBuffer")
                    .then_some(entry_source)
            })
            .next()
            .unwrap_or_else(|| {
                panic!("nova runtime source is missing storage buffer layout binding {binding}")
            });

        assert!(
            entry_source.contains("stages: ShaderStages::VERTEX"),
            "binding {binding} should only be visible to the vertex stage"
        );
        assert!(
            !entry_source.contains("ShaderStages::FRAGMENT"),
            "binding {binding} should not be visible to the fragment stage"
        );
    }
}

#[test]
fn nova_wgsl_files_are_covered_by_build_validation() {
    let shader_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/platform/nova/shaders");
    let build_script = include_str!("../../../build.rs");
    let renderer_source = nova_runtime_source();
    let mut shader_names = Vec::new();

    for shader_entry in std::fs::read_dir(&shader_dir)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", shader_dir.display()))
    {
        let shader_entry =
            shader_entry.unwrap_or_else(|error| panic!("failed to read shader dir entry: {error}"));
        let shader_path = shader_entry.path();
        if shader_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("wgsl")
        {
            continue;
        }

        let shader_name = shader_path
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .expect("WGSL shader file names should be valid UTF-8");

        assert_ne!(
            shader_name, "basic_quad.wgsl",
            "basic_quad.wgsl is deprecated and should not live in the production Nova shader directory"
        );

        shader_names.push(shader_name.to_string());

        let build_validation_path = format!("./src/platform/nova/shaders/{shader_name}");
        assert!(
            build_script.contains(&build_validation_path),
            "{shader_name} is not covered by build.rs Nova WGSL validation"
        );

        let runtime_bundle_path = format!("include_str!(\"shaders/{shader_name}\")");
        assert!(
            renderer_source.contains(&runtime_bundle_path),
            "{shader_name} is not included by Nova runtime shader bundles"
        );
    }

    assert_shader_contains(
        "build.rs",
        build_script,
        &[
            "fn check_nova_wgsl_shader_coverage",
            "std::fs::read_dir(SHADER_DIR)",
            "basic_quad.wgsl is deprecated",
            "is not covered by build shader validation",
        ],
    );

    shader_names.sort_unstable();
    let mut tested_shader_names = nova_shader_sources()
        .into_iter()
        .map(|(shader_name, _)| shader_name.to_string())
        .collect::<Vec<_>>();
    tested_shader_names.sort_unstable();
    assert_eq!(
        shader_names, tested_shader_names,
        "nova_shader_sources() should cover every production Nova WGSL file"
    );
}

fn nova_runtime_source() -> String {
    [
        include_str!("../nova.rs"),
        include_str!("atlas_resources.rs"),
        include_str!("atlas.rs"),
        include_str!("backend.rs"),
        include_str!("diagnostics.rs"),
        include_str!("draw.rs"),
        include_str!("limits.rs"),
        include_str!("pipeline.rs"),
        include_str!("nova_renderer.rs"),
        include_str!("rendering_parameters.rs"),
        include_str!("resource_bindings.rs"),
        include_str!("resource_layouts.rs"),
        include_str!("resources.rs"),
        include_str!("shader.rs"),
        include_str!("surface.rs"),
        include_str!("swapchain.rs"),
        include_str!("targets.rs"),
        include_str!("upload_metrics.rs"),
        include_str!("upload_packing.rs"),
        include_str!("frame_upload.rs"),
    ]
    .join("\n")
}

fn nova_shader_sources() -> [(&'static str, &'static str); 13] {
    [
        (
            "backdrop_blur.wgsl",
            include_str!("shaders/backdrop_blur.wgsl"),
        ),
        ("core.wgsl", include_str!("shaders/core.wgsl")),
        ("mono_sprite.wgsl", include_str!("shaders/mono_sprite.wgsl")),
        ("path.wgsl", include_str!("shaders/path.wgsl")),
        ("poly_sprite.wgsl", include_str!("shaders/poly_sprite.wgsl")),
        ("quad.wgsl", include_str!("shaders/quad.wgsl")),
        ("quad_common.wgsl", include_str!("shaders/quad_common.wgsl")),
        ("shadow.wgsl", include_str!("shaders/shadow.wgsl")),
        ("shape.wgsl", include_str!("shaders/shape.wgsl")),
        ("solid_quad.wgsl", include_str!("shaders/solid_quad.wgsl")),
        ("surface.wgsl", include_str!("shaders/surface.wgsl")),
        ("text.wgsl", include_str!("shaders/text.wgsl")),
        ("underline.wgsl", include_str!("shaders/underline.wgsl")),
    ]
}

fn assert_shader_contains(shader_name: &str, source: &str, expected_fragments: &[&str]) {
    for expected_fragment in expected_fragments {
        assert!(
            source.contains(expected_fragment),
            "{shader_name} is missing edge guard fragment: {expected_fragment}"
        );
    }
}

fn shader_struct_source<'a>(shader_name: &str, source: &'a str, struct_name: &str) -> &'a str {
    let struct_marker = format!("struct {struct_name} {{");
    let struct_start = source
        .find(&struct_marker)
        .unwrap_or_else(|| panic!("{shader_name} is missing struct {struct_name}"));
    let struct_source = &source[struct_start..];
    let struct_end = struct_source
        .find("\n}")
        .map(|offset| offset + "\n}".len())
        .unwrap_or_else(|| panic!("{shader_name} struct {struct_name} is missing closing brace"));

    &struct_source[..struct_end]
}

fn assert_fragment_contains_in_order(
    shader_name: &str,
    source: &str,
    function_name: &str,
    expected_fragments: &[&str],
) {
    let function_source = shader_function_source(shader_name, source, function_name);
    let mut search_start = 0;
    for expected_fragment in expected_fragments {
        let fragment_offset = function_source[search_start..]
                .find(expected_fragment)
                .unwrap_or_else(|| {
                    panic!(
                        "{shader_name}::{function_name} is missing ordered fragment: {expected_fragment}"
                    )
                });
        search_start += fragment_offset + expected_fragment.len();
    }
}

fn assert_fragment_function_omits(
    shader_name: &str,
    source: &str,
    function_name: &str,
    forbidden_fragment: &str,
) {
    let function_source = shader_function_source(shader_name, source, function_name);

    assert!(
        !function_source.contains(forbidden_fragment),
        "{shader_name}::{function_name} should not read {forbidden_fragment} in fragment stage"
    );
}

fn shader_function_source<'a>(shader_name: &str, source: &'a str, function_name: &str) -> &'a str {
    let function_marker = format!("fn {function_name}(");
    let function_start = source
        .find(&function_marker)
        .unwrap_or_else(|| panic!("{shader_name} is missing {function_name}"));
    let function_source = &source[function_start..];
    let body_start = function_source
        .find('{')
        .unwrap_or_else(|| panic!("{shader_name}::{function_name} is missing a body"));
    let mut depth = 0_i32;
    for (offset, character) in function_source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &function_source[..body_start + offset + 1];
                }
            }
            _ => {}
        }
    }

    panic!("{shader_name}::{function_name} has an unterminated body");
}

#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
#[test]
fn nova_sprite_hlsl_bindings_match_dx12_resource_sets() {
    let mono_source = concat!(
        include_str!("shaders/core.wgsl"),
        include_str!("shaders/text.wgsl"),
        include_str!("shaders/mono_sprite.wgsl"),
    );
    let gfx_core::ShaderCode::Hlsl(mono_hlsl) =
        compile_wgsl_to_hlsl(mono_source, ShaderStage::Fragment, "fs_mono_sprite")
            .expect("mono sprite fragment shader should compile to HLSL")
            .code
    else {
        panic!("expected mono sprite HLSL");
    };
    assert!(
        mono_hlsl.contains("Texture2D<float4> t_sprite : register(t4)"),
        "unexpected mono sprite HLSL:\n{mono_hlsl}"
    );
    assert!(
        mono_hlsl.contains("SamplerState nagaSamplerHeap[2048]: register(s0, space0)"),
        "unexpected mono sprite HLSL:\n{mono_hlsl}"
    );
    assert!(
        mono_hlsl.contains("StructuredBuffer<uint> nagaGroup0SamplerIndexArray"),
        "unexpected mono sprite HLSL:\n{mono_hlsl}"
    );

    let gfx_core::ShaderCode::Hlsl(mono_vertex_hlsl) =
        compile_wgsl_to_hlsl(mono_source, ShaderStage::Vertex, "vs_mono_sprite")
            .expect("mono sprite vertex shader should compile to HLSL")
            .code
    else {
        panic!("expected mono sprite vertex HLSL");
    };
    assert!(
        mono_vertex_hlsl.contains("ByteAddressBuffer b_mono_sprites : register(t8)"),
        "unexpected mono sprite vertex HLSL:\n{mono_vertex_hlsl}"
    );
    assert!(
        mono_vertex_hlsl.contains("_NagaConstants.first_instance"),
        "DX12 sprite vertex shaders must offset instance_index by DrawStepDescriptor.first_instance:\n{mono_vertex_hlsl}"
    );

    let gfx_core::ShaderCode::Hlsl(poly_vertex_hlsl) = compile_wgsl_to_hlsl(
        NOVA_POLY_SPRITE_SHADER_SOURCE,
        ShaderStage::Vertex,
        "vs_poly_sprite",
    )
    .expect("poly sprite vertex shader should compile to HLSL")
    .code
    else {
        panic!("expected poly sprite vertex HLSL");
    };
    assert!(
        poly_vertex_hlsl.contains("ByteAddressBuffer b_poly_sprites : register(t9)"),
        "unexpected poly sprite vertex HLSL:\n{poly_vertex_hlsl}"
    );
}

fn test_atlas_tile() -> AtlasTile {
    AtlasTile {
        texture_id: AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Rgba,
        },
        tile_id: TileId(1),
        padding: 0,
        bounds: Bounds {
            origin: Point {
                x: DevicePixels(0),
                y: DevicePixels(0),
            },
            size: size(DevicePixels(1), DevicePixels(1)),
        },
    }
}

fn test_render_pipeline_id(index: u32) -> RenderPipelineId {
    RenderPipelineId::from_parts(index, 1)
}

fn test_blend_pipelines(base: u32) -> NovaBlendPipelines {
    NovaBlendPipelines {
        solid_quads: test_render_pipeline_id(base + 1),
        quads: test_render_pipeline_id(base + 2),
        shadows: test_render_pipeline_id(base + 3),
        mono_sprites: test_render_pipeline_id(base + 6),
        poly_sprites: test_render_pipeline_id(base + 7),
        underlines: test_render_pipeline_id(base + 8),
        backdrop_blurs: test_render_pipeline_id(base + 9),
    }
}

fn test_pipelines() -> NovaPipelines {
    NovaPipelines {
        alpha: test_blend_pipelines(0),
        premultiplied: test_blend_pipelines(100),
        path_rasterization: test_render_pipeline_id(4),
        paths: test_render_pipeline_id(5),
        present_copy: test_render_pipeline_id(8),
        backdrop_blur_downsample: test_render_pipeline_id(6),
        backdrop_blur_upsample: test_render_pipeline_id(7),
    }
}

fn test_resource_set_id(index: u32) -> ResourceSetId {
    ResourceSetId::from_parts(index, 1)
}

fn test_buffer_id(index: u32) -> BufferId {
    BufferId::from_parts(index, 1)
}

fn render_step_pipeline(step: &RenderStepDescriptor) -> RenderPipelineId {
    match step {
        RenderStepDescriptor::Draw(step) => step.pipeline,
        RenderStepDescriptor::DrawIndexed(step) => step.pipeline,
    }
}

fn test_texture_id(index: u32) -> TextureId {
    TextureId::from_parts(index, 1)
}

fn test_texture_view_id(index: u32) -> TextureViewId {
    TextureViewId::from_parts(index, 1)
}

fn backdrop_blur_scene(tint: Option<crate::Hsla>) -> crate::Scene {
    let mut scene = crate::Scene::default();
    let bounds = Bounds {
        origin: Point {
            x: crate::ScaledPixels(0.0),
            y: crate::ScaledPixels(0.0),
        },
        size: size(crate::ScaledPixels(64.0), crate::ScaledPixels(32.0)),
    };
    scene.insert_primitive(crate::PaintBackdropBlur {
        order: 0,
        animation_id: None,
        bounds,
        content_mask: crate::ContentMask { bounds },
        corner_radii: Default::default(),
        radius: crate::ScaledPixels(12.0),
        downsample: 2,
        levels: 3,
        saturation: 1.0,
        tint,
    });
    scene.finish();
    scene
}

fn read_u32_at(bytes: &[u8], offset: usize) -> u32 {
    let chunk = bytes
        .get(offset..offset + std::mem::size_of::<u32>())
        .expect("test offset should be in bounds");
    u32::from_ne_bytes(chunk.try_into().expect("u32 chunk should have exact size"))
}

fn read_f32_at(bytes: &[u8], offset: usize) -> f32 {
    let chunk = bytes
        .get(offset..offset + std::mem::size_of::<f32>())
        .expect("test offset should be in bounds");
    f32::from_ne_bytes(chunk.try_into().expect("f32 chunk should have exact size"))
}
