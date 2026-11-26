//! Native window materials and application-content blur gallery.
//!
//! Run with `--backend=nova-dx12` or `--backend=nova-vulkan` to select Nova.
//! Radii are logical pixels. Window materials depend on native compositor support;
//! background blur samples earlier drawing, while filter blur includes children.

use gpui::{
    App, Application, Bounds, BoxShadow, Context, Div, RendererBackend, RendererOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, div, point, prelude::*, px, rgb, rgba,
    size,
};

struct BlurGallery;

const RADII: [f32; 4] = [0.0, 2.0, 6.0, 12.0];

fn stripes() -> Div {
    div()
        .absolute()
        .size_full()
        .flex()
        .children((0..16).map(|index| {
            div()
                .h_full()
                .flex_1()
                .bg(rgb([0x263c70, 0x94518d, 0x32a39c, 0xd5a66b][index % 4]))
        }))
}

fn glass(radius: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(rgba(0xffffff60))
        .bg(rgba(0x10203060))
        .background_blur(px(radius))
}

fn sample_content() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child("GPUI / 中文 / 🌈")
        .child(div().flex().gap_2().children(
            [0xff7979, 0xffcf67, 0x6ee7c5].map(|color| div().size_8().rounded_lg().bg(rgb(color))),
        ))
}

fn radius_row(background: bool) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .gap_4()
        .children(RADII.map(|radius| {
            div()
                .relative()
                .flex_1()
                .flex_shrink_0()
                .h(px(148.0))
                .rounded_lg()
                .overflow_hidden()
                .child(stripes())
                .child(
                    div()
                        .relative()
                        .p_3()
                        .child(format!("σ = {radius} px"))
                        .child(if background {
                            glass(radius).child(sample_content())
                        } else {
                            div().p_4().filter_blur(px(radius)).child(sample_content())
                        }),
                )
        }))
}

fn nested_effects() -> Div {
    div()
        .relative()
        .h(px(190.0))
        .flex_shrink_0()
        .rounded_xl()
        .overflow_hidden()
        .child(stripes())
        .child(
            div().relative().p_4().child(
                glass(8.0).child("Outer background blur · 圆角裁剪").child(
                    div()
                        .flex()
                        .gap_4()
                        .child(glass(16.0).flex_1().child("Nested glass · 清晰文字"))
                        .child(
                            div()
                                .flex_1()
                                .filter_blur(px(2.0))
                                .child(glass(4.0).child(sample_content())),
                        ),
                ),
            ),
        )
}

fn shadow_row() -> Div {
    div().flex().gap_4().p_4().children(RADII.map(|radius| {
        div()
            .flex_1()
            .p_4()
            .rounded_lg()
            .bg(rgb(0x35475e))
            .shadow(vec![BoxShadow {
                color: rgba(0x79cfff99).into(),
                offset: point(px(0.0), px(4.0)),
                blur_radius: px(radius),
                spread_radius: px(1.0),
            }])
            .child(format!("Shadow · {radius} px"))
    }))
}

fn materials() -> Div {
    use WindowBackgroundAppearance::{Blurred, Mica, MicaAlt, Opaque, Transparent};
    div().flex().gap_2().children(
        [Opaque, Transparent, Blurred, Mica, MicaAlt]
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                div()
                    .id(("material", index))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgba(0x30465dcc))
                    .cursor_pointer()
                    .on_click(move |_, window, _cx| {
                        window.set_background_appearance(mode);
                        window.refresh();
                    })
                    .child(format!("{mode:?}"))
            }),
    )
}

impl Render for BlurGallery {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("blur-gallery")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .text_color(rgb(0xffffff))
            .bg(rgba(0x101b2b88))
            .child(div().text_2xl().child("GPUI blur gallery · 模糊效果"))
            .child(materials())
            .child(format!(
                "Requested: {:?}  /  Accepted: {:?}  /  Capabilities: {:?}",
                window.background_appearance(),
                window.effective_background_appearance(),
                window.background_capabilities(),
            ))
            .child("Window material is visible through this translucent page; support is platform-dependent.")
            .child("Background blur — earlier content blurs; foreground text stays sharp")
            .child(radius_row(true))
            .child("Filter blur — text, emoji and shapes blur together")
            .child(radius_row(false))
            .child("Nested effects — two glass layers, filter blur and rounded clipping")
            .child(nested_effects())
            .child("Shadow blur — dedicated BoxShadow path")
            .child(shadow_row())
    }
}

fn main() -> anyhow::Result<()> {
    let mut renderer = RendererOptions::default();
    for argument in std::env::args().skip(1) {
        renderer.backend = match argument.as_str() {
            "--backend=nova-dx12" => RendererBackend::NovaDx12,
            "--backend=nova-vulkan" => RendererBackend::NovaVulkan,
            _ => anyhow::bail!("Unknown argument: {argument}"),
        };
    }
    Application::with_renderer_options(renderer).run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1080.0), px(900.0)), cx);
        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_background: WindowBackgroundAppearance::Blurred,
                ..Default::default()
            },
            |_, cx| cx.new(|_| BlurGallery),
        ) {
            eprintln!("Failed to open blur gallery: {error:#}");
            cx.quit();
        }
        cx.activate(true);
    });
    Ok(())
}
