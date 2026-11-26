//! A pixel-exact tile must rasterize the same content pixels without rebasing instances.
use super::*;

macro_rules! backend_test {
    ($name:ident, $feature:literal, $backend:ident) => {
        #[test]
        #[cfg(feature = $feature)]
        #[ignore = "requires Windows native GPU"]
        fn $name() {
            verify(RendererBackend::$backend);
        }
    };
}
backend_test!(dx11_tile_raster_pixels, "nova-gfx-dx11", NovaDx11);
backend_test!(dx12_tile_raster_pixels, "nova-gfx-dx12", NovaDx12);
backend_test!(vulkan_tile_raster_pixels, "nova-gfx-vulkan", NovaVulkan);
backend_test!(opengl_tile_raster_pixels, "nova-gfx-opengl", NovaOpenGl);

fn verify(backend: RendererBackend) {
    let window = native_window::Window::renderer_fixture();
    window.resize(64, 64);
    let mut renderer = NovaRenderer::with_atlas(
        &window,
        backend,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(64), DevicePixels(64)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native renderer");

    let bounds = crate::bounds(
        crate::point(crate::ScaledPixels(18.), crate::ScaledPixels(26.)),
        crate::size(crate::ScaledPixels(24.), crate::ScaledPixels(24.)),
    );
    let mut painter = crate::scene::Painter::default();
    painter.insert_primitive(Quad {
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        corner_radii: crate::Corners::all(crate::ScaledPixels(6.)),
        background: crate::rgba(0x40e08080).into(),
        ..Default::default()
    });
    let distant = crate::Bounds::new(
        bounds.origin + crate::point(crate::ScaledPixels(65536.), crate::ScaledPixels(-65536.)),
        bounds.size,
    );
    painter.insert_primitive(Quad {
        bounds: distant,
        content_mask: crate::ContentMask::new(distant),
        corner_radii: crate::Corners::all(crate::ScaledPixels(6.)),
        background: crate::rgba(0x40e08080).into(),
        ..Default::default()
    });
    painter.finish();
    upload(
        &mut renderer,
        0,
        &painter,
        &crate::window::retained::RetainedPaint::default(),
    );

    let source = Extent2d::new(renderer.current_size.width, renderer.current_size.height)
        .expect("source extent");
    assert!(source.width() >= 32 && source.height() >= 40);
    let tile = Extent2d::new(16, 16).expect("tile extent");
    let original_buffers = renderer.frame_resources[0].buffers;
    let mut tile_cache = RasterTileCache::new(64 * 1024 * 1024);
    tile_cache.begin_frame();
    let resident_key = RasterTileKey { layer: 1, x: 0, y: 0 };
    let resident_raster = RasterTile { extent: tile, origin: SignedOrigin2d { x: 16, y: 24 } };
    let (reference, resident) = device!(renderer, device, {
        let mut results = Vec::new();
        let mut resident = None;
        for raster in [
            RasterTile { extent: source, origin: SignedOrigin2d::ZERO },
            RasterTile { extent: tile, origin: SignedOrigin2d { x: 16, y: 24 } },
            RasterTile { extent: tile, origin: SignedOrigin2d { x: 65552, y: -65512 } },
        ] {
            let extent = raster.extent;
            let write = if raster == resident_raster {
                Some(tile_cache.prepare(device, resident_key, 1, raster, Format::Bgra8Unorm)
                    .expect("resident allocation").expect("cache capacity"))
            } else { None };
            let (color, color_view) = if let Some(write) = &write {
                let texture = write.texture();
                (texture.texture, texture.view)
            } else {
                let color = device.create_texture(&TextureDescriptor {
                    label: Some("pixel-exact raster color".into()),
                    size: extent,
                    mip_level_count: 1,
                    format: Format::Bgra8Unorm,
                    usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC | TextureUsage::SAMPLED,
                    memory_location: MemoryLocation::GpuOnly,
                    dimension: TextureDimension::D2,
                    sample_count: 1,
                }).expect("color");
                let color_view = device.create_texture_view(&TextureViewDescriptor {
                    label: None,
                    texture: color,
                    format: Format::Bgra8Unorm,
                    base_mip_level: 0,
                    mip_level_count: 1,
                }).expect("color view");
                (color, color_view)
            };
            let depth = create_depth_texture(device, "pixel-exact raster", extent)
                .expect("depth");
            let depth_view = device.create_texture_view(&TextureViewDescriptor {
                label: None,
                texture: depth,
                format: Format::Depth32Float,
                base_mip_level: 0,
                mip_level_count: 1,
            }).expect("depth view");
            let global_buffer = device.create_buffer(&BufferDescriptor {
                label: Some("pixel-exact raster globals".into()),
                size: GLOBAL_UPLOAD_BYTES as u64,
                usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
                memory_location: MemoryLocation::CpuToGpu,
            }).expect("tile globals");
            let globals = pack_tile_globals(raster, &renderer.frame_upload.globals);
            device.write_buffer(global_buffer, 0, &globals).expect("target globals");
            let buffers = FrameResourceBuffers { global_buffer, ..original_buffers };
            let bind_group = create_quad_bind_group(
                device, "pixel-exact raster", renderer.quad_bind_group_layout, &buffers,
            ).expect("target quad bind group");
            let mut steps = Vec::new();
            assert!(encode_tile_draw_commands(
                &renderer.frame_upload, &renderer.pipelines, raster,
                0..renderer.frame_upload.batches.len(),
                bind_group, bind_group, bind_group, |_| None, &mut steps,
            ), "static quads must be tile-rasterizable");
            assert!(!steps.is_empty());
            device.render_to_textures(&[RenderPassCommands {
                color_attachments: std::slice::from_ref(&color_view),
                render_pass: renderer.render_pass,
                commands: &steps,
                color_load_op: LoadOp::Clear(clear_color()),
                clear_region: None,
                depth_attachment: Some(DepthAttachment {
                    target: depth_view,
                    depth_load_op: LoadOp::Clear(1.),
                    stencil_load_op: None,
                }),
                viewport: Some(raster.viewport()),
                resolve_target: None,
            }]).expect("pixel-exact raster");
            let readback = device.read_texture(color).expect("native pixels");
            let pixels = readback.bytes.chunks_exact(readback.bytes_per_row as usize)
                .take(extent.height() as usize)
                .flat_map(|row| row[..extent.width() as usize * 4].iter().copied())
                .collect::<Vec<_>>();
            results.push(pixels);
            device.destroy_bind_group(bind_group).expect("retire binding");
            device.destroy_buffer(global_buffer).expect("retire globals");
            device.destroy_texture_view(depth_view).expect("retire depth view");
            device.destroy_texture(depth).expect("retire depth");
            if let Some(write) = write {
                tile_cache.finish_write(write, None).expect("publish completed tile");
                resident = Some((color, color_view));
            } else {
                device.destroy_texture_view(color_view).expect("retire color view");
                device.destroy_texture(color).expect("retire color");
            }
        }
        (results, resident.expect("resident tile"))
    });
    let row_bytes = source.width() as usize * 4;
    let cropped = reference[0].chunks_exact(row_bytes)
        .skip(24).take(16)
        .flat_map(|row| row[16 * 4..32 * 4].iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(reference[1].len(), 16 * 16 * 4);
    assert_eq!(reference[1], cropped, "tile must match full content raster");
    assert_eq!(reference[2], cropped, "far signed tile must match the same geometry");
    assert_ne!(&cropped[..4], &cropped[(8 * 16 + 8) * 4..][..4]);
    assert!(cropped.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));

    tile_cache.begin_frame();
    tile_cache.advance_layer(1, 2, |_| false);
    let cached = tile_cache.lookup(resident_key, 2, resident_raster, Format::Bgra8Unorm)
        .expect("clean generation retains physical tile");
    assert_eq!((cached.texture, cached.view), resident);
    assert_eq!(tile_cache.bytes(), 16 * 16 * 4);

    // Reuse the actual render-target texture twice, changing only the sprite
    // placement. No readback bytes are uploaded to the GPU.
    let mut composite = crate::scene::Painter::default();
    let atlas_tile = AtlasTile {
        texture_id: AtlasTextureId {
            index: u32::MAX - 1,
            kind: AtlasTextureKind::Bgra,
        },
        tile_id: crate::TileId(u32::MAX - 1),
        padding: 0,
        bounds: crate::bounds(crate::point(DevicePixels(0), DevicePixels(0)),
            crate::size(DevicePixels(16), DevicePixels(16))),
    };
    for x in [0., 24.] {
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(x), crate::ScaledPixels(0.)),
            crate::size(crate::ScaledPixels(16.), crate::ScaledPixels(16.)),
        );
        composite.insert_primitive(PolychromeSprite {
            order: 0,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            corner_radii: Default::default(),
            tile: atlas_tile,
            grayscale: false,
            opacity: 1.,
            sampling: 2,
            animation_id: None,
        });
    }
    composite.finish();
    upload(&mut renderer, 0, &composite, &crate::window::retained::RetainedPaint::default());
    let (target, target_view) = super::target(&renderer);
    assert!(source.width() >= 40);
    device!(renderer, device, {
        let global_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("resident tile composite globals".into()),
            size: GLOBAL_UPLOAD_BYTES as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        }).expect("composite globals");
        let raster = RasterTile { extent: source, origin: SignedOrigin2d::ZERO };
        device.write_buffer(global_buffer, 0, &pack_tile_globals(raster, &renderer.frame_upload.globals))
            .expect("composite globals");
        let buffers = FrameResourceBuffers { global_buffer, ..renderer.frame_resources[0].buffers };
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("resident tile composite".into()),
            layout: renderer.poly_sprite_bind_group_layout,
            entries: poly_atlas_resource_bindings(buffers, resident.1, renderer.atlas_sampler),
        }).expect("resident tile binding");
        for pipeline in [
            renderer.pipelines.premultiplied.poly_sprites,
            renderer.pipelines.premultiplied.poly_sprites_color,
        ] {
            let steps = [RenderCommand::Draw(DrawCommand {
                pipeline,
                bind_groups: bind_group_list([bind_group]),
                instance_count: 2,
                ..quad_step(&renderer)
            })];
            device.render_to_textures(&[RenderPassCommands {
                color_attachments: std::slice::from_ref(&target_view),
                render_pass: renderer.render_pass,
                commands: &steps,
                color_load_op: LoadOp::Clear(clear_color()),
                clear_region: None,
                depth_attachment: Some(DepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.),
                    stencil_load_op: None,
                }),
                viewport: Some(raster.viewport()),
                resolve_target: None,
            }]).expect("resident tile composite");
            let pixels = device.read_texture(target).expect("composited pixels");
            for x in [0usize, 24] {
                let actual = pixels.bytes.chunks_exact(pixels.bytes_per_row as usize)
                    .take(16).flat_map(|row| row[x * 4..(x + 16) * 4].iter().copied())
                    .collect::<Vec<_>>();
                assert_eq!(actual, cropped, "resident premultiplied tile at {x}");
            }
        }
        device.destroy_bind_group(bind_group).expect("retire composite binding");
        device.destroy_buffer(global_buffer).expect("retire composite globals");
        device.destroy_texture_view(target_view).expect("retire target view");
        device.destroy_texture(target).expect("retire target");
        tile_cache.clear(device).expect("retire completed resident tile");
        assert_eq!(tile_cache.bytes(), 0);
    });
    renderer.destroy();
}
