//! Replacing frame buffers must not invalidate independent resident Filter pixels.
use super::*;

macro_rules! backend_tests {
    ($static_name:ident, $growth_name:ident, $feature:literal, $backend:ident) => {
        #[test]
        #[cfg(feature = $feature)]
        #[ignore = "requires Windows native GPU"]
        fn $static_name() {
            verify(RendererBackend::$backend, false);
        }
        #[test]
        #[cfg(feature = $feature)]
        #[ignore = "requires Windows native GPU"]
        fn $growth_name() {
            verify(RendererBackend::$backend, true);
        }
    };
}
backend_tests!(
    dx11_static_rebind_keeps_filter_pixels,
    dx11_animation_growth_keeps_filter_pixels,
    "nova-gfx-dx11",
    NovaDx11
);
backend_tests!(
    dx12_static_rebind_keeps_filter_pixels,
    dx12_animation_growth_keeps_filter_pixels,
    "nova-gfx-dx12",
    NovaDx12
);
backend_tests!(
    vulkan_static_rebind_keeps_filter_pixels,
    vulkan_animation_growth_keeps_filter_pixels,
    "nova-gfx-vulkan",
    NovaVulkan
);
backend_tests!(
    opengl_static_rebind_keeps_filter_pixels,
    opengl_animation_growth_keeps_filter_pixels,
    "nova-gfx-opengl",
    NovaOpenGl
);

fn samples() -> [crate::SceneAnimationValue; 2] {
    [sample(0.8), panel_sample(1.)]
}

fn replace_static_buffer(renderer: &mut NovaRenderer) {
    let slot = renderer.current_frame_resource_index;
    // Populate the other slot with the same resident physical versions through
    // production adoption, so shrinking back exercises Adopt after Allocate.
    renderer
        .activate_frame_resources(1 - slot)
        .expect("other free slot");
    renderer
        .prepare_static_buffers()
        .expect("adopt resident streams");
    renderer
        .activate_frame_resources(slot)
        .expect("original free slot");
    let current = renderer.frame_resources[slot].buffers;
    let old_len = renderer.frame_upload.mono_sprites.len();
    // Unused packed tail applies capacity pressure without changing draw batches
    // or any captured source pixels. No GPU draw consumes this synthetic tail.
    renderer.frame_upload.mono_sprites.resize(
        (current.mono_sprite_capacity + 1) * PACKED_MONO_SPRITE_BYTES,
        0,
    );
    // The alternate slot must not satisfy adoption with its old smaller
    // version while the synthetic tail is present.
    renderer
        .retained_upload
        .invalidate_stream(1 - slot, BufferStream::MonoSprites);
    renderer
        .prepare_static_buffers()
        .expect("allocate static version");
    assert_ne!(renderer.mono_sprite_buffer, current.mono_sprite_buffer);
    renderer.frame_upload.mono_sprites.truncate(old_len);
    // Its untouched physical buffer still contains the original signature.
    renderer
        .retained_upload
        .inherit_stream(1 - slot, 1 - slot, BufferStream::MonoSprites);
}

fn grow_animation_buffer(renderer: &mut NovaRenderer) {
    let current = renderer.frame_resources[renderer.current_frame_resource_index].buffers;
    let old_len = renderer.frame_upload.gpu_indexed_animation_values.len();
    renderer.frame_upload.gpu_indexed_animation_values.resize(
        (current.animation_value_capacity + 1) * PACKED_ANIMATION_VALUE_BYTES,
        0,
    );
    renderer
        .ensure_stream_capacity()
        .expect("grow indexed animation table");
    assert_ne!(
        renderer.animation_value_buffer,
        current.animation_value_buffer
    );
    renderer
        .frame_upload
        .gpu_indexed_animation_values
        .truncate(old_len);
}

fn verify(backend: RendererBackend, animation_growth: bool) {
    let window = native_window::Window::renderer_fixture();
    let mut renderer = NovaRenderer::with_atlas(
        &window,
        backend,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(SIDE as i32), DevicePixels(SIDE as i32)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native renderer");
    let scene = page(true, renderer.current_size, PopupPlacement::Sibling);
    let target = super::super::target(&renderer);
    while renderer.swapchain_warmup_frames > 0 {
        pixels(&mut renderer, &scene, target, 0.8, samples(), true);
    }
    pixels(&mut renderer, &scene, target, 0.8, samples(), true);
    let full_filter_pixels = crate::performance_metrics_snapshot().backdrop_blur_target_pixels;
    verify_rebinding(&mut renderer, &scene, target, animation_growth);
    eprintln!(
        "buffer filter gate backend={backend:?} animation_growth={animation_growth} full_filter_pixels={full_filter_pixels} rebound_filter_pixels=0 rebound_source_pixels=0 pixels_equal=true"
    );
    renderer
        .wait_for_pending_submissions()
        .expect("retire readback");
    device!(renderer, device, {
        device.destroy_texture_view(target.1).expect("destroy view");
        device.destroy_texture(target.0).expect("destroy texture");
    });
    renderer.destroy();
}

fn verify_rebinding(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::window::Paint>,
    target: (TextureId, TextureViewId),
    animation_growth: bool,
) {
    let expected = pixels(renderer, scene, target, 0.8, samples(), false);
    renderer
        .wait_for_pending_submissions()
        .expect("fence-safe slot");
    assert!(renderer.filters.is_valid());
    let root_identity = renderer.root_color.identity();
    if animation_growth {
        grow_animation_buffer(renderer);
    } else {
        replace_static_buffer(renderer);
    }
    assert!(
        renderer.filters.is_valid(),
        "buffer rebinding must preserve cached Filter pixels"
    );
    renderer.retained_upload.invalidate_encode_key();
    let actual = pixels(renderer, scene, target, 0.8, samples(), false);
    let metrics = crate::performance_metrics_snapshot();
    assert_eq!(
        metrics.backdrop_blur_target_pixels, 0,
        "rebinding must not rerun Gaussian passes"
    );
    assert_eq!(
        metrics.backdrop_blur_source_pixels, 0,
        "unchanged Root/Element sources remain resident"
    );
    assert_eq!(renderer.root_color.identity(), root_identity);
    assert!(
        actual == expected,
        "rebound descriptors must sample unchanged native pixels"
    );
}
