use super::actions::MapViewerAction;
use super::layout::IDE_LEFT_STRIPE_WIDTH;
use super::state::{MapViewerBottomTab, MapViewerRightPanel};
use crate::ui::theme::colors::ThemeColors;
use gpui::{
    App, Context, CursorStyle, EventEmitter, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use lucide_gpui::icons as lucide_icons;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MapToolStripeSnapshot {
    pub left_panel_open: bool,
    pub right_panel_open: bool,
    pub bottom_panel_open: bool,
    pub active_bottom_tab: MapViewerBottomTab,
    pub active_right_panel: MapViewerRightPanel,
}

#[derive(Default)]
pub struct MapToolStripeView {
    snapshot: Option<MapToolStripeSnapshot>,
}

impl MapToolStripeView {
    pub fn set_snapshot(&mut self, snapshot: MapToolStripeSnapshot, cx: &mut Context<Self>) {
        if self.snapshot == Some(snapshot) {
            return;
        }
        self.snapshot = Some(snapshot);
        cx.notify();
    }
}

impl EventEmitter<MapViewerAction> for MapToolStripeView {}

impl Render for MapToolStripeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme_colors(cx);
        let snapshot = self.snapshot.unwrap_or(MapToolStripeSnapshot {
            left_panel_open: true,
            right_panel_open: false,
            bottom_panel_open: false,
            active_bottom_tab: MapViewerBottomTab::ChunkTree,
            active_right_panel: MapViewerRightPanel::Nbt,
        });

        div()
            .w(px(IDE_LEFT_STRIPE_WIDTH))
            .flex_none()
            .h_full()
            .min_h(px(0.0))
            .py(px(8.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.0))
            .bg(colors.surface)
            .child(stripe_button(
                "stripe-tools",
                &colors,
                lucide_icons::icon_wrench(),
                "工具",
                snapshot.left_panel_open,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleLeftPanel);
                }),
            ))
            .child(stripe_button(
                "stripe-chunks",
                &colors,
                lucide_icons::icon_layers(),
                "区块",
                snapshot.bottom_panel_open
                    && snapshot.active_bottom_tab == MapViewerBottomTab::ChunkTree,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleBottomTab(
                        MapViewerBottomTab::ChunkTree,
                    ));
                }),
            ))
            .child(stripe_button(
                "stripe-players",
                &colors,
                lucide_icons::icon_users(),
                "玩家",
                snapshot.bottom_panel_open
                    && snapshot.active_bottom_tab == MapViewerBottomTab::Players,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleBottomTab(
                        MapViewerBottomTab::Players,
                    ));
                }),
            ))
            .child(stripe_button(
                "stripe-details",
                &colors,
                lucide_icons::icon_info(),
                "详情",
                snapshot.bottom_panel_open
                    && snapshot.active_bottom_tab == MapViewerBottomTab::Details,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleBottomTab(
                        MapViewerBottomTab::Details,
                    ));
                }),
            ))
            .child(stripe_button(
                "stripe-3d",
                &colors,
                lucide_icons::icon_box(),
                "3D",
                snapshot.right_panel_open
                    && snapshot.active_right_panel == MapViewerRightPanel::Preview3d,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleRightPanel(
                        MapViewerRightPanel::Preview3d,
                    ));
                }),
            ))
            .child(stripe_button(
                "stripe-nbt",
                &colors,
                lucide_icons::icon_file_text(),
                "NBT",
                snapshot.right_panel_open
                    && snapshot.active_right_panel == MapViewerRightPanel::Nbt,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleRightPanel(MapViewerRightPanel::Nbt));
                }),
            ))
            .child(stripe_button(
                "stripe-diagnostics",
                &colors,
                lucide_icons::icon_activity(),
                "诊断",
                snapshot.bottom_panel_open
                    && snapshot.active_bottom_tab == MapViewerBottomTab::Diagnostics,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleBottomTab(
                        MapViewerBottomTab::Diagnostics,
                    ));
                }),
            ))
            .child(stripe_button(
                "stripe-history",
                &colors,
                lucide_icons::icon_history(),
                "历史",
                snapshot.bottom_panel_open
                    && snapshot.active_bottom_tab == MapViewerBottomTab::History,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(MapViewerAction::ToggleBottomTab(
                        MapViewerBottomTab::History,
                    ));
                }),
            ))
            .child(div().flex_1())
    }
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

fn stripe_button(
    id: &'static str,
    colors: &ThemeColors,
    icon_path: &'static str,
    label: &'static str,
    active: bool,
    on_click: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let accent = colors.accent;
    let muted = colors.text_muted;
    let hover_bg = Hsla {
        a: super::layout::CHROME_ELEVATED_ALPHA,
        ..colors.surface_hover
    };
    let active_bg = Hsla {
        a: super::layout::CHROME_ELEVATED_ALPHA * 0.55,
        ..accent
    };
    let foreground = if active { accent } else { muted };
    div()
        .id(id)
        .relative()
        .w(px(IDE_LEFT_STRIPE_WIDTH - 10.0))
        .h(px(46.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(3.0))
        .rounded(px(8.0))
        .cursor(CursorStyle::PointingHand)
        .bg(if active {
            active_bg
        } else {
            gpui::transparent_black()
        })
        .hover(|style| if active { style } else { style.bg(hover_bg) })
        .when(active, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(8.0))
                    .bottom(px(8.0))
                    .w(px(super::layout::CHROME_ACTIVE_RAIL_WIDTH))
                    .bg(accent),
            )
        })
        .child(crate::ui::components::icon::themed_icon(
            icon_path,
            super::layout::CHROME_ICON_SIZE,
            foreground,
        ))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(foreground)
                .child(label),
        )
        .on_mouse_down(MouseButton::Left, on_click)
}
