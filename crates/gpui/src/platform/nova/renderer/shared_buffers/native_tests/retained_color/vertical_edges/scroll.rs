//! Real Window completion -> wheel input -> retained packet -> native present -> full-reference pixels.
use super::*;
use crate::{
    Animation, AnimationSpec, AppContext, Context, Entity, FillMode, InteractiveElement,
    IntoElement, ParentElement, Render, ScrollDelta, ScrollHandle, ScrollWheelEvent,
    StatefulInteractiveElement, Styled, TestAppContext, Window,
};
use std::{cell::Cell, rc::Rc, time::Duration};

struct Page {
    chrome: Entity<Chrome>,
    scroll: ScrollHandle,
    renders: Rc<Cell<usize>>,
}

struct Chrome {
    renders: Rc<Cell<usize>>,
}

impl Render for Chrome {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        crate::div()
            .relative()
            .size_full()
            .bg(crate::rgb(0x204060))
            .children((0..384).map(|index| {
                crate::div()
                    .absolute()
                    .left(crate::px((index % 32) as f32))
                    .top(crate::px((index / 32) as f32))
                    .size(crate::px(1.))
                    .bg(crate::rgb(0x204060))
            }))
    }
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        crate::div()
            .relative()
            .size_full()
            .child(
                self.chrome
                    .clone()
                    .cached(crate::StyleRefinement::default().size_full()),
            )
            .child(
                crate::div()
                    .id("scroll")
                    .absolute()
                    .left(crate::px(40.))
                    .top(crate::px(16.))
                    .w(crate::px(20.))
                    .h(crate::px(40.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        crate::div()
                            .w(crate::px(12.))
                            .h(crate::px(100.))
                            .flex_none()
                            .rounded(crate::px(6.))
                            .bg(crate::rgb(0x40e080))
                            .with_vertical_edges_animation(
                                "bar",
                                Animation::from_spec(
                                    AnimationSpec::new(Duration::from_millis(100))
                                        .fill_mode(FillMode::Both),
                                ),
                                [crate::px(98.), crate::px(0.)],
                                [crate::px(0.); 2],
                            )
                            .expect("leaf Quad"),
                    ),
            )
    }
}

macro_rules! backend_test {
    ($name:ident, $feature:literal, $backend:ident) => {
        #[gpui::test]
        #[cfg(feature = $feature)]
        #[ignore = "requires Windows native GPU"]
        fn $name(cx: &mut TestAppContext) {
            verify(cx, RendererBackend::$backend);
        }
    };
}
backend_test!(dx11_vertical_edges_first_scroll, "nova-gfx-dx11", NovaDx11);
backend_test!(dx12_vertical_edges_first_scroll, "nova-gfx-dx12", NovaDx12);
backend_test!(
    vulkan_vertical_edges_first_scroll,
    "nova-gfx-vulkan",
    NovaVulkan
);
backend_test!(
    opengl_vertical_edges_first_scroll,
    "nova-gfx-opengl",
    NovaOpenGl
);

fn read_pixels(renderer: &NovaRenderer, target: (TextureId, TextureViewId)) -> Vec<u8> {
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
            .expect("composition readback draw");
        let data = device.read_texture(target.0).expect("native pixels");
        data.bytes
            .chunks_exact(data.bytes_per_row as usize)
            .take(64)
            .flat_map(|row| row[..256].iter().copied())
            .collect()
    })
}

fn advance_animation(
    native: &mut NovaRenderer,
    packet: &mut crate::PresentationPacket,
    renders: &Cell<usize>,
) {
    let baseline = renders.get();
    let started = packet.frame_time;
    for (index, millis) in [70, 80, 90].into_iter().enumerate() {
        packet.sample_animations(started + Duration::from_millis(millis));
        assert!(
            native
                .present_framebuffer_only(packet.clone())
                .expect("retained animation sample")
        );
        native
            .wait_for_pending_submissions()
            .expect("sample retirement");
        packet.consume_submitted_damage();
        let metrics = crate::performance_metrics_snapshot();
        let writes = crate::platform::nova::renderer::buffer_upload::TEST_QUAD_UPLOAD_BYTES.get();
        if index > 0 {
            assert_eq!(
                writes, 0,
                "actual Window animation must not upload static Quads"
            );
            assert_eq!(metrics.encoded_scene_primitives, 0);
            assert!(
                metrics.backdrop_blur_source_pixels < 64 * 64,
                "retained root source must refresh only local animation damage"
            );
        }
        eprintln!(
            "vertical_edges window_animation backend={} millis={millis} quad_write_bytes={writes} encoded={} source_pixels={} native_damage={:?}",
            native.backend_info.label(),
            metrics.encoded_scene_primitives,
            metrics.backdrop_blur_source_pixels,
            native.draw_step_scratch.submitted_present_damage
        );
    }
    assert_eq!(
        renders.get(),
        baseline,
        "presentation samples must not run View::render"
    );
}

fn verify(cx: &mut TestAppContext, backend: RendererBackend) {
    let scroll = ScrollHandle::new();
    let renders = Rc::new(Cell::new(0));
    let chrome_renders = Rc::new(Cell::new(0));
    let chrome = cx.new(|_| Chrome {
        renders: chrome_renders.clone(),
    });
    let (_, mut visual) = cx.add_window_view(|window, _| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .set_scene_animation_owner(true);
        window
            .platform_window
            .as_test()
            .unwrap()
            .capture_submitted_packets();
        Page {
            chrome,
            scroll: scroll.clone(),
            renders: renders.clone(),
        }
    });
    visual.simulate_scale_factor_change(1.);
    visual.simulate_resize(crate::size(crate::px(64.), crate::px(64.)));
    let mut packet = visual.update(|window, cx| {
        window.draw(cx).clear();
        assert!(window.test_present().is_accepted());
        window
            .platform_window
            .as_test()
            .unwrap()
            .take_submitted_packet()
            .unwrap()
    });
    let native_window = native_window::Window::renderer_fixture();
    let reference_window = native_window::Window::renderer_fixture();
    let mut native = renderer(&native_window, backend);
    let mut reference = renderer(&reference_window, backend);
    let target = super::super::super::target(&native);
    let reference_target = super::super::super::target(&reference);
    advance_animation(&mut native, &mut packet, &renders);
    let completions = packet.sample_animations(packet.frame_time + Duration::from_secs(1));
    assert_eq!(completions.len(), 1);
    let static_bounds: Vec<_> = packet
        .paint
        .quads
        .iter()
        .filter(|quad| quad.background.solid == crate::rgb(0x204060))
        .map(|quad| quad.bounds)
        .collect();
    assert!(
        native
            .present_framebuffer_only(packet)
            .expect("terminal native presentation")
    );
    let before = read_pixels(&native, target);
    let settled = visual.update(|window, cx| {
        for completion in completions {
            window.presentation_animation_completed(completion);
        }
        window.draw(cx).clear();
        assert!(window.test_present().is_accepted());
        window
            .platform_window
            .as_test()
            .unwrap()
            .take_submitted_packet()
            .unwrap()
    });
    assert!(settled.presentation_animation_timelines.is_empty());
    native
        .present_framebuffer_only(settled)
        .expect("completion cleanup presentation");
    let baseline_renders = renders.get();
    let chrome_baseline = chrome_renders.get();
    visual.simulate_event(ScrollWheelEvent {
        position: crate::point(crate::px(50.), crate::px(30.)),
        delta: ScrollDelta::Pixels(crate::point(crate::px(0.), crate::px(-9.5))),
        ..Default::default()
    });
    assert!(
        scroll.offset().y < crate::px(0.),
        "real wheel event must scroll"
    );
    // TestPlatform records vsync requests but deliberately does not synthesize native callbacks.
    // Deliver exactly the frame requested by the wheel handler, as the platform owner would.
    let platform = visual.update(|window, _| window.platform_window.as_test().unwrap().clone());
    let requested = platform
        .last_requested_frame()
        .expect("wheel must request a frame");
    assert!(requested.needs_ui_commit());
    platform.simulate_request_frame(requested);
    visual.run_until_parked();
    let packet = visual.update(|window, _| {
        window
            .platform_window
            .as_test()
            .unwrap()
            .take_submitted_packet()
            .expect("wheel frame must submit a real packet")
    });
    let bar = packet
        .paint
        .quads
        .iter()
        .find(|quad| quad.background.solid == crate::rgb(0x40e080))
        .expect("one bar");
    assert_eq!(bar.bounds.top().0, 16. + scroll.offset().y.0);
    assert_eq!(bar.bounds.size.height.0, 100.);
    assert_eq!(bar.content_mask.bounds.top().0, 16.);
    assert_eq!(bar.content_mask.bounds.bottom().0, 56.);
    // Replay may rebind frame-local DrawOrder and copy a primitive. Its contract is the actual
    // replay path and skipped static Render, not equality of bytes or CPU allocation addresses.
    assert!(
        packet.paint.replayed_primitives >= 384,
        "cached chrome must use retained replay"
    );
    assert_eq!(
        chrome_renders.get(),
        chrome_baseline,
        "static sibling must not render on scroll"
    );
    assert_eq!(
        static_bounds,
        packet
            .paint
            .quads
            .iter()
            .filter(|quad| quad.background.solid == crate::rgb(0x204060))
            .map(|quad| quad.bounds)
            .collect::<Vec<_>>(),
        "static geometry survives replay"
    );
    assert!(
        !packet.dirty_region.is_full(),
        "first scroll must retain local damage"
    );
    assert_eq!(
        packet.partial_present_mode,
        crate::PartialPresentMode::Partial
    );
    let mut full_damage = crate::DirtyRegion::empty();
    full_damage.mark_full(bounds(0., 0., 64., 64.));
    let full = crate::PresentationPacket::new(
        packet.paint.clone(),
        packet.presentation_animation_values.iter().copied(),
        [],
        Instant::now(),
        full_damage,
        Default::default(),
        crate::PartialPresentMode::FullRedraw,
    );
    assert!(
        native
            .present_framebuffer_only(packet)
            .expect("first-scroll native present")
    );
    let metrics = crate::performance_metrics_snapshot();
    let actual = read_pixels(&native, target);
    assert!(
        reference
            .present_framebuffer_only(full)
            .expect("full reference")
    );
    assert_eq!(
        actual,
        read_pixels(&reference, reference_target),
        "retained first-scroll pixels"
    );
    assert_ne!(actual, before, "scroll must change rounded top pixels");
    eprintln!(
        "vertical_edges first_scroll backend={backend:?} root_renders={} native_damage={:?} quad_upload_bytes={} dirty_area={} source_pixels={}",
        renders.get() - baseline_renders,
        native.draw_step_scratch.submitted_present_damage,
        metrics.quad_upload_bytes,
        metrics.dirty_rect_area,
        metrics.backdrop_blur_source_pixels
    );
    release_target(&mut native, target);
    release_target(&mut reference, reference_target);
    native.destroy();
    reference.destroy();
    visual.update(|window, _| window.remove_window());
}
