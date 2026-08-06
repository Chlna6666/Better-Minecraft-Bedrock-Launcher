use super::helpers::*;
use super::model::*;
use super::prelude::*;
use crate::ui::components::icon::themed_icon;
use lucide_gpui::icons as lucide_icons;

impl MapViewerWindowView {
    pub(super) fn top_bar_snapshot(&self) -> MapTopBarSnapshot {
        MapTopBarSnapshot {
            window_width: self.window_width,
            asset_name: self.asset.display_name.clone(),
            version_name: SharedString::from(self.version.display_name()),
            mode: self.mode,
            dimension: self.dimension,
            y_layer: self.y_layer,
            zoom_percent: self.viewport.scale * 100.0,
            activity: SharedString::from(compact_activity_label(self)),
            chunk_transfer_progress: self.professional.chunk_transfer_progress.clone(),
        }
    }

    pub(super) fn tool_stripe_snapshot(&self) -> MapToolStripeSnapshot {
        MapToolStripeSnapshot {
            left_panel_open: self.ui_state.left_panel_open,
            right_panel_open: self.ui_state.right_panel_open,
            bottom_panel_open: self.ui_state.bottom_panel_open,
            active_bottom_tab: self.ui_state.active_bottom_tab,
            active_right_panel: self.ui_state.active_right_panel,
        }
    }

    pub(super) fn menu_overlay_snapshot(&self) -> MapMenuOverlaySnapshot {
        MapMenuOverlaySnapshot {
            open: self.ui_state.top_more_open || self.context_menu.is_some(),
        }
    }

    pub(super) fn render_dock_drag_overlay(&self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .occlude()
            .cursor(match self.ui_state.dock_drag.map(|drag| drag.drag) {
                Some(DockDrag::RightPanel) => CursorStyle::ResizeColumn,
                Some(DockDrag::BottomPanel) => CursorStyle::ResizeRow,
                None => CursorStyle::Arrow,
            })
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if !event.dragging() {
                    this.release_pointer_captures(
                        "dock overlay mouse move without pressed button",
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                }
                this.update_dock_drag(event.position, cx);
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("dock overlay mouse up", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("dock overlay mouse up out", cx);
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(|_event, _window, cx| cx.stop_propagation())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                    this.release_pointer_captures("dock overlay stale mouse down", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseDownEvent, _window, cx| {
                    this.release_pointer_captures("dock overlay stale right mouse down", cx);
                    cx.stop_propagation();
                }),
            )
    }

    pub(super) fn render_menu_overlay(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        let has_menu = self.ui_state.top_more_open || self.context_menu.is_some();
        div().absolute().inset_0().when(has_menu, |this| {
            this.child(self.menu_overlay_view.clone()).child(
                div()
                    .absolute()
                    .inset_0()
                    .when(self.ui_state.top_more_open, |this| {
                        this.child(self.render_top_more_menu(colors, cx))
                    })
                    .when_some(self.context_menu, |this, menu| {
                        this.child(self.render_context_menu(colors, menu, cx))
                    }),
            )
        })
    }

    pub(super) fn render_workspace(&self, colors: &ThemeColors, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .overflow_hidden()
            .bg(colors.surface)
    }
}

pub(super) fn dimension_label(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "主世界".to_string(),
        Dimension::Nether => "下界".to_string(),
        Dimension::End => "末地".to_string(),
        Dimension::Unknown(id) => format!("维度 {id}"),
    }
}

pub(super) fn compact_activity_label(view: &MapViewerWindowView) -> String {
    if let Some(progress) = view.professional.chunk_transfer_progress.as_ref() {
        return progress.label().to_string();
    }
    if view.metadata_loading {
        return "扫描中".to_string();
    }
    if view.render_batch_active {
        let running_batches = view.render_cancels.len();
        return format!(
            "加载 {} · 批次 {running_batches}",
            view.tile_manager.loading_count()
        );
    }
    let queued = view.tile_manager.queued_count();
    if queued > 0 {
        return format!("等待 {queued}");
    }
    if view.tile_manager.failed_count() > 0 {
        return format!("失败 {}", view.tile_manager.failed_count());
    }
    if view.tile_manager.invalid_count() > 0 {
        return format!("空 {}", view.tile_manager.invalid_count());
    }
    "就绪".to_string()
}

pub(super) fn panel_title(colors: &ThemeColors, title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(title.into())
}

pub(super) fn panel_section_body(colors: &ThemeColors) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        // No border box: sections are separated by whitespace + a header row,
        // reducing the visual noise the old bordered card produced.
        .child(div().h(px(1.0)).w_full().bg(Hsla {
            a: CHROME_HAIRLINE_ALPHA,
            ..colors.border
        }))
}

/// Icon + label header for a left-dock section.
pub(super) fn panel_section_header(
    colors: &ThemeColors,
    icon_path: &'static str,
    title: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_muted)
        .child(themed_icon(
            icon_path,
            CHROME_TOOLBAR_ICON_SIZE,
            colors.text_muted,
        ))
        .child(title.into())
}

pub(super) fn panel_field_label(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_secondary)
        .child(label.into())
}

pub(super) fn dock_close_button(colors: &ThemeColors) -> Div {
    div()
        .size(px(30.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .cursor(CursorStyle::PointingHand)
        .text_color(colors.text_secondary)
        .hover(|style| {
            style.bg(Hsla {
                a: CHROME_ELEVATED_ALPHA,
                ..colors.surface_hover
            })
        })
        .child(themed_icon(
            lucide_icons::icon_x(),
            CHROME_TAB_ICON_SIZE,
            colors.text_secondary,
        ))
}

/// Tab button variant with a leading icon (for the bottom dock tab strip).
pub(super) fn tab_button_with_icon(
    colors: &ThemeColors,
    icon_path: &'static str,
    label: impl Into<SharedString>,
    active: bool,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(crate::ui::theme::tokens::radius::XS))
        .text_size(px(12.0))
        .cursor(CursorStyle::PointingHand)
        .text_color(if active {
            colors.text_primary
        } else {
            colors.text_secondary
        })
        .bg(if active {
            Hsla {
                a: 0.20,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.0,
                ..colors.surface
            }
        })
        .hover(|style| {
            style.bg(Hsla {
                a: CHROME_ELEVATED_ALPHA,
                ..colors.surface_hover
            })
        })
        .child(themed_icon(
            icon_path,
            CHROME_TAB_ICON_SIZE,
            colors.text_muted,
        ))
        .child(label.into())
}

pub(super) fn db_node_icon(kind: &DbTreeNodeKind) -> &'static str {
    match kind {
        DbTreeNodeKind::Dimension(_) => "◇",
        DbTreeNodeKind::Chunk(_) => "▣",
    }
}

pub(super) fn overlay_panel(colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .px(px(8.0))
        .py(px(8.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.86,
            ..colors.surface
        })
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_scroll_wheel(|_event, _window, cx| cx.stop_propagation())
}

pub(super) fn separator(colors: &ThemeColors) -> Div {
    div().w(px(1.0)).h(px(22.0)).bg(Hsla {
        a: CHROME_HAIRLINE_ALPHA,
        ..colors.border
    })
}

pub(super) fn toolbar_button(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(10.0))
        .py(px(6.0))
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
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(label.into())
}

pub(super) fn mode_button(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    active: bool,
) -> Div {
    div()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(if active { colors.accent } else { colors.border })
        .bg(if active {
            Hsla {
                a: 0.18,
                ..colors.accent
            }
        } else {
            Hsla {
                a: CHROME_ELEVATED_ALPHA,
                ..colors.surface_hover
            }
        })
        .hover(|style| {
            if active {
                style
            } else {
                style.bg(Hsla {
                    a: CHROME_ELEVATED_ALPHA + 0.15,
                    ..colors.surface_hover
                })
            }
        })
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if active {
            colors.accent
        } else {
            colors.text_primary
        })
        .child(label.into())
}

pub(super) fn status_badge(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(9.0))
        .py(px(5.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: CHROME_ELEVATED_ALPHA,
            ..colors.surface_hover
        })
        .text_size(px(12.0))
        .text_color(colors.text_secondary)
        .child(label.into())
}

pub(super) fn danger_button(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    div()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: 0.40,
            ..colors.danger
        })
        .bg(Hsla {
            a: 0.14,
            ..colors.danger
        })
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.danger)
        .child(label.into())
}
