//! Static page pixels must survive presentation-only popup reversal.
use super::*;
use std::sync::Arc;

mod lifecycle;
mod static_background;

const SIDE: usize = 64;

#[derive(Clone, Copy, Debug)]
enum PopupPlacement {
    Sibling,
    Nested,
    NestedTranslation,
    Closed,
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> crate::Bounds<crate::ScaledPixels> {
    crate::bounds(
        crate::point(crate::ScaledPixels(x), crate::ScaledPixels(y)),
        crate::size(crate::ScaledPixels(width), crate::ScaledPixels(height)),
    )
}

fn quad(bounds: crate::Bounds<crate::ScaledPixels>, color: crate::Rgba) -> crate::Quad {
    crate::Quad {
        bounds,
        content_mask: crate::ContentMask::new(bounds),
        background: color.into(),
        ..Default::default()
    }
}

fn sample(progress: f32) -> crate::SceneAnimationValue {
    crate::SceneAnimationValue {
        animation_id: crate::SceneAnimationId(1),
        property: crate::TransitionProperty::Opacity,
        progress,
        from: [0.; 4],
        to: [1., 0., 0., 0.],
    }
}

fn popup_sample(progress: f32, placement: PopupPlacement) -> crate::SceneAnimationValue {
    if matches!(placement, PopupPlacement::NestedTranslation) {
        crate::SceneAnimationValue {
            property: crate::TransitionProperty::Translation,
            to: [80., 0., 0., 0.],
            ..sample(progress)
        }
    } else {
        sample(progress)
    }
}

fn panel_sample(progress: f32) -> crate::SceneAnimationValue {
    let pose = [10. * (1. - progress), 0., 1., 1.];
    crate::SceneAnimationValue {
        animation_id: crate::SceneAnimationId(2),
        property: crate::TransitionProperty::VisualState,
        progress: 1.,
        from: pose,
        to: pose,
    }
}

fn popup(scene: &mut crate::Scene, viewport: crate::Bounds<crate::ScaledPixels>) {
    let popup_bounds = bounds(20., 20., 16., 16.);
    scene.begin_blur(crate::BlurCapture {
        animation_id: Some(crate::SceneAnimationId(1)),
        layer_id: Some(crate::GlobalElementId::from_path(&["popup".into()])),
        bounds: popup_bounds,
        content_mask: crate::ContentMask::new(viewport),
        radius: crate::ScaledPixels(0.),
        opacity: 1.,
    });
    scene.insert_primitive(quad(popup_bounds, crate::rgb(0xff0000)));
    scene.end_blur();
}

fn page(backdrop: bool, size: DrawableSize, placement: PopupPlacement) -> Arc<crate::Scene> {
    let viewport = bounds(0., 0., size.width as f32, size.height as f32);
    let mut scene = crate::Scene::default();
    // One background plus the settled panel is below two viewport areas,
    // matching the runtime page that the original admission cutoff rejected.
    scene.insert_primitive(quad(viewport, crate::rgb(0x204060)));
    if backdrop {
        scene.insert_primitive(crate::PaintBackdropBlur {
            order: 0,
            animation_id: None,
            bounds: viewport,
            content_mask: crate::ContentMask::new(viewport),
            corner_radii: Default::default(),
            radius: crate::ScaledPixels(2.),
            downsample: 2,
            levels: 1,
            recompute_overlap: false,
            saturation: 1.,
            opacity: 1.,
            tint: None,
        });
    }
    // Like the Settings panel, a large settled capture keeps its forwards
    // animation binding while the small popup changes independently.
    let panel_bounds = bounds(0., 0., size.width as f32 * 0.75, size.height as f32);
    scene.begin_blur(crate::BlurCapture {
        animation_id: Some(crate::SceneAnimationId(2)),
        layer_id: Some(crate::GlobalElementId::from_path(
            &["settings-panel".into()],
        )),
        bounds: panel_bounds,
        content_mask: crate::ContentMask::new(viewport),
        radius: crate::ScaledPixels(0.),
        opacity: 1.,
    });
    // An inactive retained child binding must not invalidate the parent source
    // while the unrelated popup animates. No sample is supplied for this id.
    scene.insert_animated_primitive(
        quad(panel_bounds, crate::rgb(0x6080a0)),
        crate::SceneAnimationId(3),
    );
    if matches!(placement, PopupPlacement::Nested) {
        popup(&mut scene, viewport);
    } else if matches!(placement, PopupPlacement::NestedTranslation) {
        let mut moving = quad(bounds(-20., 50., 16., 16.), crate::rgb(0xff0000));
        moving.content_mask = crate::ContentMask::new(bounds(40., 48., 32., 24.));
        moving.animation_id = Some(crate::SceneAnimationId(1));
        scene.insert_primitive(moving);
    }
    scene.end_blur();
    for index in 0..32 {
        scene.insert_primitive(quad(
            bounds((index % 8) as f32 * 8., (index / 8) as f32 * 8., 3., 3.),
            crate::rgb(0x80a040),
        ));
    }
    if matches!(placement, PopupPlacement::Sibling) {
        popup(&mut scene, viewport);
    }
    // Static foreground must remain above the independently animated popup.
    scene.insert_primitive(quad(bounds(22., 22., 4., 4.), crate::rgb(0x00ffff)));
    // The presentation packet owns this track's sample. A caller-sampled
    // scene value with the same ID would take precedence over the packet.
    scene.finish();
    scene.revision = 1;
    Arc::new(scene)
}

fn pixels<const N: usize>(
    renderer: &mut NovaRenderer,
    scene: &Arc<crate::Scene>,
    target: (TextureId, TextureViewId),
    previous: f32,
    values: [crate::SceneAnimationValue; N],
    full: bool,
) -> Vec<u8> {
    let previous_values: Vec<_> = values
        .iter()
        .map(|value| {
            if value.animation_id == crate::SceneAnimationId(1) {
                crate::SceneAnimationValue {
                    progress: previous,
                    ..*value
                }
            } else {
                *value
            }
        })
        .collect();
    let mut damage = crate::DirtyRegion::empty();
    if full {
        damage.mark_full(bounds(
            0.,
            0.,
            renderer.current_size.width as f32,
            renderer.current_size.height as f32,
        ));
    } else {
        for region in crate::paint::gpu_sample_damage(&scene, &previous_values, &values) {
            damage.push(region);
        }
        if !scene.animation_ids().contains(&crate::SceneAnimationId(1)) {
            // The discrete close commit removes this known fixture popup.
            damage.push(bounds(20., 20., 16., 16.));
        }
        assert!(!damage.is_full());
    }
    let blur_damage = crate::paint::backdrop_blur_animation_damage_plan(&scene, &previous_values, &values);
    let mut packet = crate::PresentationPacket::new(
        scene.clone(), std::sync::Arc::default(),
        values,
        [],
        Instant::now(),
        1.,
        damage,
        blur_damage,
        crate::PartialPresentMode::FullRedraw,
    );
    packet.force_full_backdrop_blur_refresh = full;
    assert!(
        renderer
            .present_framebuffer_only(packet)
            .expect("present popup sample")
    );
    device!(renderer, device, {
        device
            .render_step_list_to_texture_compat(
                target.1,
                renderer.render_pass,
                RenderStepList::from_render_steps(renderer.draw_step_scratch.steps()),
                LoadOp::Clear(clear_color()),
                Some(RenderPassDepthAttachment {
                    target: renderer.depth_texture_view,
                    depth_load_op: LoadOp::Clear(1.),
                }),
            )
            .expect("draw final composition");
        let readback = device.read_texture(target.0).expect("read popup pixels");
        readback
            .bytes
            .chunks_exact(readback.bytes_per_row as usize)
            .take(renderer.current_size.height as usize)
            .flat_map(|row| {
                row[..renderer.current_size.width as usize * 4]
                    .iter()
                    .copied()
            })
            .collect()
    })
}

fn verify(backend: RendererBackend) {
    for backdrop in [false, true] {
        for placement in [
            PopupPlacement::Sibling,
            PopupPlacement::Nested,
            PopupPlacement::NestedTranslation,
        ] {
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
            let target = super::target(&renderer);
            let scene = page(backdrop, renderer.current_size, placement);
            renderer.pack_scene(
                &scene, &crate::window::retained::RetainedPaint::default(),
                &[popup_sample(0., placement), panel_sample(0.5)],
                BackdropBlurQuality::default(),
            );
            assert!(renderer.frame_upload.retained_root_blur.is_none());
            renderer.pack_scene(
                &scene, &crate::window::retained::RetainedPaint::default(),
                &[popup_sample(0., placement), panel_sample(1.)],
                BackdropBlurQuality::default(),
            );
            assert!(
                renderer.frame_upload.retained_root_blur.is_some(),
                "same-revision panel completion must reconsider color retention"
            );
            renderer.retained_upload.invalidate_encode_key();
            while renderer.swapchain_warmup_frames > 0 {
                pixels(
                    &mut renderer,
                    &scene,
                    target,
                    0.,
                    [popup_sample(0., placement), panel_sample(0.5)],
                    true,
                );
            }
            let promoted = pixels(
                &mut renderer,
                &scene,
                target,
                0.,
                [popup_sample(0.1, placement), panel_sample(1.)],
                false,
            );
            assert!(renderer.frame_upload.retained_root_blur.is_some());
            let complete = pixels(
                &mut renderer,
                &scene,
                target,
                0.1,
                [popup_sample(0.1, placement), panel_sample(1.)],
                true,
            );
            assert!(
                promoted == complete,
                "new color target must initialize all pixels"
            );
            let baseline = pixels(
                &mut renderer,
                &scene,
                target,
                0.,
                [popup_sample(0., placement), panel_sample(1.)],
                true,
            );
            assert!(
                renderer.frame_upload.retained_root_blur.is_some(),
                "a small popup over a static page must retain page pixels"
            );
            let mut previous = 0.;
            let mut largest_source_scissor = 0u64;
            let mut largest_depth_clear = 0u64;
            let mut largest_total_source_pixels = 0usize;
            let samples = if matches!(placement, PopupPlacement::NestedTranslation) {
                [0.8, 0.2, 0.7, 1., 0.6, 0.]
            } else {
                [0.8, 0.2, 0.7, 1., 0.4, 0.]
            };
            for current in samples {
                let local = pixels(
                    &mut renderer,
                    &scene,
                    target,
                    previous,
                    [popup_sample(current, placement), panel_sample(1.)],
                    false,
                );
                let total_source_pixels =
                    crate::diagnostics::performance_metrics::performance_metrics_snapshot()
                        .backdrop_blur_source_pixels;
                largest_total_source_pixels = largest_total_source_pixels.max(total_source_pixels);
                assert!(
                    total_source_pixels
                        < renderer.current_size.width as usize
                            * renderer.current_size.height as usize
                            / 4,
                    "all submitted sources must remain local: backend={backend:?} backdrop={backdrop} placement={placement:?} total_source_pixels={total_source_pixels}"
                );
                assert_eq!(
                    renderer.draw_step_scratch.steps().len(),
                    1,
                    "final main pass must sample retained color instead of redrawing the page"
                );
                let root = renderer
                    .frame_upload
                    .retained_root_blur
                    .expect("retained root");
                let layers = renderer.prepare_color_layers(&[root]);
                assert!(
                    !layers.is_empty(),
                    "local change must update retained color"
                );
                let mut source_draws = 0;
                for layer in layers {
                    assert!(layer.preserve_retained_source);
                    let passes = crate::platform::nova::renderer::present::color_layer_passes(
                        renderer.render_pass,
                        renderer.pipelines.filter_render_pass,
                        RenderPassDepthAttachment {
                            target: renderer.depth_texture_view,
                            depth_load_op: LoadOp::Clear(1.),
                        },
                        std::slice::from_ref(&layer),
                    );
                    let first = passes
                        .iter()
                        .find(|pass| pass.depth_attachment.is_some())
                        .expect("source attachment pass");
                    assert_eq!(first.color_load_op, LoadOp::Load);
                    assert_eq!(
                        first
                            .depth_attachment
                            .expect("depth attachment")
                            .depth_load_op,
                        LoadOp::Clear(1.)
                    );
                    let initialized = first
                        .clear_region
                        .expect("source depth must clear only local damage");
                    let clear_pixels = u64::from(initialized.width) * u64::from(initialized.height);
                    largest_depth_clear = largest_depth_clear.max(clear_pixels);
                    assert!(
                        clear_pixels
                            < u64::from(renderer.current_size.width)
                                * u64::from(renderer.current_size.height)
                    );
                    for step in layer
                        .source_groups
                        .iter()
                        .flat_map(|group| &group.source_steps)
                    {
                        let scissor = match step {
                            RenderStepDescriptor::Draw(step) => step.scissor,
                            RenderStepDescriptor::DrawIndexed(step) => step.scissor,
                        }
                        .expect("local page capture needs a scissor");
                        source_draws += 1;
                        assert!(scissor.x >= initialized.x && scissor.y >= initialized.y);
                        assert!(scissor.x + scissor.width <= initialized.x + initialized.width);
                        assert!(scissor.y + scissor.height <= initialized.y + initialized.height);
                        largest_source_scissor = largest_source_scissor
                            .max(u64::from(scissor.width) * u64::from(scissor.height));
                        assert!(
                            scissor.width < renderer.current_size.width
                                && scissor.height < renderer.current_size.height
                        );
                    }
                }
                assert!(source_draws > 0, "local change must reach source drawing");
                let full = pixels(
                    &mut renderer,
                    &scene,
                    target,
                    current,
                    [popup_sample(current, placement), panel_sample(1.)],
                    true,
                );
                if local != full {
                    let changed: Vec<_> = local
                        .chunks_exact(4)
                        .zip(full.chunks_exact(4))
                        .enumerate()
                        .filter(|(_, (local, full))| local != full)
                        .collect();
                    let (index, (actual, expected)) = changed[0];
                    panic!(
                        "popup reversal mismatch backend={backend:?} backdrop={backdrop} placement={placement:?} previous={previous} current={current} pixels={} first=({}, {}) actual={actual:?} expected={expected:?}",
                        changed.len(),
                        index % renderer.current_size.width as usize,
                        index / renderer.current_size.width as usize,
                    );
                }
                assert_eq!(&baseline[..4], &local[..4], "static corner must survive");
                let foreground = (22 * renderer.current_size.width as usize + 22) * 4;
                assert_eq!(
                    &baseline[foreground..foreground + 4],
                    &local[foreground..foreground + 4],
                    "static foreground must occlude the moving popup"
                );
                if current > 0.
                    && (!matches!(placement, PopupPlacement::NestedTranslation) || current >= 0.6)
                {
                    assert!(baseline != local, "animation sample must reach pixels");
                }
                previous = current;
            }
            eprintln!(
                "popup color gate backend={backend:?} backdrop={backdrop} placement={placement:?} viewport={}x{} largest_source_scissor_pixels={largest_source_scissor} largest_depth_clear_pixels={largest_depth_clear} largest_total_source_pixels={largest_total_source_pixels} final_main_draws=1 samples=6",
                renderer.current_size.width, renderer.current_size.height,
            );
            if !backdrop && matches!(placement, PopupPlacement::Sibling) {
                lifecycle::verify(&mut renderer, &scene, target);
            }
            renderer
                .wait_for_pending_submissions()
                .expect("complete readback");
            device!(renderer, device, {
                device
                    .destroy_texture_view(target.1)
                    .expect("destroy readback view");
                device
                    .destroy_texture(target.0)
                    .expect("destroy readback texture");
            });
            renderer.destroy();
        }
    }
}

#[test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11"]
fn dx11_popup_reversal_preserves_static_color() {
    verify(RendererBackend::NovaDx11);
}

#[test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12"]
fn dx12_popup_reversal_preserves_static_color() {
    verify(RendererBackend::NovaDx12);
}

#[test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan"]
fn vulkan_popup_reversal_preserves_static_color() {
    verify(RendererBackend::NovaVulkan);
}

#[test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5"]
fn opengl_popup_reversal_preserves_static_color() {
    verify(RendererBackend::NovaOpenGl);
}
