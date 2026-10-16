//! Final DrawStep culling must retain every instance after sprite run merging.
use super::*;

pub(super) fn verify<D>(
    device: &mut D,
    resources: &RendererResources,
    atlas: &NovaGpuAtlasTexture,
    target: TextureId,
    view: TextureViewId,
) where
    D: BackendResources + BackendPresentationCompat + TextureTransferDevice,
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
        &mut RetainedQuadChunkCache::default(),
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
        .write_buffer(frame.buffers.mono_sprite_buffer, 0, &upload.mono_sprites)
        .expect("merged sprite upload");
    let mut steps = Vec::new();
    draw_steps_for_upload_into_clipped(
        &upload,
        &resources.pipelines,
        resources.pipelines.alpha,
        frame.resource_sets.quad_resource_set,
        frame.resource_sets.shadow_resource_set,
        frame.path_resource_set,
        |_| Some(atlas.mono_resource_sets[0]),
        frame.resource_sets.underline_resource_set,
        |_| None,
        DrawStepMode::Present,
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
        .render_step_list_to_texture_compat(
            view,
            resources.render_pass,
            gfx_core::RenderStepList::Render(&steps),
            LoadOp::Clear(ClearColor {
                red: 0.,
                green: 0.,
                blue: 0.,
                alpha: 1.,
            }),
            Some(gfx_core::RenderPassDepthAttachment {
                target: resources.depth_texture_view,
                depth_load_op: LoadOp::Clear(1.),
            }),
        )
        .expect("clipped source DrawStep");
    let drawn = device.read_texture(target).expect("merged source pixels");
    let offset = 8 * drawn.bytes_per_row as usize + 12 * 4;
    assert_eq!(&drawn.bytes[offset..offset + 4], &[255; 4]);
    println!("merged glyph source gate: prepared_runs=2 draw_steps=1 right_pixels=white");
}
