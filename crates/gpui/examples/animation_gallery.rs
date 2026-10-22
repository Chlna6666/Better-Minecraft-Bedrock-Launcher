//! Retained animation stress gallery.
//!
//! On Windows, run with `--backend=nova-dx12` and `--backend=nova-vulkan`.
//! Visual tracks run forever in alternating directions so values remain
//! continuous at each cycle boundary. The example compares retained visual
//! tracks with layout/color callback tracks.
//!
//! The gallery covers opacity, translation, relative translation, scale,
//! rotation, blur, horizontal/vertical reveal, captured subtree translation,
//! scale-opacity, translation-opacity, parallel groups, spring, custom easing,
//! repeated retargeting, and a layout/color callback fallback. `--copies=N`
//! multiplies each visual track to expose CPU and upload scaling.

use std::{
    borrow::Cow,
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui::{
    App, Application, AssetSource, Bounds, Context, RendererBackend, RendererOptions, SharedString,
    Timer, TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowOptions, div,
    prelude::*, px, rgb, size, window_metrics_snapshot,
};

mod animation_gallery_support;

const INITIAL_PRESENT_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_COPIES: usize = 4;
const MAX_COPIES: usize = 64;

#[derive(Clone, Copy)]
struct Config {
    backend: RendererBackend,
    copies: usize,
    client_titlebar: bool,
}

struct ExampleAssets {
    examples: PathBuf,
}

impl AssetSource for ExampleAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let path = if path == "animation-perf-icon.png" {
            self.examples.join("image/app-icon.png")
        } else {
            self.examples.join(path)
        };
        Ok(Some(Cow::Owned(std::fs::read(path)?)))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

struct AnimationLab {
    copies: usize,
    client_titlebar: bool,
    phase: bool,
    axis_comparison_enabled: bool,
    spring_comparison_enabled: bool,
    layout_fallback_enabled: bool,
}

impl Render for AnimationLab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut cards =
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .children(animation_gallery_support::visual_cards(
                    self.copies,
                    self.phase,
                ));
        if self.axis_comparison_enabled {
            cards = cards.child(animation_gallery_support::axis_comparison_card(self.phase));
        }
        if self.spring_comparison_enabled {
            cards = cards.child(animation_gallery_support::spring_comparison_card(
                self.phase,
            ));
        }
        if self.layout_fallback_enabled {
            cards = cards
                .child(animation_gallery_support::layout_fallback_card())
                .child(animation_gallery_support::color_fallback_card());
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x111827))
            .p_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .when(self.client_titlebar, |header| header.h(px(48.0)))
                    .child(
                        div()
                            .text_lg()
                            .text_color(rgb(0xf8fafc))
                            .when(self.client_titlebar, |title| {
                                title
                                    .flex_1()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .window_control_area(WindowControlArea::Drag)
                            })
                            .child("GPUI retained animation gallery"),
                    )
                    .child(
                        div()
                            .id("retarget-all")
                            .rounded_md()
                            .bg(rgb(0x2563eb))
                            .px_3()
                            .py_2()
                            .text_color(rgb(0xffffff))
                            .child("Retarget all tracks")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.phase = !view.phase;
                                cx.notify();
                            })),
                    ),
            )
            .child(div().text_sm().text_color(rgb(0xcbd5e1)).child(format!(
                "{} copies per visual track · click Retarget while running",
                self.copies
            )))
            .child(
                div()
                    .id("animation-gallery-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(cards),
            )
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_config()?;
    let options = RendererOptions {
        backend: config.backend,
        frame_metrics: true,
        ..RendererOptions::default()
    };
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");
    Application::with_renderer_options(options)
        .with_assets(ExampleAssets { examples })
        .run(move |cx| open_lab(cx, config));
    Ok(())
}

fn open_lab(cx: &mut App, config: Config) {
    let view = cx.new(|_| AnimationLab {
        copies: config.copies,
        client_titlebar: config.client_titlebar,
        phase: false,
        axis_comparison_enabled: false,
        spring_comparison_enabled: false,
        layout_fallback_enabled: false,
    });
    let window_view = view.clone();
    let window_bounds = Bounds::centered(None, size(px(1120.0), px(820.0)), cx);
    let window_handle = match cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: config.client_titlebar.then(|| TitlebarOptions {
                appears_transparent: true,
                ..TitlebarOptions::default()
            }),
            ..WindowOptions::default()
        },
        move |_, _| window_view,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("failed to open animation gallery: {error:#}");
            cx.quit();
            return;
        }
    };
    let window_id = window_handle.window_id().as_u64();
    cx.activate(true);

    cx.spawn(async move |cx| {
        let initial_present_deadline = Instant::now() + INITIAL_PRESENT_TIMEOUT;
        while animation_sample_count(window_id) < 2 {
            if Instant::now() >= initial_present_deadline {
                eprintln!(
                    "native frame callback did not present the initial scene in {INITIAL_PRESENT_TIMEOUT:?}"
                );
                return;
            }
            Timer::after(Duration::from_millis(50)).await;
        }

        Timer::after(Duration::from_millis(1800)).await;
        if cx
            .update(|cx| {
                view.update(cx, |view, cx| {
                    view.axis_comparison_enabled = true;
                    cx.notify();
                })
            })
            .is_err()
        {
            return;
        }
        eprintln!("axis A/B phase started: compositor and layout X/Y translation only");
        Timer::after(Duration::from_secs(3)).await;
        if cx
            .update(|cx| {
                view.update(cx, |view, cx| {
                    view.axis_comparison_enabled = false;
                    view.spring_comparison_enabled = true;
                    cx.notify();
                })
            })
            .is_err()
        {
            return;
        }
        eprintln!("spring A/B phase started: same default spring on compositor and layout X/Y tracks");
        Timer::after(Duration::from_secs(3)).await;
        if cx
            .update(|cx| {
            view.update(cx, |view, cx| {
                view.layout_fallback_enabled = true;
                cx.notify();
            })
            })
            .is_err()
        {
            return;
        }
        eprintln!("layout/color fallback phase started after isolated axis A/B interval");
    })
    .detach();
}

fn animation_sample_count(window_id: u64) -> usize {
    window_metrics_snapshot()
        .into_iter()
        .find(|window| window.window_id == window_id)
        .map_or(0, |window| window.animation_sampled_present_count)
}

fn parse_config() -> Result<Config, Box<dyn Error>> {
    let mut config = Config {
        backend: default_backend(),
        copies: DEFAULT_COPIES,
        client_titlebar: false,
    };
    for argument in std::env::args().skip(1) {
        if let Some(value) = argument.strip_prefix("--backend=") {
            config.backend = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--copies=") {
            config.copies = value.parse()?;
        } else if argument == "--client-titlebar" {
            config.client_titlebar = true;
        } else if argument == "--help" || argument == "-h" {
            print_help();
            std::process::exit(0);
        } else {
            return Err(format!("unknown argument: {argument}").into());
        }
    }
    if !matches!(
        config.backend,
        RendererBackend::NovaDx12 | RendererBackend::NovaVulkan
    ) {
        return Err("--backend must be nova-dx12 or nova-vulkan".into());
    }
    if !(1..=MAX_COPIES).contains(&config.copies) {
        return Err(format!("--copies must be between 1 and {MAX_COPIES}").into());
    }
    Ok(config)
}

fn print_help() {
    println!(
        "animation_gallery [--backend=nova-dx12|nova-vulkan] [--copies=1..{MAX_COPIES}] [--client-titlebar]\n\
         Default: {DEFAULT_COPIES} copies per visual track. This is an interactive animation gallery.",
    );
}

fn default_backend() -> RendererBackend {
    if cfg!(target_os = "windows") {
        RendererBackend::NovaDx12
    } else {
        RendererBackend::NovaVulkan
    }
}
