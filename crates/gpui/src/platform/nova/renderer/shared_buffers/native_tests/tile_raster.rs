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
        background: crate::rgb(0x40e080).into(),
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
    let scissor = ScissorRect { x: 16, y: 24, width: 16, height: 16 };
    let reference = device!(renderer, device, {
        let mut results = Vec::new();
        for (extent, origin) in [
            (source, SignedOrigin2d::ZERO),
            (tile, SignedOrigin2d { x: -16, y: -24 }),
        ] {
            let color = device.create_texture(&TextureDescriptor {
                label: Some("pixel-exact raster color".into()),
                size: extent,
                mip_level_count: 1,
                format: Format::Bgra8Unorm,
                usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
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
            let mut globals = pack_global_params(
                renderer.current_size,
                renderer.surface_alpha.outputs_premultiplied_alpha(),
                [-(origin.x as f32), -(origin.y as f32)],
            );
            globals[12..24].copy_from_slice(&renderer.frame_upload.globals[12..24]);
            device.write_buffer(global_buffer, 0, &globals).expect("target globals");
            let buffers = FrameResourceBuffers { global_buffer, ..original_buffers };
            let bind_group = create_quad_bind_group(
                device, "pixel-exact raster", renderer.quad_bind_group_layout, &buffers,
            ).expect("target quad bind group");
            let step = RenderCommand::Draw(DrawCommand {
                bind_groups: bind_group_list([bind_group]),
                scissor: Some(scissor),
                ..quad_step(&renderer)
            });
            device.render_to_textures(&[RenderPassCommands {
                color_attachments: std::slice::from_ref(&color_view),
                render_pass: renderer.render_pass,
                commands: std::slice::from_ref(&step),
                color_load_op: LoadOp::Clear(clear_color()),
                clear_region: None,
                depth_attachment: Some(DepthAttachment {
                    target: depth_view,
                    depth_load_op: LoadOp::Clear(1.),
                    stencil_load_op: None,
                }),
                viewport: Some(Viewport {
                    origin,
                    extent: source,
                    reference_extent: source,
                    mapping: ViewportMapping::PixelExact,
                }),
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
            device.destroy_texture_view(color_view).expect("retire color view");
            device.destroy_texture(color).expect("retire color");
        }
        results
    });
    let row_bytes = source.width() as usize * 4;
    let cropped = reference[0].chunks_exact(row_bytes)
        .skip(24).take(16)
        .flat_map(|row| row[16 * 4..32 * 4].iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(reference[1].len(), 16 * 16 * 4);
    assert_eq!(reference[1], cropped, "tile must match full content raster");
    assert_ne!(&cropped[..4], &cropped[(8 * 16 + 8) * 4..][..4]);
    renderer.destroy();
}
