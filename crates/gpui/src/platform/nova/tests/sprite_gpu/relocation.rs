use super::*;

pub(super) fn verify_atlas_compaction<D>(
    device: &mut D,
    resources: &RendererResources,
    target: TextureId,
    view: TextureViewId,
) where
    D: BackendResources + BackendPresentationCompat + TextureTransferDevice,
{
    for kind in [AtlasTextureKind::Monochrome, AtlasTextureKind::Bgra] {
        let atlas = NovaAtlas::new();
        *atlas.state.lock().expect("atlas") = NovaAtlasState::default();
        let key = AtlasKey::Glyph(RenderGlyphParams {
            font_id: FontId(1),
            glyph_id: GlyphId(2),
            font_size: px(14.0),
            subpixel_variant: Point { x: 0, y: 0 },
            scale_factor: 1.0,
            grayscale_antialiasing: true,
            is_emoji: kind == AtlasTextureKind::Bgra,
            is_cjk: false,
        });
        assert_eq!(key.texture_kind(), kind);
        let pixels = if kind == AtlasTextureKind::Monochrome {
            vec![255; 256]
        } else {
            [20, 40, 80, 255].repeat(256)
        };
        let old = atlas
            .ensure_tile_with(key, &mut || {
                Ok(Some((
                    size(DevicePixels(16), DevicePixels(16)),
                    Cow::Borrowed(&pixels),
                )))
            })
            .expect("fixture allocation")
            .expect("tile");
        let descriptor = || AtlasResourceDescriptor {
            mono_sprite_resource_set_layout: resources.mono_sprite_resource_set_layout,
            poly_sprite_resource_set_layout: resources.poly_sprite_resource_set_layout,
            frame_buffers: vec![resources.frame_resources[0].buffers],
            sampler: resources.atlas_sampler,
        };
        let mut textures = FxHashMap::default();
        sync_gpu_atlas_textures(
            &atlas,
            &mut textures,
            device,
            "compact hardware gate",
            descriptor(),
        )
        .expect("source page");
        upload_pending_atlas(&atlas, device, |id| {
            textures
                .get(&id)
                .map(|texture| texture.texture)
                .context("resident source")
        })
        .expect("source pixels");
        let original = device
            .read_texture(textures[&old.texture_id].texture)
            .expect("source readback");
        // Scene owns an immutable old tile handle. Compaction must not require rebuilding it.
        let bounds = bounds(point(px(0.0), px(0.0)), size(px(16.0), px(16.0))).scale(1.0);
        let mask = crate::ContentMask {
            bounds,
            corner_bounds: bounds,
            ..Default::default()
        };
        let mut scene = crate::Scene::default();
        if kind == AtlasTextureKind::Monochrome {
            scene.insert_primitive(MonochromeSprite {
                order: 0,
                pad: 0,
                animation_id: None,
                bounds,
                content_mask: mask,
                color: crate::white().into(),
                tile: old,
                transformation: Default::default(),
            });
        } else {
            scene.insert_primitive(PolychromeSprite {
                order: 0,
                sampling: 0,
                grayscale: false,
                opacity: 1.0,
                animation_id: None,
                bounds,
                content_mask: mask,
                corner_radii: Default::default(),
                tile: old,
            });
        }
        scene.finish();
        assert!(
            renderer::compact::compact_atlas(
                &atlas,
                &mut textures,
                device,
                "compact hardware gate",
                &descriptor()
            )
            .expect("GPU atlas compact")
        );
        let mut upload = FrameUpload::default();
        atlas.copy_placements_into(&mut upload.atlas_placements);
        let relocated = upload.atlas_placements[&old.tile_id];
        assert_ne!(old.texture_id, relocated.texture_id);
        // Retire the source before readback/draw; the backend must preserve pending GPU copies.
        sync_gpu_atlas_textures(
            &atlas,
            &mut textures,
            device,
            "compact hardware gate",
            descriptor(),
        )
        .expect("source retirement");
        let destination = &textures[&relocated.texture_id];
        let copied = device
            .read_texture(destination.texture)
            .expect("compact pixels");
        let bpp = atlas_bytes_per_pixel(kind);
        for y in 0..18 {
            for x in 0..18 {
                let source_offset = (old.bounds.origin.y.0 as usize - 1 + y)
                    * original.bytes_per_row as usize
                    + (old.bounds.origin.x.0 as usize - 1 + x) * bpp;
                let destination_offset = (relocated.bounds.origin.y.0 as usize - 1 + y)
                    * copied.bytes_per_row as usize
                    + (relocated.bounds.origin.x.0 as usize - 1 + x) * bpp;
                assert_eq!(
                    &original.bytes[source_offset..source_offset + bpp],
                    &copied.bytes[destination_offset..destination_offset + bpp],
                    "{kind:?} preserves padded texel ({x},{y})"
                );
            }
        }
        upload.encode(
            &scene,
            &[],
            DrawableSize {
                width: 16,
                height: 16,
            },
            &RenderingParameters::from_env(),
            false,
            BackdropBlurQuality::Full,
        );
        let (buffer, bytes, pipeline, set) = if kind == AtlasTextureKind::Monochrome {
            assert_eq!(
                read_u32_at(&upload.mono_sprites, 88),
                relocated.texture_id.index
            );
            assert_eq!(
                read_u32_at(&upload.mono_sprites, 104),
                relocated.bounds.origin.x.0 as u32
            );
            assert!(
                matches!(upload.batches.as_slice(), [UploadedBatch::MonoSprites { texture_id, .. }] if *texture_id == relocated.texture_id)
            );
            (
                resources.frame_resources[0].buffers.mono_sprite_buffer,
                &upload.mono_sprites,
                resources.pipelines.alpha.mono_sprites,
                destination.mono_resource_sets[0],
            )
        } else {
            assert!(
                matches!(upload.batches.as_slice(), [UploadedBatch::PolySprites { texture_id, .. }] if *texture_id == relocated.texture_id)
            );
            (
                resources.frame_resources[0].buffers.poly_sprite_buffer,
                &upload.poly_sprites,
                resources.pipelines.alpha.poly_sprites,
                destination.poly_resource_sets[0],
            )
        };
        device
            .write_buffer(buffer, 0, bytes)
            .expect("relocated scene packing");
        let step = DrawStepDescriptor {
            pipeline,
            resource_sets: resource_set_list([set]),
            vertex_count: 4,
            instance_count: 1,
            first_vertex: 0,
            first_instance: 0,
            scissor: None,
        };
        device
            .render_step_list_to_texture_compat(
                view,
                resources.render_pass,
                gfx_core::RenderStepList::Draw(&[step]),
                LoadOp::Clear(ClearColor {
                    red: 0.0,
                    green: 0.0,
                    blue: 0.0,
                    alpha: 1.0,
                }),
                Some(gfx_core::RenderPassDepthAttachment {
                    target: resources.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.0),
                }),
            )
            .expect("old Scene draws relocated tile");
        let drawn = device.read_texture(target).expect("relocated Scene output");
        let center = 8 * drawn.bytes_per_row as usize + 8 * 4;
        let expected = if kind == AtlasTextureKind::Monochrome {
            [255; 4]
        } else {
            [20, 40, 80, 255]
        };
        assert_eq!(
            &drawn.bytes[center..center + 4],
            &expected,
            "{kind:?} old Scene after compact"
        );
        for (id, texture) in textures {
            destroy_gpu_atlas_texture(device, texture, "compact hardware gate", id);
        }
        println!("atlas compact {kind:?}: 2048 -> 512, padded pixels and immutable Scene verified");
    }
}

pub(super) fn verify_texture_copies<D: BackendResources + TextureTransferDevice>(device: &mut D) {
    for format in [Format::R8Unorm, Format::Bgra8Unorm] {
        let desc = TextureDescriptor {
            label: Some("GPU relocation fixture".into()),
            size: Extent2d::new(7, 5).expect("extent"),
            mip_level_count: 1,
            format,
            usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST | TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        };
        let source = device.create_texture(&desc).expect("source");
        let destination = device.create_texture(&desc).expect("destination");
        let bpp = format.bytes_per_pixel() as usize;
        let bytes: Vec<u8> = (0..7 * 5 * bpp).map(|index| (index * 7) as u8).collect();
        let write = TextureWriteDescriptor {
            texture: source,
            mip_level: 0,
            origin: Origin2d::ZERO,
            size: desc.size,
            layout: TextureDataLayout::new(0, (7 * bpp) as u32, 5).expect("pitch"),
        };
        device.write_texture(write, &bytes).expect("source upload");
        // No wait between upload and copies: exercise command ordering while the GPU may still
        // be using the source. Two overlapping destination copies also test WAW ordering.
        device
            .copy_texture_batch(&[
                gfx_core::TextureCopy {
                    source,
                    destination,
                    source_origin: Origin2d::ZERO,
                    destination_origin: Origin2d::ZERO,
                    size: desc.size,
                },
                gfx_core::TextureCopy {
                    source,
                    destination,
                    source_origin: Origin2d { x: 1, y: 1 },
                    destination_origin: Origin2d { x: 3, y: 2 },
                    size: Extent2d::new(2, 2).expect("extent"),
                },
            ])
            .expect("native GPU texture region copies");
        device
            .destroy_texture(source)
            .expect("source retirement while copies can be pending");
        let readback = device.read_texture(destination).expect("relocated pixels");
        for y in 0..5 {
            for x in 0..7 {
                let (sx, sy) = if (3..5).contains(&x) && (2..4).contains(&y) {
                    (x - 2, y - 1)
                } else {
                    (x, y)
                };
                let destination_offset = y * readback.bytes_per_row as usize + x * bpp;
                let source_offset = (sy * 7 + sx) * bpp;
                assert_eq!(
                    &readback.bytes[destination_offset..destination_offset + bpp],
                    &bytes[source_offset..source_offset + bpp],
                    "{format:?} copied pixel ({x},{y})"
                );
            }
        }
        device
            .destroy_texture(destination)
            .expect("destination cleanup");
    }
}

pub(super) fn fragment_heap<D: gfx_core::Device>(device: &mut D) -> Option<Vec<BufferId>> {
    if D::BACKEND_KIND != gfx_core::BackendKind::Vulkan {
        return None;
    }
    let mut buffers = Vec::new();
    for _ in 0..32 {
        buffers.push(
            device
                .create_buffer(&BufferDescriptor {
                    label: Some("heap fragmentation fixture".into()),
                    size: 1024 * 1024,
                    usage: BufferUsage::STORAGE | BufferUsage::COPY_SRC,
                    memory_location: MemoryLocation::GpuOnly,
                })
                .expect("fragment buffer"),
        );
    }
    Some(buffers)
}

pub(super) fn compact_heap<D: gfx_core::Device>(device: &mut D, buffers: Option<Vec<BufferId>>) {
    if let Some(mut buffers) = buffers {
        let survivor = buffers.pop().expect("survivor");
        device
            .write_buffer(survivor, 0, &[0x52; 256])
            .expect("survivor data");
        for buffer in buffers {
            device.destroy_buffer(buffer).expect("create holes");
        }
        let report = device.compact_memory().expect("live heap relocation");
        println!("heap relocation: {report:?}");
        assert!(
            report.moved_resources > 0,
            "fixture must actually move live resources"
        );
        assert!(
            report.reserved_after < report.reserved_before,
            "backing blocks must actually decrease"
        );
        assert!(
            report.reserved_peak <= report.reserved_before,
            "relocation must not allocate new backing blocks"
        );
        assert!(report.moved_bytes <= 8 * 1024 * 1024 && report.moved_resources <= 128);
        // Same public ID must still accept writes. The following production draws verify the
        // existing texture-view/resource-set IDs now refer to preserved replacement objects.
        device
            .write_buffer(survivor, 256, &[0x91; 256])
            .expect("stable buffer ID after relocation");
        device.destroy_buffer(survivor).expect("survivor cleanup");
    }
}
