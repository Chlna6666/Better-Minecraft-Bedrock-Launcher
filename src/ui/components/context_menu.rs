use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::theme::colors::ThemeColors;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Bounds, BoxShadow, CursorStyle, Div, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Point, RenderOnce, SharedString, Styled, div, hsla, point, px, rgb,
};

const MENU_MARGIN: f32 = 8.0;
const DEFAULT_WIDTH: f32 = 268.0;
const DEFAULT_MAX_HEIGHT: f32 = 420.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextMenuPlacement {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub max_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextMenuAnchor {
    Cursor(Point<Pixels>),
    RectEdge(Bounds<Pixels>),
}

#[must_use]
pub fn place_context_menu(
    position: Point<Pixels>,
    viewport_width: f32,
    viewport_height: f32,
    width: f32,
    preferred_height: f32,
) -> ContextMenuPlacement {
    let width = width
        .max(160.0)
        .min((viewport_width - MENU_MARGIN * 2.0).max(160.0));
    let available_height = (viewport_height - MENU_MARGIN * 2.0).max(96.0);
    let max_height = preferred_height.min(available_height).max(96.0);
    let left = (position.x / px(1.0))
        .min((viewport_width - width - MENU_MARGIN).max(MENU_MARGIN))
        .max(MENU_MARGIN);
    let top = (position.y / px(1.0))
        .min((viewport_height - max_height - MENU_MARGIN).max(MENU_MARGIN))
        .max(MENU_MARGIN);

    ContextMenuPlacement {
        left,
        top,
        width,
        max_height,
    }
}

#[must_use]
pub fn place_context_menu_at_anchor(
    anchor: ContextMenuAnchor,
    viewport_width: f32,
    viewport_height: f32,
    width: f32,
    preferred_height: f32,
) -> ContextMenuPlacement {
    match anchor {
        ContextMenuAnchor::Cursor(position) => place_context_menu(
            position,
            viewport_width,
            viewport_height,
            width,
            preferred_height,
        ),
        ContextMenuAnchor::RectEdge(bounds) => {
            let anchor_left = bounds.left() / px(1.0);
            let anchor_right = bounds.right() / px(1.0);
            let anchor_bottom = bounds.bottom() / px(1.0);
            let preferred_left = (anchor_right - width).max(anchor_left);
            place_context_menu(
                point(px(preferred_left), px(anchor_bottom + 4.0)),
                viewport_width,
                viewport_height,
                width,
                preferred_height,
            )
        }
    }
}

pub struct ContextMenuItem {
    pub label: SharedString,
    pub description: Option<SharedString>,
    pub checked: bool,
    pub disabled: bool,
    pub danger: bool,
    on_click: Option<Box<dyn Fn(&mut App) + 'static>>,
}

impl ContextMenuItem {
    #[must_use]
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            description: None,
            checked: false,
            disabled: false,
            danger: false,
            on_click: None,
        }
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub const fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    #[must_use]
    pub const fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    #[must_use]
    pub fn on_click(mut self, on_click: impl Fn(&mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(on_click));
        self
    }
}

pub struct ContextMenuGroup {
    pub title: Option<SharedString>,
    pub items: Vec<ContextMenuEntry>,
}

impl ContextMenuGroup {
    #[must_use]
    pub fn new(items: Vec<ContextMenuEntry>) -> Self {
        Self { title: None, items }
    }

    #[must_use]
    pub fn titled(title: impl Into<SharedString>, items: Vec<ContextMenuEntry>) -> Self {
        Self {
            title: Some(title.into()),
            items,
        }
    }
}

pub enum ContextMenuEntry {
    Item(ContextMenuItem),
    Submenu {
        label: SharedString,
        expanded: bool,
        items: Vec<ContextMenuItem>,
        on_toggle: Option<Box<dyn Fn(&mut App) + 'static>>,
    },
}

impl ContextMenuEntry {
    #[must_use]
    pub fn item(item: ContextMenuItem) -> Self {
        Self::Item(item)
    }

    #[must_use]
    pub fn submenu(
        label: impl Into<SharedString>,
        expanded: bool,
        items: Vec<ContextMenuItem>,
        on_toggle: impl Fn(&mut App) + 'static,
    ) -> Self {
        Self::Submenu {
            label: label.into(),
            expanded,
            items,
            on_toggle: Some(Box::new(on_toggle)),
        }
    }
}

#[derive(IntoElement)]
pub struct ContextMenu {
    header: Option<SharedString>,
    groups: Vec<ContextMenuGroup>,
    colors: ThemeColors,
    placement: ContextMenuPlacement,
}

impl ContextMenu {
    #[must_use]
    pub fn new(colors: &ThemeColors, groups: Vec<ContextMenuGroup>) -> Self {
        Self {
            header: None,
            groups,
            colors: *colors,
            placement: ContextMenuPlacement {
                left: MENU_MARGIN,
                top: MENU_MARGIN,
                width: DEFAULT_WIDTH,
                max_height: DEFAULT_MAX_HEIGHT,
            },
        }
    }

    #[must_use]
    pub fn header(mut self, header: impl Into<SharedString>) -> Self {
        self.header = Some(header.into());
        self
    }

    #[must_use]
    pub const fn placement(mut self, placement: ContextMenuPlacement) -> Self {
        self.placement = placement;
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.colors;
        div()
            .absolute()
            .left(px(self.placement.left))
            .top(px(self.placement.top))
            .w(px(self.placement.width))
            .max_h(px(self.placement.max_height))
            .rounded(px(8.0))
            .border_1()
            .border_color(hsla(
                colors.border.h,
                colors.border.s,
                colors.border.l,
                0.24,
            ))
            .bg(hsla(
                colors.surface.h,
                colors.surface.s,
                colors.surface.l,
                0.98,
            ))
            .shadow(vec![BoxShadow {
                color: Hsla {
                    a: 0.30,
                    ..rgb(0x000000).into()
                },
                offset: point(px(0.0), px(12.0)),
                blur_radius: px(28.0),
                spread_radius: px(0.0),
            }])
            .flex()
            .flex_col()
            .overflow_hidden()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .on_mouse_down(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .on_mouse_up(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .on_mouse_up(MouseButton::Right, |_event, _window, cx| {
                cx.stop_propagation()
            })
            .on_scroll_wheel(|_event, _window, cx| cx.stop_propagation())
            .when_some(self.header, |this, header| {
                this.child(
                    div()
                        .px(px(12.0))
                        .py(px(9.0))
                        .border_b_1()
                        .border_color(hsla(
                            colors.border.h,
                            colors.border.s,
                            colors.border.l,
                            0.16,
                        ))
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(header),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .py(px(5.0))
                    .overflow_y_scrollbar()
                    .children(
                        self.groups
                            .into_iter()
                            .enumerate()
                            .map(move |(index, group)| {
                                render_group(colors, group, index > 0).into_any_element()
                            }),
                    ),
            )
    }
}

fn render_group(colors: ThemeColors, group: ContextMenuGroup, separated: bool) -> Div {
    div()
        .flex()
        .flex_col()
        .when(separated, |this| {
            this.mt(px(5.0)).pt(px(5.0)).border_t_1().border_color(hsla(
                colors.border.h,
                colors.border.s,
                colors.border.l,
                0.14,
            ))
        })
        .when_some(group.title, |this, title| {
            this.child(
                div()
                    .px(px(12.0))
                    .py(px(4.0))
                    .text_size(px(10.0))
                    .text_color(colors.text_muted)
                    .child(title),
            )
        })
        .children(
            group
                .items
                .into_iter()
                .map(move |entry| render_entry(colors, entry).into_any_element()),
        )
}

fn render_entry(colors: ThemeColors, entry: ContextMenuEntry) -> Div {
    match entry {
        ContextMenuEntry::Item(item) => render_item(colors, item),
        ContextMenuEntry::Submenu {
            label,
            expanded,
            items,
            on_toggle,
        } => {
            let toggle = on_toggle;
            div()
                .flex()
                .flex_col()
                .occlude()
                .child(
                    menu_row(colors, false, false)
                        .child(indicator(if expanded { "⌄" } else { "›" }, colors))
                        .child(div().flex_1().child(label))
                        .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                            cx.stop_propagation();
                            if let Some(on_toggle) = toggle.as_ref() {
                                on_toggle(cx);
                            }
                        }),
                )
                .when(expanded, |this| {
                    this.children(
                        items.into_iter().map(move |item| {
                            render_item(colors, item).pl(px(22.0)).into_any_element()
                        }),
                    )
                })
        }
    }
}

fn render_item(colors: ThemeColors, item: ContextMenuItem) -> Div {
    let disabled = item.disabled;
    let danger = item.danger;
    let on_click = item.on_click;
    menu_row(colors, disabled, danger)
        .child(indicator(if item.checked { "✓" } else { "" }, colors))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .child(item.label)
                .when_some(item.description, |this, description| {
                    this.child(
                        div()
                            .text_size(px(10.0))
                            .text_color(colors.text_muted)
                            .child(description),
                    )
                }),
        )
        .when(!disabled, |this| {
            this.on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                cx.stop_propagation();
                if let Some(on_click) = on_click.as_ref() {
                    on_click(cx);
                }
            })
        })
}

fn menu_row(colors: ThemeColors, disabled: bool, danger: bool) -> Div {
    div()
        .mx(px(5.0))
        .px(px(7.0))
        .py(px(7.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .text_size(px(12.0))
        .text_color(if disabled {
            colors.text_muted
        } else if danger {
            colors.danger
        } else {
            colors.text_primary
        })
        .when(!disabled, |this| {
            this.cursor(CursorStyle::PointingHand).hover(|style| {
                style.bg(hsla(
                    colors.surface_hover.h,
                    colors.surface_hover.s,
                    colors.surface_hover.l,
                    0.82,
                ))
            })
        })
}

fn indicator(label: &'static str, colors: ThemeColors) -> Div {
    div()
        .w(px(14.0))
        .flex_none()
        .text_size(px(12.0))
        .text_color(colors.text_muted)
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, point, size};

    #[test]
    fn context_menu_stays_inside_viewport() {
        let placement = place_context_menu(point(px(790.0), px(590.0)), 800.0, 600.0, 260.0, 480.0);
        assert!(placement.left + placement.width <= 792.0);
        assert!(placement.top + placement.max_height <= 592.0);
        assert!(placement.left >= 8.0);
        assert!(placement.top >= 8.0);
    }

    #[test]
    fn context_menu_uses_scrollable_height_for_small_viewports() {
        let placement = place_context_menu(point(px(10.0), px(10.0)), 320.0, 180.0, 260.0, 480.0);
        assert_eq!(placement.max_height, 164.0);
    }

    #[test]
    fn context_menu_rect_anchor_places_menu_below_button() {
        let button = Bounds::new(point(px(680.0), px(8.0)), size(px(96.0), px(36.0)));
        let placement = place_context_menu_at_anchor(
            ContextMenuAnchor::RectEdge(button),
            800.0,
            600.0,
            260.0,
            420.0,
        );

        assert!(placement.left + placement.width <= 792.0);
        assert!(placement.top >= 48.0);
    }
}
