//! Compare a real ordinary-view splice with a complete rebuild of the same view state.
use super::*;
use crate::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

struct Leaf {
    blue: bool,
}

impl Render for Leaf {
    fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .absolute()
            .left(crate::px(8.))
            .top(crate::px(8.))
            .size(crate::px(16.))
            .bg(crate::rgb(if self.blue { 0x0000ff } else { 0xff0000 }))
    }
}

struct Root {
    leaf: Entity<Leaf>,
    renders: Rc<Cell<usize>>,
    backdrop: bool,
}

impl Render for Root {
    fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let blur = || {
            crate::div()
                .absolute()
                .left(crate::px(6.))
                .top(crate::px(6.))
                .size(crate::px(20.))
                .background_blur(crate::px(2.))
        };
        let mut root = crate::div().relative().size_full().bg(crate::rgb(0x00ff00));
        if self.backdrop {
            root = root.child(blur()).child(self.leaf.clone()).child(blur());
        } else {
            // Cross the production retained-root threshold with actual static siblings.
            for index in 0..384 {
                root = root.child(
                    crate::div()
                        .absolute()
                        .left(crate::px((index % 32) as f32))
                        .top(crate::px((index / 32) as f32))
                        .size(crate::px(1.))
                        .bg(crate::rgb(0x00ff00)),
                );
            }
            root = root.child(self.leaf.clone());
        }
        root.child(
            crate::div()
                .absolute()
                .size(crate::px(4.))
                .bg(crate::rgb(0xffff00)),
        )
    }
}

#[gpui::test]
#[cfg(feature = "nova-gfx-dx11")]
#[ignore = "requires Windows hardware D3D11"]
fn dx11_plain_leaf_splice_matches_full_rebuild(cx: &mut TestAppContext) {
    verify(cx, RendererBackend::NovaDx11);
}
#[gpui::test]
#[cfg(feature = "nova-gfx-dx12")]
#[ignore = "requires Windows hardware D3D12"]
fn dx12_plain_leaf_splice_matches_full_rebuild(cx: &mut TestAppContext) {
    verify(cx, RendererBackend::NovaDx12);
}
#[gpui::test]
#[cfg(feature = "nova-gfx-vulkan")]
#[ignore = "requires Windows hardware Vulkan"]
fn vulkan_plain_leaf_splice_matches_full_rebuild(cx: &mut TestAppContext) {
    verify(cx, RendererBackend::NovaVulkan);
}
#[gpui::test]
#[cfg(feature = "nova-gfx-opengl")]
#[ignore = "requires Windows hardware OpenGL 4.5"]
fn opengl_plain_leaf_splice_matches_full_rebuild(cx: &mut TestAppContext) {
    verify(cx, RendererBackend::NovaOpenGl);
}

fn pixels(
    renderer: &mut NovaRenderer,
    scene: Arc<crate::Scene>,
    target: (TextureId, TextureViewId),
    local_damage: Option<crate::Bounds<crate::ScaledPixels>>,
    blur_damage: crate::BackdropBlurDamagePlan,
) -> Vec<u8> {
    let mut damage = crate::DirtyRegion::empty();
    if let Some(bounds) = local_damage {
        damage.push(bounds);
    } else {
        damage.mark_full(crate::bounds(
            crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
            crate::size(crate::ScaledPixels(32.), crate::ScaledPixels(32.)),
        ));
    }
    let mut packet = crate::PresentationPacket::new(
        scene, std::sync::Arc::default(),
        [],
        [],
        std::time::Instant::now(),
        1.,
        damage,
        blur_damage,
        crate::PartialPresentMode::FullRedraw,
    );
    packet.force_full_backdrop_blur_refresh = local_damage.is_none();
    assert!(
        renderer
            .present_framebuffer_only(packet)
            .expect("present real view scene")
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
            .expect("draw real view scene");
        let readback = device.read_texture(target.0).expect("read real view scene");
        readback
            .bytes
            .chunks_exact(readback.bytes_per_row as usize)
            .take(32)
            .flat_map(|row| row[..32 * 4].iter().copied())
            .collect()
    })
}

fn verify(cx: &mut TestAppContext, backend: RendererBackend) {
    verify_case(cx, backend, true);
    verify_case(cx, backend, false);
}

fn verify_case(cx: &mut TestAppContext, backend: RendererBackend, backdrop: bool) {
    let renders = Rc::new(Cell::new(0));
    let leaf = cx.new(|_| Leaf { blue: false });
    let (_, visual) = cx.add_window_view(|_, _| Root {
        leaf: leaf.clone(),
        renders: renders.clone(),
        backdrop,
    });
    visual.simulate_scale_factor_change(1.0);
    visual.simulate_resize(crate::size(crate::px(32.), crate::px(32.)));
    let before = visual.update(|window, cx| {
        window.draw(cx).clear();
        window.rendered_frame.scene.snapshot()
    });
    let baseline = renders.get();
    leaf.update(visual, |leaf, cx| {
        leaf.blue = true;
        cx.notify();
    });
    let after = visual.update(|window, cx| {
        window.draw(cx).clear();
        window.rendered_frame.scene.snapshot()
    });
    assert_eq!(
        renders.get(),
        baseline,
        "ordinary leaf update must skip root Render"
    );
    let reference = visual.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
        window.rendered_frame.scene.snapshot()
    });
    assert_ne!(before.revision, after.revision);
    let leaf_bounds = after
        .quads
        .iter()
        .find(|quad| quad.background.solid == crate::rgb(0x0000ff))
        .expect("changed leaf quad")
        .bounds;
    let blur_damage = crate::paint::backdrop_blur_damage_plan(&after, &before, &[], &[]);
    if backdrop {
        assert!(blur_damage.refresh_required());
    }
    let window = native_window::Window::renderer_fixture();
    let mut renderer = NovaRenderer::with_atlas(
        &window,
        backend,
        &RendererOptions::default(),
        GpuSubmissionMode::Deferred,
        crate::size(DevicePixels(32), DevicePixels(32)),
        false,
        NovaRendererAtlas::new(),
    )
    .expect("native renderer");
    let target = super::target(&renderer);
    // Let rotating frame slots and the initial interactive-quality window settle.
    while renderer.swapchain_warmup_frames > 0 {
        pixels(
            &mut renderer,
            before.clone(),
            target,
            None,
            Default::default(),
        );
    }
    // The first steady-quality frame may replace filter targets.
    pixels(
        &mut renderer,
        before.clone(),
        target,
        None,
        Default::default(),
    );
    let before_pixels = pixels(&mut renderer, before, target, None, Default::default());
    let after_pixels = pixels(&mut renderer, after, target, Some(leaf_bounds), blur_damage);
    if !backdrop {
        let root = renderer
            .frame_upload
            .retained_root_blur
            .expect("retained root");
        let layers = renderer.prepare_color_layers(&[root]);
        assert!(!layers.is_empty(), "local root source refresh must execute");
        for layer in layers {
            assert!(layer.preserve_retained_source);
            for step in layer
                .source_groups
                .iter()
                .flat_map(|group| &group.source_steps)
            {
                let scissor = match step {
                    RenderStepDescriptor::Draw(step) => step.scissor,
                    RenderStepDescriptor::DrawIndexed(step) => step.scissor,
                }
                .expect("retained root source must be clipped");
                assert!(scissor.width < 32 && scissor.height < 32);
            }
        }
    }
    let reference_pixels = pixels(&mut renderer, reference, target, None, Default::default());
    assert_ne!(
        before_pixels, after_pixels,
        "changed leaf must reach the GPU"
    );
    assert_eq!(
        after_pixels, reference_pixels,
        "splice must preserve blur barriers and retained static pixels"
    );
    assert_eq!(&before_pixels[..4], &after_pixels[..4]);
    renderer
        .wait_for_pending_submissions()
        .expect("complete presentation");
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
