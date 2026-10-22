//! Final render command culling must retain every instance after sprite run merging.
use super::*;

pub(super) fn verify<D>(
    device: &mut D,
    resources: &RendererResources,
    atlas: &NovaGpuAtlasTexture,
    target: TextureId,
    view: TextureViewId,
) where
    D: ResourceDevice + PresentationDevice + TextureTransferDevice,
{
    let mut painter = crate::scene::Painter::default();
    for (x, sampling) in [(0., 0), (10., 1)] {
        let bounds = bounds(point(px(x), px(4.)), size(px(4.), px(8.))).scale(1.);
        painter.push_layer(bounds);
        painter.insert_primitive(MonochromeSprite {
            order: 0,
            pad: sampling,
            animation_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            color: crate::white().into(),
            tile: AtlasTile {
                texture_id: AtlasTextureId {
                    index: 1,
                    kind: AtlasTextureKind::Monochrome,
                },
                bounds: bounds_device_pixels(),
                ..test_atlas_tile()
            },
            transformation: Default::default(),
        });
        painter.pop_layer();
    }
    painter.finish();
    assert_eq!(painter.prepared_batches().len(), 2);
    let mut upload = FrameUpload::default();
    upload.encode(
        &mut RetainedChunkCache::default(),
        &painter,
        &crate::window::retained::RetainedPaint::default(),
        &[],
        DrawableSize {
            width: 16,
            height: 16,
        },
        &RenderingParameters::system(),
        false,
        BackdropBlurQuality::Full,
    );
    assert!(matches!(
        upload.batches.as_slice(),
        [UploadedBatch::MonoSprites { count: 2, .. }]
    ));
    let frame = resources.frame_resources[0];
    device
        .write_buffer(
            frame.buffers.mono_sprite_buffer,
            0,
            &upload.mono_sprites.to_vec(),
        )
        .expect("merged sprite upload");
    let mut steps = Vec::new();
    encode_clipped_draw_commands(
        &upload,
        &resources.pipelines,
        resources.pipelines.alpha,
        frame.bind_groups.quad_resource_set,
        frame.bind_groups.shadow_resource_set,
        frame.path_resource_set,
        |_| Some(atlas.mono_resource_sets[0]),
        frame.bind_groups.underline_resource_set,
        |_| None,
        DrawMode::Present,
        Some(ScissorRect {
            x: 10,
            y: 4,
            width: 4,
            height: 8,
        }),
        &mut steps,
    );
    assert_eq!(
        steps.len(),
        1,
        "right-hand source damage must retain the merged draw"
    );
    // Execute the generated descriptor, rather than constructing a replacement draw.
    device
        .render_to_textures(&[RenderPassCommands {
            texture_view: view,
            render_pass: resources.render_pass,
            commands: &steps,
            color_load_op: LoadOp::Clear(ClearColor {
                red: 0.,
                green: 0.,
                blue: 0.,
                alpha: 1.,
            }),
            clear_region: None,
            depth_attachment: Some(gfx_core::DepthAttachment {
                target: resources.depth_texture_view,
                depth_load_op: LoadOp::Clear(1.),
            }),
            viewport: None,
        }])
        .expect("clipped source render command");
    let drawn = device.read_texture(target).expect("merged source pixels");
    let offset = 8 * drawn.bytes_per_row as usize + 12 * 4;
    assert_eq!(&drawn.bytes[offset..offset + 4], &[255; 4]);
    println!("merged glyph source gate: prepared_runs=2 commands=1 right_pixels=white");
}
