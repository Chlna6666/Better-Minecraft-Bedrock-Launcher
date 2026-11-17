//! Test a size-selected image fragment pipeline without changing batching or UVs.
use super::comparison::{self, Draw, native_binary};
use super::*;

pub(super) fn verify<D: gfx_core::Device + TextureTransferDevice>(
    device: &mut D,
    shaders: ShaderBinaries,
) {
    let config = SurfaceConfig::new(256, 256, Format::Bgra8Unorm).expect("image comparison extent");
    let reference = gfx_shader::WgslModule::parse(concat!(
        include_str!("../../shaders/core.wgsl"),
        include_str!("../../shaders/animation.wgsl"),
        include_str!("../../shaders/shape.wgsl"),
        include_str!("reference/poly_sprite.wgsl"),
    ))
    .expect("original image WGSL");
    let mut original = shaders.clone();
    original.poly_fragment = native_binary(
        reference
            .compile_backend(D::BACKEND_KIND, ShaderStage::Fragment, "fs_poly_sprite")
            .expect("original image fragment"),
        D::BACKEND_KIND,
    );
    // Fallback artifacts must stay identical, not merely pixel-equivalent.
    match (&original.poly_fragment.code, &shaders.poly_fragment.code) {
        (ShaderCode::DxBytecode(reference), ShaderCode::DxBytecodeStatic(actual)) => {
            assert!(
                reference.as_slice() == *actual,
                "fallback D3D bytecode changed"
            );
        }
        (reference, actual) => assert!(reference == actual, "fallback shader changed"),
    }
    println!("Image fallback shader artifact matches original exactly");
    let resources = [original, shaders].map(|shaders| {
        let core =
            create_renderer_core(device, config, "image comparison", shaders).expect("image core");
        let resources = create_renderer_resources(device, config, "image comparison", &core)
            .expect("image resources");
        upload_globals(device, resources.frame_resources[0].buffers, config.size);
        resources
    });
    let targets = [
        color_target(device, config.size),
        color_target(device, config.size),
    ];
    for kind in [AtlasTextureKind::Rgba, AtlasTextureKind::Bgra] {
        let atlases = [0, 1].map(|i| {
            let atlas = atlas_texture(device, &resources[i], kind);
            checkerboard(device, &atlas);
            atlas
        });
        let base_draws = [0, 1].map(|i| Draw {
            pipeline: resources[i].pipelines.alpha.poly_sprites,
            bind_group: atlases[i].poly_bind_groups[0],
            target: targets[i].1,
        });
        let mut cases = Vec::new();
        for edge in [32., 64., 127., 128., 129., 192., 240.] {
            for grayscale in [false, true] {
                cases.push((image(edge, grayscale), true));
            }
        }
        for width in [60., 140.] {
            for sampling in [0, 1] {
                let mut sprite = image(240., false);
                sprite.sampling = sampling;
                sprite.content_mask = crate::ContentMask::new(
                    bounds(point(px(20.25), px(20.75)), size(px(width), px(220.))).scale(1.),
                );
                sprite.content_mask.corner_radii = crate::Corners::all(crate::ScaledPixels(12.));
                cases.push((sprite, true));
            }
        }
        for (width, height) in [(64., 256.), (256., 64.), (32., 256.)] {
            let mut sprite = image(128., false);
            sprite.bounds = bounds(point(px(0.), px(0.)), size(px(width), px(height))).scale(1.);
            cases.push((sprite, true));
        }
        for (mut sprite, timed) in cases {
            sprite.tile.texture_id.kind = kind;
            let edge = sprite.bounds.size.width.0;
            let grayscale = sprite.grayscale;
            let height = sprite.bounds.size.height.0;
            let mask_width = sprite.content_mask.bounds.size.width.0;
            let sampling = sprite.sampling;
            let mut frame = FrameUpload::default();
            super::super::images::encode_images(&mut frame, &[sprite], 256);
            let color_only = match frame.batches.as_slice() {
                [UploadedBatch::PolySprites { color_only, .. }] => *color_only,
                _ => panic!("expected one image batch"),
            };
            for resource in &resources {
                device
                    .write_buffer(
                        resource.frame_resources[0].buffers.poly_sprite_buffer,
                        0,
                        frame.poly_sprites.as_slice(),
                    )
                    .expect("image frame upload");
            }
            let mut draws = base_draws;
            if color_only {
                draws[1].pipeline = resources[1].pipelines.alpha.poly_sprites_color;
            }
            let pixels = std::array::from_fn::<_, 2, _>(|i| {
                comparison::render(device, &resources[i], draws[i], None);
                device.read_texture(targets[i].0).expect("image readback")
            });
            for y in 0..256 {
                let rows = [0, 1].map(|i| {
                    let offset = y * pixels[i].bytes_per_row as usize;
                    &pixels[i].bytes[offset..offset + 1024]
                });
                assert_eq!(
                    rows[0], rows[1],
                    "adaptive image edge={edge} grayscale={grayscale} row={y}"
                );
            }
            if timed {
                comparison::profile(
                    device,
                    &resources,
                    targets,
                    draws,
                    &format!(
                        "AdaptiveImage/{kind:?}/width={edge}/height={height}/gray={grayscale}/mask={mask_width}/sampling={sampling}"
                    ),
                );
            }
        }
    }
    println!("Adaptive image full-frame comparison: 42 cases, maximum channel error=0/255");
}

fn image(edge: f32, grayscale: bool) -> PolychromeSprite {
    let area = bounds(point(px(8.25), px(12.25)), size(px(edge), px(edge))).scale(1.);
    PolychromeSprite {
        order: 0,
        animation_id: None,
        sampling: 0,
        grayscale,
        opacity: 0.7,
        bounds: area,
        content_mask: crate::ContentMask::new(
            bounds(point(px(0.), px(0.)), size(px(256.), px(256.))).scale(1.),
        ),
        corner_radii: crate::Corners::all(crate::ScaledPixels(4.)),
        tile: AtlasTile {
            texture_id: AtlasTextureId {
                index: 1,
                kind: AtlasTextureKind::Rgba,
            },
            bounds: bounds_device_pixels(),
            ..test_atlas_tile()
        },
    }
}

fn checkerboard<D: ResourceDevice>(device: &mut D, atlas: &NovaGpuAtlasTexture) {
    let mut pixels = Vec::new();
    for y in 0..16u8 {
        for x in 0..16u8 {
            pixels.extend_from_slice(&[
                x * 16,
                y * 16,
                (x ^ y) * 16,
                if (x + y) % 3 == 0 { 0 } else { 192 },
            ]);
        }
    }
    device
        .write_texture(
            TextureWriteDescriptor {
                texture: atlas.texture,
                mip_level: 0,
                origin: Origin2d::ZERO,
                size: Extent2d::new(16, 16).expect("checker extent"),
                layout: TextureDataLayout {
                    offset: 0,
                    bytes_per_row: std::num::NonZeroU32::new(64).expect("checker row"),
                    rows_per_image: std::num::NonZeroU32::new(16).expect("checker rows"),
                },
            },
            &pixels,
        )
        .expect("checker upload");
}
