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
    let coverage = crate::bounds(
        crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(-65536.)),
        crate::size(crate::ScaledPixels(65600.), crate::ScaledPixels(65600.)),
    );
    let mut capture = crate::scene::Painter::default();
    capture.begin_scroll_capture();
    capture.insert_primitive(Quad {
        bounds,
        content_mask: crate::ContentMask::new(coverage),
        corner_radii: crate::Corners::all(crate::ScaledPixels(6.)),
        background: crate::rgba(0x40e08080).into(),
        ..Default::default()
    });
    let distant = crate::Bounds::new(
        bounds.origin + crate::point(crate::ScaledPixels(65536.), crate::ScaledPixels(-65536.)),
        bounds.size,
    );
    capture.insert_primitive(Quad {
        bounds: distant,
        content_mask: crate::ContentMask::new(coverage),
        corner_radii: crate::Corners::all(crate::ScaledPixels(6.)),
        background: crate::rgba(0x40e08080).into(),
        ..Default::default()
    });
    let mut paint = crate::window::Paint::default();
    let viewport = crate::ContentMask::new(crate::bounds(
        crate::point(crate::px(0.), crate::px(0.)),
        crate::size(crate::px(64.), crate::px(64.)),
    ));
    paint.layers.raster_fixture(capture.end_scroll_capture(), coverage, viewport, [0, -128]);
    let layer = &paint.layers.frames[0];
    let (operations, origin) = layer.raster_source_for_tile([16, 152, 32, 168])
        .expect("published content-space tile source");
    assert_eq!(origin, [16, 24]);
    let (_, far_origin) = layer.raster_source_for_tile([65552, -65384, 65568, -65368])
        .expect("published far signed tile source");
    assert_eq!(far_origin, [65552, -65512]);
    assert!(layer.raster_source_for_tile([0, -65536, 16, -65520]).is_none());

    // Compile the immutable source through the ordinary native upload
    // pipeline. RasterTile supplies projection; no primitive is translated.
    let mut painter = crate::scene::Painter::default();
    for operation in operations.iter() {
        match operation {
            crate::scene::PaintOperation::Primitive(primitive) => painter.insert_primitive(primitive.clone()),
            crate::scene::PaintOperation::StartLayer(bounds) => painter.push_layer(*bounds),
            crate::scene::PaintOperation::EndLayer => painter.pop_layer(),
            _ => unreachable!("static raster source"),
        }
    }
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
    renderer.raster_tiles.begin_frame();
    let resident_key = RasterTileKey { layer: 1, x: 0, y: 0 };
    let resident_raster = RasterTile { extent: tile, origin: SignedOrigin2d { x: origin[0], y: origin[1] } };
    let cache_raster = RasterTile { extent: tile, origin: SignedOrigin2d { x: 16, y: 152 } };
    let (reference, resident) = device!(renderer, device, {
        let mut results = Vec::new();
        let mut resident = None;
        for raster in [
            RasterTile { extent: source, origin: SignedOrigin2d::ZERO },
            resident_raster,
            RasterTile { extent: tile, origin: SignedOrigin2d { x: far_origin[0], y: far_origin[1] } },
        ] {
            let extent = raster.extent;
            let write = if raster == resident_raster {
                Some(renderer.raster_tiles.prepare(device, resident_key, 1, cache_raster, Format::Bgra8Unorm)
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
            let submission = device.render_to_textures(&[RenderPassCommands {
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
            let is_resident = write.is_some();
            let mut write = write;
            if let Some(id) = submission {
                if let Some(reservation) = write.take() {
                    renderer.raster_tiles.finish_write(reservation, Some(id)).expect("track native writer");
                    assert!(renderer.raster_tiles.lookup(resident_key, 1, cache_raster, Format::Bgra8Unorm).is_none());
                }
                device.wait_submission(id).expect("native raster fence");
                renderer.raster_tiles.complete(id);
            }
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
            if is_resident {
                if let Some(write) = write {
                    renderer.raster_tiles.finish_write(write, None).expect("publish completed tile");
                }
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

    renderer.raster_tiles.begin_frame();
    renderer.raster_tiles.advance_layer(1, 2, |_| false);
    let cached = renderer.raster_tiles.lookup(resident_key, 2, cache_raster, Format::Bgra8Unorm)
        .expect("clean generation retains physical tile");
    assert_eq!((cached.texture, cached.view), resident);
    assert_eq!(renderer.raster_tiles.bytes(), 16 * 16 * 4);

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
            let submission = device.render_to_textures(&[RenderPassCommands {
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
            if let Some(id) = submission {
                renderer.raster_tiles.record_reader(resident_key, cached, id).expect("track native tile reader");
                assert!(renderer.raster_tiles.has_pending_users());
                device.wait_submission(id).expect("native composite fence");
                renderer.raster_tiles.complete(id);
            }
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
        renderer.raster_tiles.clear(device).expect("retire completed resident tile");
        assert_eq!(renderer.raster_tiles.bytes(), 0);
    });
    renderer.destroy();
}

#[test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12 and a native presentable window"]
fn dx12_scroll_raster_present_skips_scene_pack() {
    for native in [false, true] {
        verify_direct_present(RendererBackend::NovaDx12, false, native, false);
        verify_direct_present(RendererBackend::NovaDx12, true, native, false);
    }
    verify_direct_present(RendererBackend::NovaDx12, false, true, true);
}

#[test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan and a native presentable window"]
fn vulkan_scroll_raster_present_skips_scene_pack() {
    for native in [false, true] {
        verify_direct_present(RendererBackend::NovaVulkan, false, native, false);
        verify_direct_present(RendererBackend::NovaVulkan, true, native, false);
    }
    verify_direct_present(RendererBackend::NovaVulkan, false, true, true);
}

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11 and a native presentable window"]
fn dx11_scroll_raster_present_skips_scene_pack() {
    for native in [false, true] {
        verify_direct_present(RendererBackend::NovaDx11, false, native, false);
        verify_direct_present(RendererBackend::NovaDx11, true, native, false);
    }
    verify_direct_present(RendererBackend::NovaDx11, false, true, true);
}

#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows OpenGL 4.5 and a native presentable window"]
fn opengl_scroll_raster_present_skips_scene_pack() {
    for native in [false, true] {
        verify_direct_present(RendererBackend::NovaOpenGl, false, native, false);
        verify_direct_present(RendererBackend::NovaOpenGl, true, native, false);
    }
    verify_direct_present(RendererBackend::NovaOpenGl, false, true, true);
}

#[cfg(any(feature = "nova-gfx-dx11", feature = "nova-gfx-dx12", feature = "nova-gfx-vulkan", feature = "nova-gfx-opengl"))]
mod mixed_extension {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    #[derive(Default)]
    pub(super) struct Counts {
        pub(super) prepared: AtomicUsize,
        pub(super) accepted: AtomicUsize,
        pub(super) completed: AtomicUsize,
    }

    pub(super) struct Input(pub(super) Arc<Counts>);
    struct Renderer(Arc<Counts>);

    impl crate::RendererExtension for Input {
        fn is_time_dependent(&self) -> bool { true }
        fn create_renderer(&self, _: &mut dyn crate::ExtensionDevice, _: crate::RendererExtensionContext)
            -> crate::Result<Box<dyn crate::RendererExtensionRenderer>> {
            Ok(Box::new(Renderer(Arc::clone(&self.0))))
        }
    }

    impl crate::RendererExtensionRenderer for Renderer {
        fn render(&mut self, _: &dyn crate::RendererExtension, _: &mut dyn crate::ExtensionDevice,
            _: crate::RendererExtensionContext, _: &mut Vec<RenderCommand>) -> crate::Result<()> {
            self.0.prepared.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn gpu_submission_accepted(&mut self, _: SubmissionId) {
            self.0.accepted.fetch_add(1, Ordering::Relaxed);
        }
        fn gpu_submission_completed(&mut self, _: SubmissionId) {
            self.0.completed.fetch_add(1, Ordering::Relaxed);
        }
        fn destroy(&mut self, _: &mut dyn crate::ExtensionDevice) -> crate::Result<()> { Ok(()) }
    }
}

#[cfg(any(feature = "nova-gfx-dx11", feature = "nova-gfx-dx12", feature = "nova-gfx-vulkan", feature = "nova-gfx-opengl"))]
fn verify_direct_present(backend: RendererBackend, with_glyph: bool, native: bool, with_extension: bool) {
    let window = native_window::Window::renderer_fixture();
    window.resize(64, 64);
    let mut renderer = NovaRenderer::with_atlas(
        &window, backend, &RendererOptions::default(), GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(64), DevicePixels(64)), false, NovaRendererAtlas::new(),
    ).expect("direct scroll renderer");
    let viewport = crate::ContentMask::new(crate::bounds(
        crate::point(crate::px(0.), crate::px(0.)), crate::size(crate::px(64.), crate::px(64.)),
    ));
    let bounds = viewport.scale(1.).bounds;
    let coverage = crate::bounds(
        crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(-64.)),
        crate::size(crate::ScaledPixels(64.), crate::ScaledPixels(192.)),
    );
    // A translucent source detects missing Replace clears: a third-frame
    // repaint without a clear would accumulate alpha and green intensity.
    let quad = Quad { bounds, content_mask: crate::ContentMask::new(coverage),
        background: crate::rgba(0x00ff0080).into(), ..Default::default() };
    let glyph = if with_glyph {
        use crate::{AtlasKey, FontId, GlyphId, RenderGlyphParams};
        let key = AtlasKey::Glyph(RenderGlyphParams {
            font_id: FontId(1), glyph_id: GlyphId(2), font_size: crate::px(14.),
            subpixel_variant: crate::point(0, 0), scale_factor: 1.,
            grayscale_antialiasing: true, is_emoji: false, is_cjk: false,
        });
        let tile = renderer.atlas.ensure_tile_with(key, &mut || {
            Ok(Some((crate::size(DevicePixels(64), DevicePixels(64)),
                std::borrow::Cow::Owned(vec![255; 64 * 64]))))
        }).expect("glyph allocation").expect("glyph identity");
        renderer.sync_atlas_textures_for_current_backend().expect("atlas pages");
        let descriptor = renderer.atlas_resource_descriptor();
        let atlas = renderer.atlas.clone();
        device!(renderer, device, {
            upload_pending_atlas(frame_upload::queue::AtlasUpload::new(&atlas), device,
                &mut renderer.gpu_atlas_textures, &mut renderer.scroll_composite_slots,
                "scroll glyph fixture", &descriptor)
                .expect("glyph upload");
        });
        Some(crate::MonochromeSprite {
            order: 0, pad: 0, animation_id: None, bounds,
            content_mask: crate::ContentMask::new(coverage),
            color: crate::rgb(0xffffff), tile,
            transformation: crate::TransformationMatrix::unit(),
        })
    } else { None };
    let mut source = crate::scene::Operations::default();
    source.push(crate::scene::PaintOperation::Primitive(crate::scene::Primitive::Quad(quad)));
    if let Some(glyph) = glyph {
        source.push(crate::scene::PaintOperation::Primitive(crate::scene::Primitive::MonochromeSprite(glyph)));
    }
    // The second cell is asymmetric vertically: detect an incorrect GL
    // scissor flip or texture row convention, not just an incorrect color.
    let second_bounds = crate::bounds(crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(32.)),
        crate::size(crate::ScaledPixels(64.), crate::ScaledPixels(32.)));
    let second_quad = Quad { bounds: second_bounds, content_mask: crate::ContentMask::new(coverage),
        background: crate::rgb(0x0000ff).into(), ..Default::default() };
    let mut second_source = crate::scene::Operations::default();
    second_source.push(crate::scene::PaintOperation::Primitive(crate::scene::Primitive::Quad(second_quad)));
    let mut painter = crate::scene::Painter::default();
    // Background and foreground overlap both promoted fragments.
    painter.insert_primitive(Quad { bounds, content_mask: viewport.scale(1.),
        background: crate::rgb(0xff0000).into(), ..Default::default() });
    if native {
        assert!(painter.push_deferred_scroll(crate::window::scroll_paint::native_scroll_fixture(
            std::mem::take(&mut source), coverage, viewport)));
    } else {
        painter.insert_primitive(Quad { content_mask: viewport.scale(1.), ..quad });
        if let Some(glyph) = glyph {
            painter.insert_primitive(crate::MonochromeSprite { content_mask: viewport.scale(1.), ..glyph });
        }
    }
    // Keep an ordinary root interval between two independent promoted layers.
    // The optional extension owns the same logical operation position and
    // emits no draws: test registry preparation/consumer fences even when
    // expansion produces an empty command run.
    let extension_counts = std::sync::Arc::new(mixed_extension::Counts::default());
    if with_extension {
        assert!(native);
        painter.insert_primitive(crate::PaintRendererExtension::new(bounds, viewport.scale(1.),
            std::sync::Arc::new(mixed_extension::Input(extension_counts.clone()))));
    } else {
        painter.insert_primitive(Quad { bounds, content_mask: viewport.scale(1.),
            background: crate::rgb(0x800000).into(), ..Default::default() });
    }
    if native {
        assert!(painter.push_deferred_scroll(crate::window::scroll_paint::native_scroll_fixture(
            std::mem::take(&mut second_source), coverage, viewport)));
    } else {
        painter.insert_primitive(Quad { content_mask: viewport.scale(1.), ..second_quad });
    }
    painter.insert_primitive(Quad { bounds, content_mask: viewport.scale(1.),
        background: crate::rgb(0x808080).into(), ..Default::default() });
    // Keep the same glyph in the ordinary root as well as the raster
    // source: the root descriptor must now survive direct presentations.
    if let Some(glyph) = glyph {
        painter.insert_primitive(crate::MonochromeSprite { content_mask: viewport.scale(1.), ..glyph });
    }
    painter.finish();
    let mut paint = crate::window::Paint::new(painter.into_scene(), std::sync::Arc::default(),
        crate::window::PaintGeometry { layout_extent: Some((64, 64)), scale_factor: 1., ..Default::default() });
    let end = if with_glyph && !native { 3 } else { 2 };
    if native {
        assert_eq!(paint.scene.len(), 5 + usize::from(with_glyph));
        assert_eq!(paint.scene.quads.len(), if with_extension { 2 } else { 3 });
        assert_eq!(paint.scene.renderer_extensions.len(), usize::from(with_extension));
        assert_eq!(paint.scene.monochrome_sprites.len(), usize::from(with_glyph));
        paint.layers = crate::window::SceneLayers::capture_native(&paint.scene, None);
    } else {
        paint.layers.raster_fixture(source, coverage, viewport, [0, 0]);
        paint.layers.certify_fixture_root(&paint.scene, 1..end);
        paint.layers.raster_fixture(second_source, coverage, viewport, [0, 0]);
        paint.layers.certify_fixture_root(&paint.scene, end+1..end+2);
    }
    assert_eq!(paint.layers.frames[0].root_static_span(), Some(&(1..end)));
    assert_eq!(paint.layers.frames[1].root_static_span(), Some(&(end+1..end+2)));
    let mut protected = FxHashSet::default();
    paint.layers.collect_atlas_tile_ids_into(&mut protected);
    if let Some(glyph) = glyph {
        assert!(protected.contains(&(glyph.tile.texture_id, glyph.tile.tile_id.0)));
    }
    let paint = std::sync::Arc::new(paint);
    let packet = || crate::PresentationPacket::new(paint.clone(), [], [], Instant::now(),
        crate::DirtyRegion::full(bounds), crate::BackdropBlurDamagePlan::default(),
        crate::PartialPresentMode::FullRedraw);
    let encode_generation = renderer.frame_upload.encode_generation;
    let mut physical = None;
    let mut second_physical = None;
    let mut composite_buffers = None;
    let mut composite_commands: Option<std::sync::Arc<[RenderCommand]>> = None;
    let mut projection_buffer = None;
    let mut composite_binding = None;
    let mut outside_binding = None;
    let tile = RasterTile { origin: SignedOrigin2d::ZERO, extent: Extent2d::new(64, 64).unwrap() };
    let key = RasterTileKey { layer: paint.layers.frames[0].id, x: 0, y: 0 };
    let second_key = RasterTileKey { layer: paint.layers.frames[1].id, x: 0, y: 0 };
    for iteration in 0..3 {
        if iteration == 2 {
            // Repaint exactly one cell after both previous reader fences
            // complete. The adjacent page cell must keep its own pixels.
            renderer.raster_tiles.invalidate_layer(key.layer);
            if let Some(glyph) = glyph {
                device!(renderer, device, {
                    renderer.scroll_composite_slots[0]
                        .retire_atlas_bindings(device, glyph.tile.texture_id)
                        .expect("retire root atlas descriptor");
                });
                assert!(renderer.scroll_composite_slots[0].resident_command_list().is_none());
            }
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while renderer.presentation_readiness(None, Some((64, 64))).expect("scroll readiness") != PresentReadiness::Ready
            || !renderer.draw(packet()).expect("scroll direct present")
        {
            assert!(Instant::now() < deadline, "scroll root must present");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        renderer.wait_pending_submissions().expect("scroll native completion");
        assert_eq!(renderer.frame_upload.encode_generation, encode_generation, "must bypass Scene packing");
        assert!(renderer.scroll_raster_resources.is_empty(), "all transient bindings retired");
        let handles = renderer.scroll_composite_slots[0].handles();
        assert!(handles.iter().all(Option::is_some));
        if let Some(previous) = composite_buffers {
            assert_eq!(previous, handles, "free slot reuses physical composite buffers");
        }
        composite_buffers = Some(handles);
        let slot = &renderer.scroll_composite_slots[0];
        let commands = slot.resident_command_list();
        if with_extension {
            assert!(commands.is_none(), "extension commands must not persist across frames");
        } else {
            let commands = commands.expect("static root and resident tiles own a command list");
            if let Some(previous) = &composite_commands {
                assert_eq!(std::sync::Arc::ptr_eq(previous, &commands),
                    !(with_glyph && iteration == 2),
                    "reuse unchanged commands, but rebuild after atlas descriptor retirement");
            }
            composite_commands = Some(commands);
        }
        // The third frame repaints only one cell. It must retain the
        // existing projection allocation rather than creating a new one.
        let projection = slot.projection_buffer();
        if let Some(previous) = projection_buffer {
            assert_eq!(projection, Some(previous), "free slot retains projection storage");
        }
        projection_buffer = projection;
        let root_binding = renderer.scroll_composite_slots[0].outside_quad_binding().expect("resident outside quad binding");
        if let Some(previous) = outside_binding { assert_eq!(previous, root_binding); }
        outside_binding = Some(root_binding);
        let read = renderer.raster_tiles.lookup_ordered(key, 1, tile, Format::Bgra8Unorm, |_| false)
            .expect("completed tile sampling lease");
        let binding = device!(renderer, device, {
            renderer.raster_tiles.sampling_binding(device, read, 0, || {
                panic!("resident tile must already own the composite slot descriptor")
            }).expect("resident descriptor")
        });
        if let Some(previous) = composite_binding {
            assert_eq!(previous, binding, "resident hit reuses the native bind group");
        }
        composite_binding = Some(binding);
        let texture = read.texture();
        let second_read = renderer.raster_tiles.lookup_ordered(second_key, 1, tile, Format::Bgra8Unorm, |_| false)
            .expect("second native scroll texture");
        let second = second_read.texture();
        let paged = true; // All native test backends now use full-attachment page projection.
        if paged {
            assert_eq!(texture, second, "equal size classes share a physical page");
            assert_ne!(read.region(), second_read.region());
            let other_binding = device!(renderer, device, {
                renderer.raster_tiles.sampling_binding(device, second_read, 0, || panic!("shared page descriptor")).unwrap()
            });
            assert_eq!(binding, other_binding);
        } else {
            assert_ne!(texture, second);
        }
        if let Some(previous) = second_physical {
            assert_eq!(previous, second, "second layer must retain its physical texture");
        }
        second_physical = Some(second);
        let assert_cell = |lease: gfx_core::RasterTileRead, row: u32, expected: [u8; 4]| {
            let region = lease.region();
            device!(renderer, device, {
                let pixels = device.read_texture(lease.texture().texture).expect("physical page pixels");
                let offset = (region.y + row) as usize * pixels.bytes_per_row as usize
                    + (region.x + region.width / 2) as usize * 4;
                assert_eq!(&pixels.bytes[offset..offset + 4], &expected);
            });
        };
        assert_cell(second_read, 16, [0, 0, 0, 0]);
        assert_cell(second_read, 48, [255, 0, 0, 255]);
        assert_eq!(renderer.raster_tiles.bytes(), 2 * 64 * 64 * 4, "two misses allocate exactly two cells");
        assert_eq!(renderer.raster_tiles.logical_tiles(), 2);
        assert_eq!(renderer.raster_tiles.physical_textures(), if paged { 1 } else { 2 });
        if let Some(previous) = physical {
            assert_eq!(previous, texture, "resident hit must retain the physical texture");
        }
        physical = Some(texture);
        assert_cell(read, 16, if with_glyph { [255, 255, 255, 255] } else { [0, 128, 0, 128] });
        assert_cell(read, 48, if with_glyph { [255, 255, 255, 255] } else { [0, 128, 0, 128] });
    }
    if with_extension {
        use std::sync::atomic::Ordering;
        assert!(extension_counts.prepared.load(Ordering::Relaxed) >= 2);
        assert!(extension_counts.accepted.load(Ordering::Relaxed) >= 2);
        assert!(extension_counts.completed.load(Ordering::Relaxed) >= 2);
    }
    renderer.destroy();
}
