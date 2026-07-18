use super::actions::MapViewerAction;
use super::layout::{
    CHROME_ELEVATED_ALPHA, CHROME_HAIRLINE_ALPHA, CHROME_ICON_SIZE, CHROME_SURFACE_ALPHA,
    IDE_TOP_BAR_HEIGHT, top_toolbar_layout,
};
use super::model::{ChunkTransferProgress, ViewerMode};
use super::panels::{mode_button, status_badge};
use crate::ui::components::icon::themed_icon;
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
            .child(render_title(&snapshot, layout.title_width, &colors))
            .when(!layout.show_modes, |this| {
                this.child(status_badge(&colors, viewer_mode_label(snapshot.mode)))
            })
            .when(layout.show_modes, |this| {
                this.child(toolbar_group(&colors).children(mode_buttons(
                    snapshot.mode,
                    &colors,
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
                        .child(stepper_name(&colors, "缩放"))
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
                top_command_button(&colors, lucide_icons::icon_upload(), "导入").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _event, _window, cx| {
                        cx.emit(MapViewerAction::ImportStructureFile);
                    }),
                ),
            )
            .child(
                top_command_button(&colors, lucide_icons::icon_chevron_down(), "更多")
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

fn render_title(snapshot: &MapTopBarSnapshot, width: f32, colors: &ThemeColors) -> Div {
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
                        .child("Bedrock Map"),
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
                            dimension_label(snapshot.dimension)
                        )),
                ),
        )
}

fn mode_buttons(
    active: ViewerMode,
    colors: &ThemeColors,
    cx: &mut Context<MapTopBarView>,
) -> Vec<gpui::AnyElement> {
    [
        (ViewerMode::Surface, "地形"),
        (ViewerMode::Biome, "群系"),
        (ViewerMode::Height, "高度"),
        (ViewerMode::Layer, "Y层"),
        (ViewerMode::Cave, "洞穴"),
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
        .rounded(px(8.0))
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

fn stepper_name(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .px(px(5.0))
        .text_size(px(11.0))
        .text_color(colors.text_secondary)
        .child(label)
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
        .rounded(px(6.0))
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

fn top_command_button(colors: &ThemeColors, icon_path: &'static str, label: &'static str) -> Div {
    div()
        .h(px(36.0))
        .px(px(9.0))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(8.0))
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
        .child(label)
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

fn viewer_mode_label(mode: ViewerMode) -> &'static str {
    match mode {
        ViewerMode::Surface => "地形",
        ViewerMode::Biome => "群系",
        ViewerMode::Height => "高度",
        ViewerMode::Layer => "Y层",
        ViewerMode::Cave => "洞穴",
    }
}

fn dimension_label(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "主世界".to_string(),
        Dimension::Nether => "下界".to_string(),
        Dimension::End => "末地".to_string(),
        Dimension::Unknown(id) => format!("维度 {id}"),
    }
}

fn transfer_progress_badge(colors: &ThemeColors, progress: &ChunkTransferProgress) -> Div {
    div()
        .w(px(146.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(8.0))
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
