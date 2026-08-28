use super::actions::MapViewerAction;
use super::layout::{
    CHROME_ELEVATED_ALPHA, CHROME_HAIRLINE_ALPHA, CHROME_ICON_SIZE, CHROME_SURFACE_ALPHA,
    IDE_TOP_BAR_HEIGHT, top_toolbar_layout,
};
use super::model::{ChunkTransferProgress, ViewerMode};
use super::panels::{mode_button, status_badge};
use crate::ui::components::icon::themed_icon;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use bedrock_render::Dimension;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Context, CursorStyle, Div, EventEmitter, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, SharedString, Styled, Window, div, px, relative,
};
use lucide_gpui::icons as lucide_icons;

#[derive(Clone, Debug, PartialEq)]
pub struct MapTopBarSnapshot {
    pub window_width: f32,
    pub asset_name: SharedString,
    pub version_name: SharedString,
    pub mode: ViewerMode,
    pub dimension: Dimension,
    pub y_layer: i32,
    pub zoom_percent: f32,
    pub activity: SharedString,
    pub chunk_transfer_progress: Option<ChunkTransferProgress>,
}

#[derive(Default)]
pub struct MapTopBarView {
    snapshot: Option<MapTopBarSnapshot>,
}

impl MapTopBarView {
    pub fn set_snapshot(&mut self, snapshot: MapTopBarSnapshot, cx: &mut Context<Self>) {
        if self.snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }
}

impl EventEmitter<MapViewerAction> for MapTopBarView {}

impl Render for MapTopBarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme_colors(cx);
        let i18n = cx.global::<I18n>().clone();
        let Some(snapshot) = self.snapshot.clone() else {
            return div().h(px(IDE_TOP_BAR_HEIGHT)).into_any_element();
        };
        let layout = top_toolbar_layout(snapshot.window_width);

        div()
            .h(px(IDE_TOP_BAR_HEIGHT))
            .flex_none()
            .px(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(Hsla {
                a: CHROME_HAIRLINE_ALPHA,
                ..colors.border
            })
            .bg(Hsla {
                a: CHROME_SURFACE_ALPHA,
                ..colors.surface
            })
            .flex()
            .items_center()
            .gap(px(8.0))
            .overflow_hidden()
            .child(render_title(&snapshot, layout.title_width, &colors, &i18n))
            .when(!layout.show_modes, |this| {
                this.child(status_badge(
                    &colors,
                    viewer_mode_label(&i18n, snapshot.mode),
                ))
            })
            .when(layout.show_modes, |this| {
                this.child(toolbar_group(&colors).children(mode_buttons(
                    snapshot.mode,
                    &colors,
                    &i18n,
                    cx,
                )))
            })
            .when(layout.show_y_controls, |this| {
                this.child(
                    toolbar_group(&colors)
                        .child(stepper_name(&colors, "Y"))
                        .child(
                            top_icon_button(&colors, lucide_icons::icon_minus()).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _event, _window, cx| {
                                    cx.emit(MapViewerAction::StepY(-1));
                                }),
                            ),
                        )
                        .child(stepper_value(&colors, snapshot.y_layer.to_string()))
                        .child(
                            top_icon_button(&colors, lucide_icons::icon_plus()).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _event, _window, cx| {
                                    cx.emit(MapViewerAction::StepY(1));
                                }),
                            ),
                        ),
                )
            })
            .when(layout.show_zoom_controls, |this| {
                this.child(
                    toolbar_group(&colors)
                        .child(stepper_name(&colors, t!("MapViewer.zoom")))
                        .child(
                            top_icon_button(&colors, lucide_icons::icon_minus()).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _event, _window, cx| {
                                    cx.emit(MapViewerAction::ZoomBy(0.8));
                                }),
                            ),
                        )
                        .child(stepper_value(
                            &colors,
                            format!("{:.0}%", snapshot.zoom_percent),
                        ))
                        .child(
                            top_icon_button(&colors, lucide_icons::icon_plus()).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|_this, _event, _window, cx| {
                                    cx.emit(MapViewerAction::ZoomBy(1.25));
                                }),
                            ),
                        ),
                )
            })
            .child(div().flex_1())
            .when_some(
                snapshot.chunk_transfer_progress.as_ref(),
                |this, progress| this.child(transfer_progress_badge(&colors, progress)),
            )
            .when(snapshot.chunk_transfer_progress.is_none(), |this| {
                this.child(status_badge(&colors, snapshot.activity))
            })
            .child(
                top_command_button(&colors, lucide_icons::icon_upload(), t!("MapViewer.import"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_this, _event, _window, cx| {
                            cx.emit(MapViewerAction::ImportStructureFile);
                        }),
                    ),
            )
            .child(
                top_command_button(
                    &colors,
                    lucide_icons::icon_chevron_down(),
                    t!("MapViewer.more"),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _event, _window, cx| {
                        cx.emit(MapViewerAction::ToggleTopMore);
                    }),
                ),
            )
            .into_any_element()
    }
}

fn render_title(
    snapshot: &MapTopBarSnapshot,
    width: f32,
    colors: &ThemeColors,
    i18n: &I18n,
) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.0))
        .overflow_hidden()
        .child(themed_icon(
            lucide_icons::icon_map(),
            CHROME_ICON_SIZE,
            colors.accent,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .min_w(px(0.0))
                .overflow_hidden()
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(t!("MapViewer.bedrock_map")),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.text_secondary)
                        .overflow_hidden()
                        .child(format!(
                            "{} · {} · {}",
                            snapshot.asset_name,
                            snapshot.version_name,
                            dimension_label(i18n, snapshot.dimension)
                        )),
                ),
        )
}

fn mode_buttons(
    active: ViewerMode,
    colors: &ThemeColors,
    i18n: &I18n,
    cx: &mut Context<MapTopBarView>,
) -> Vec<gpui::AnyElement> {
    [
        (ViewerMode::Surface, t!("MapViewer.mode_surface")),
        (ViewerMode::Biome, t!("MapViewer.mode_biome")),
        (ViewerMode::Height, t!("MapViewer.mode_height")),
        (ViewerMode::Layer, t!("MapViewer.mode_layer")),
        (ViewerMode::Cave, t!("MapViewer.mode_cave")),
    ]
    .into_iter()
    .map(|(mode, label)| {
        mode_button(colors, label, active == mode)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::SetMode(mode));
                }),
            )
            .into_any_element()
    })
    .collect()
}

fn toolbar_group(colors: &ThemeColors) -> Div {
    div()
        .h(px(38.0))
        .p(px(3.0))
        .flex()
        .items_center()
        .gap(px(3.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: CHROME_HAIRLINE_ALPHA,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.28,
            ..colors.surface_hover
        })
}

fn stepper_name(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(5.0))
        .text_size(px(11.0))
        .text_color(colors.text_secondary)
        .child(label.into())
}

fn stepper_value(colors: &ThemeColors, value: impl Into<SharedString>) -> Div {
    div()
        .min_w(px(34.0))
        .flex()
        .justify_center()
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(value.into())
}

fn top_icon_button(colors: &ThemeColors, icon_path: &'static str) -> Div {
    div()
        .size(px(28.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| {
            style.bg(Hsla {
                a: CHROME_ELEVATED_ALPHA,
                ..colors.surface_hover
            })
        })
        .child(themed_icon(
            icon_path,
            CHROME_ICON_SIZE - 3.0,
            colors.text_secondary,
        ))
}

fn top_command_button(
    colors: &ThemeColors,
    icon_path: &'static str,
    label: impl Into<SharedString>,
) -> Div {
    div()
        .h(px(36.0))
        .px(px(9.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: CHROME_HAIRLINE_ALPHA,
            ..colors.border
        })
        .bg(Hsla {
            a: CHROME_ELEVATED_ALPHA,
            ..colors.surface_hover
        })
        .hover(|style| {
            style.bg(Hsla {
                a: CHROME_ELEVATED_ALPHA + 0.15,
                ..colors.surface_hover
            })
        })
        .cursor(CursorStyle::PointingHand)
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(themed_icon(
            icon_path,
            CHROME_ICON_SIZE - 2.0,
            colors.text_secondary,
        ))
        .child(label.into())
}

fn theme_colors(cx: &App) -> ThemeColors {
    let theme = cx.global::<crate::ui::state::theme::ThemeState>();
    crate::ui::theme::colors::lerp_theme_colors(
        &crate::ui::theme::colors::LightColors::colors(),
        &crate::ui::theme::colors::DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    )
}

fn viewer_mode_label(i18n: &I18n, mode: ViewerMode) -> SharedString {
    match mode {
        ViewerMode::Surface => t!("MapViewer.mode_surface"),
        ViewerMode::Biome => t!("MapViewer.mode_biome"),
        ViewerMode::Height => t!("MapViewer.mode_height"),
        ViewerMode::Layer => t!("MapViewer.mode_layer"),
        ViewerMode::Cave => t!("MapViewer.mode_cave"),
    }
}

fn dimension_label(i18n: &I18n, dimension: Dimension) -> SharedString {
    match dimension {
        Dimension::Overworld => t!("MapViewer.dimension_overworld"),
        Dimension::Nether => t!("MapViewer.dimension_nether"),
        Dimension::End => t!("MapViewer.dimension_end"),
        Dimension::Unknown(id) => {
            t!("MapViewer.dimension_unknown", id = &id.to_string())
        }
    }
}

fn transfer_progress_badge(colors: &ThemeColors, progress: &ChunkTransferProgress) -> Div {
    div()
        .w(px(146.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: CHROME_ELEVATED_ALPHA,
            ..colors.surface_hover
        })
        .flex()
        .flex_col()
        .gap(px(4.0))
        .overflow_hidden()
        .child(
            div()
                .text_size(px(11.0))
                .text_color(colors.text_secondary)
                .overflow_hidden()
                .child(progress.label()),
        )
        .child(
            div()
                .w_full()
                .h(px(3.0))
                .rounded_full()
                .bg(Hsla {
                    a: CHROME_HAIRLINE_ALPHA,
                    ..colors.border
                })
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(progress.ratio()))
                        .rounded_full()
                        .bg(colors.accent),
                ),
        )
}
