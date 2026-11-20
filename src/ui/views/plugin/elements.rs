use gpui::{
    AnyElement, App, AppContext, FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton,
    ObjectFit, ParentElement, SharedString, Styled, StyledImage, Window, div, img, px,
};

use crate::ui::components::icon::themed_icon;
use crate::ui::components::input::{Input, InputEvent, InputState};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

use bmcbl_plugin_runtime::{
    Align, BadgeNode, ButtonNode, CheckboxNode, ContainerNode, IconNode, ImageFit, ImageNode,
    InputNode, LayoutDirection, LinkNode, ListNode, ProgressNode, SelectNode, SpacerNode, TextNode,
    TextSizeToken, ThemeToken, ToggleNode, ViewNode, ViewStyle, ViewTree,
};

pub fn render_validated_view_tree(
    tree: &ViewTree,
    plugin_id: &str,
    page_id: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let colors = current_theme_colors(window.animation_time(), cx);
    render_node(
        &tree.nodes,
        tree.root,
        plugin_id,
        page_id,
        window,
        cx,
        colors,
    )
}

fn render_node(
    nodes: &[ViewNode],
    index: u32,
    plugin_id: &str,
    page_id: Option<&str>,
    window: &mut Window,
    cx: &mut App,
    colors: ThemeColors,
) -> AnyElement {
    match &nodes[index as usize] {
        ViewNode::Container(ContainerNode { style, children }) => {
            let columns = style.direction == LayoutDirection::Row
                && style.align == Align::Stretch
                && style.full_width
                && children.len() == 2;
            let compact = columns && window.bounds().size.width <= px(1280.);
            let surface = (style.background == Some(ThemeToken::Surface))
                .then(|| crate::ui::components::page_shell::glass_card(&colors));
            let mut element = styled_container_on(style, &colors, surface);
            if columns {
                element = element.items_start();
                if compact {
                    element = element.flex_col();
                }
            }
            for (position, child) in children.iter().enumerate() {
                let child = render_node(nodes, *child, plugin_id, page_id, window, cx, colors);
                if columns {
                    let child = div().min_w(px(0.)).child(child);
                    element = element.child(if compact {
                        child.w_full()
                    } else if position == 0 {
                        child.flex_1()
                    } else {
                        child.w(px(330.)).flex_none()
                    });
                } else {
                    element = element.child(child);
                }
            }
            element.into_any_element()
        }
        ViewNode::Text(TextNode { text, style }) => {
            styled_text(text.clone(), style, &colors).into_any_element()
        }
        ViewNode::Button(ButtonNode {
            label,
            action_id,
            action_value,
            style,
        }) => {
            let action_id = action_id.to_string();
            let action_value = action_value.as_ref().map(|v| v.to_string());
            let plugin_id = plugin_id.to_string();
            let page_id = page_id.map(str::to_string);
            let id = SharedString::from(format!("plugin-{plugin_id}-{index}"));
            let mut button = if style.color == Some(ThemeToken::SecondaryText) {
                crate::ui::components::button::secondary_button(&colors, id, label.clone())
            } else {
                crate::ui::components::button::primary_button(&colors, id, label.clone())
            };
            if style.full_width {
                button = button.w_full();
            }
            button
                .on_mouse_up(MouseButton::Left, move |_event, _window, cx| {
                    crate::plugins::runtime::dispatch_plugin_action(
                        cx,
                        plugin_id.clone(),
                        page_id.clone(),
                        action_id.clone(),
                        action_value.clone(),
                    );
                })
                .into_any_element()
        }
        ViewNode::Input(node) => render_input(node, index, plugin_id, page_id, window, cx),
        ViewNode::Checkbox(CheckboxNode {
            label,
            checked,
            action_id,
            action_value,
            style,
        }) => {
            let plugin_id = plugin_id.to_string();
            let page_id = page_id.map(str::to_string);
            let action_id = action_id.to_string();
            let action_value = action_value
                .as_ref()
                .map(|v| v.to_string())
                .or_else(|| Some((!*checked).to_string()));
            let marker = if *checked { "✓" } else { "" };
            styled_container(style, &colors)
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(18.0))
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(token_color(ThemeToken::Border, &colors))
                        .bg(if *checked {
                            token_color(ThemeToken::Accent, &colors)
                        } else {
                            token_color(ThemeToken::Surface, &colors)
                        })
                        .text_color(colors.btn_primary_text)
                        .text_size(px(12.0))
                        .child(marker),
                )
                .child(styled_text(label.clone(), style, &colors).p_0())
                .on_mouse_up(MouseButton::Left, move |_event, _window, cx| {
                    crate::plugins::runtime::dispatch_plugin_action(
                        cx,
                        plugin_id.clone(),
                        page_id.clone(),
                        action_id.clone(),
                        action_value.clone(),
                    );
                })
                .into_any_element()
        }
        ViewNode::Toggle(ToggleNode {
            label,
            enabled,
            action_id,
            action_value,
            style,
        }) => {
            let plugin_id = plugin_id.to_string();
            let page_id = page_id.map(str::to_string);
            let action_id = action_id.to_string();
            let action_value = action_value
                .as_ref()
                .map(|v| v.to_string())
                .or_else(|| Some((!*enabled).to_string()));
            let knob_offset = if *enabled { 18.0 } else { 2.0 };
            styled_container(style, &colors)
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .child(
                    div()
                        .relative()
                        .w(px(38.0))
                        .h(px(22.0))
                        .rounded(px(999.0))
                        .bg(if *enabled {
                            token_color(ThemeToken::Accent, &colors)
                        } else {
                            token_color(ThemeToken::Border, &colors)
                        })
                        .child(
                            div()
                                .absolute()
                                .left(px(knob_offset))
                                .top(px(2.0))
                                .size(px(18.0))
                                .rounded(px(999.0))
                                .bg(colors.btn_primary_text),
                        ),
                )
                .child(styled_text(label.clone(), style, &colors).p_0())
                .on_mouse_up(MouseButton::Left, move |_event, _window, cx| {
                    crate::plugins::runtime::dispatch_plugin_action(
                        cx,
                        plugin_id.clone(),
                        page_id.clone(),
                        action_id.clone(),
                        action_value.clone(),
                    );
                })
                .into_any_element()
        }
        ViewNode::Select(SelectNode {
            label,
            action_id,
            options,
            selected,
            style,
        }) => {
            let mut element = styled_container(style, &colors)
                .gap(px(6.0))
                .child(styled_text(label.clone(), style, &colors).p_0());
            let mut row = div().flex().flex_row().flex_wrap().gap(px(6.0));
            for option in options {
                let plugin_id = plugin_id.to_string();
                let page_id = page_id.map(str::to_string);
                let action_id = action_id.to_string();
                let option_value = option.value.to_string();
                let is_selected = selected.as_deref() == Some(option.value.as_str());
                row = row.child(
                    div()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(token_color(ThemeToken::Border, &colors))
                        .bg(if is_selected {
                            token_color(ThemeToken::Accent, &colors)
                        } else {
                            token_color(ThemeToken::Surface, &colors)
                        })
                        .text_size(px(12.0))
                        .text_color(if is_selected {
                            colors.btn_primary_text
                        } else {
                            token_color(ThemeToken::PrimaryText, &colors)
                        })
                        .cursor_pointer()
                        .child(SharedString::from(option.label.clone()))
                        .on_mouse_up(MouseButton::Left, move |_event, _window, cx| {
                            crate::plugins::runtime::dispatch_plugin_action(
                                cx,
                                plugin_id.clone(),
                                page_id.clone(),
                                action_id.clone(),
                                Some(option_value.clone()),
                            );
                        }),
                );
            }
            element = element.child(row);
            element.into_any_element()
        }
        ViewNode::Progress(ProgressNode {
            label,
            value,
            total,
            style,
        }) => {
            let ratio = total
                .filter(|total| *total > 0)
                .map(|total| (*value as f64 / total as f64).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let width_percent = (ratio * 100.0) as f32;
            styled_container(style, &colors)
                .gap(px(6.0))
                .child(styled_text(label.clone(), style, &colors).p_0())
                .child(
                    div()
                        .w_full()
                        .h(px(7.0))
                        .rounded(px(999.0))
                        .bg(token_color(ThemeToken::Border, &colors))
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(px(width_percent.max(2.0)))
                                .bg(token_color(ThemeToken::Accent, &colors)),
                        ),
                )
                .into_any_element()
        }
        ViewNode::Link(LinkNode {
            label,
            url,
            tooltip: _,
            style,
        }) => {
            let plugin_id = plugin_id.to_string();
            let url = url.to_string();
            styled_text(label.clone(), style, &colors)
                .cursor_pointer()
                .on_mouse_up(MouseButton::Left, move |_event, _window, cx| {
                    if let Err(error) =
                        crate::plugins::runtime::open_plugin_link(cx, &plugin_id, &url)
                    {
                        tracing::warn!(plugin_id, error = %error, "plugin link denied");
                    }
                })
                .into_any_element()
        }
        ViewNode::ItemList(ListNode { items, style }) => {
            let mut element = styled_container(style, &colors);
            for item in items {
                element = element.child(render_node(
                    nodes, *item, plugin_id, page_id, window, cx, colors,
                ));
            }
            element.into_any_element()
        }
        ViewNode::Separator => div()
            .h(px(1.0))
            .w_full()
            .bg(token_color(ThemeToken::Border, &colors))
            .into_any_element(),
        ViewNode::Badge(BadgeNode { label, style }) => styled_text(label.clone(), style, &colors)
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(8.0))
            .bg(token_color(
                style.background.unwrap_or(ThemeToken::Surface),
                &colors,
            ))
            .into_any_element(),
        ViewNode::Icon(IconNode { name, style }) => {
            let path = icon_path(name);
            let color = token_color(style.color.unwrap_or(ThemeToken::SecondaryText), &colors);
            div()
                .flex()
                .items_center()
                .justify_center()
                .child(themed_icon(path, 18.0, color))
                .into_any_element()
        }
        ViewNode::Image(ImageNode {
            src,
            alt: _alt,
            caption,
            placeholder,
            fallback,
            style,
            height,
            min_height,
            max_height,
            aspect_ratio_x,
            aspect_ratio_y,
            corner_radius,
            fit,
        }) => {
            let Some(source) = crate::plugins::runtime::plugin_image(cx, plugin_id, src) else {
                return image_replacement_element(
                    image_state_label(fallback, "Image unavailable"),
                    colors,
                )
                .into_any_element();
            };
            let radius = px(f32::from(corner_radius.unwrap_or(16)));
            let object_fit = match fit {
                ImageFit::Cover => ObjectFit::Cover,
                ImageFit::Contain => ObjectFit::Contain,
            };
            let frame_height = image_frame_height(
                *height,
                *min_height,
                *max_height,
                *aspect_ratio_x,
                *aspect_ratio_y,
            );
            let loading_label = image_state_label(placeholder, "Loading image...");
            let fallback_label = image_state_label(fallback, "Image unavailable");
            let image_surface = div()
                .relative()
                .w_full()
                .h(px(frame_height))
                .rounded(radius)
                .overflow_hidden()
                .bg(token_color(ThemeToken::Surface, &colors))
                .child(
                    img(source)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .rounded(radius)
                        .object_fit(object_fit)
                        .render_to_bounds()
                        .with_loading({
                            let loading_label = loading_label.clone();
                            let colors = colors;
                            move || {
                                image_replacement_element(loading_label.clone(), colors)
                                    .into_any_element()
                            }
                        })
                        .with_fallback({
                            let fallback_label = fallback_label.clone();
                            let colors = colors;
                            move || {
                                image_replacement_element(fallback_label.clone(), colors)
                                    .into_any_element()
                            }
                        }),
                );
            let mut frame = styled_container(style, &colors)
                .w_full()
                .rounded(radius)
                .overflow_hidden()
                .child(image_surface);
            let caption_text = caption.trim();
            if !caption_text.is_empty() {
                frame = frame
                    .bg(token_color(
                        style.background.unwrap_or(ThemeToken::Surface),
                        &colors,
                    ))
                    .border_1()
                    .border_color(token_color(ThemeToken::Border, &colors))
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .text_size(px(11.0))
                            .text_color(token_color(ThemeToken::MutedText, &colors))
                            .child(SharedString::from(caption_text.to_string())),
                    );
            }
            frame.into_any_element()
        }
        ViewNode::Spacer(SpacerNode { size }) => div().h(px(f32::from(*size))).into_any_element(),
    }
}

struct PluginInput {
    input: gpui::Entity<InputState>,
    value: String,
    placeholder: String,
    _events: gpui::Subscription,
}

fn render_input(
    node: &InputNode,
    index: u32,
    plugin_id: &str,
    page_id: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let key = SharedString::from(format!(
        "plugin-input-{plugin_id}-{}-{}-{index}",
        page_id.unwrap_or_default(),
        node.action_id
    ));
    let state = window.use_keyed_state(key, cx, |window, cx| {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_text(node.value.clone(), cx);
            input.set_placeholder(node.placeholder.clone(), cx);
            input
        });
        let plugin = plugin_id.to_owned();
        let page = page_id.map(str::to_owned);
        let action = node.action_id.clone();
        let events = window.subscribe(&input, cx, move |input, event, _, cx| {
            let action_id = match event {
                InputEvent::Change => action.clone(),
                InputEvent::PressEnter { .. } => format!("{action}-submit"),
                InputEvent::Focus | InputEvent::Blur => return,
            };
            let value = input.read(cx).value().to_string();
            crate::plugins::runtime::dispatch_plugin_action(
                cx,
                plugin.clone(),
                page.clone(),
                action_id,
                Some(value),
            );
        });
        PluginInput {
            input,
            value: node.value.clone(),
            placeholder: node.placeholder.clone(),
            _events: events,
        }
    });
    let input = state.update(cx, |state, cx| {
        if state.value != node.value || state.placeholder != node.placeholder {
            state.value = node.value.clone();
            state.placeholder = node.placeholder.clone();
            state.input.update(cx, |input, cx| {
                if input.value().as_ref() != state.value {
                    input.set_text(state.value.clone(), cx);
                }
                input.set_placeholder(state.placeholder.clone(), cx);
            });
        }
        state.input.clone()
    });
    let control = Input::new(&input).min_w(px(0.));
    if node.style.full_width {
        control.w_full().into_any_element()
    } else {
        control.into_any_element()
    }
}

fn styled_container(style: &ViewStyle, colors: &ThemeColors) -> gpui::Div {
    styled_container_on(style, colors, None)
}

fn styled_container_on(
    style: &ViewStyle,
    colors: &ThemeColors,
    surface: Option<gpui::Div>,
) -> gpui::Div {
    let has_surface = surface.is_some();
    let mut element = surface
        .unwrap_or_else(div)
        .flex()
        .gap(px(f32::from(style.gap)));
    element = match style.direction {
        LayoutDirection::Row => element.flex_row(),
        LayoutDirection::Column => element.flex_col(),
    };
    element = match style.align {
        Align::Start => element.items_start(),
        Align::Center => element.items_center(),
        Align::End => element.items_end(),
        Align::Stretch => element,
    };
    if style.padding > 0 {
        element = element.p(px(f32::from(style.padding)));
    }
    if !has_surface && let Some(background) = style.background {
        element = element.bg(token_color(background, colors));
    }
    if style.full_width {
        element = element.w_full();
    }
    if let Some(radius) = style.corner_radius {
        element = element.rounded(px(f32::from(radius)));
    }
    element
}

fn styled_text(
    text: impl Into<SharedString>,
    style: &ViewStyle,
    colors: &ThemeColors,
) -> gpui::Div {
    let mut element = styled_container(style, colors)
        .text_color(token_color(
            style.color.unwrap_or(ThemeToken::PrimaryText),
            colors,
        ))
        .text_size(text_size(style.text_size))
        .child(text.into());

    if style.emphasis {
        element = element.font_weight(FontWeight::SEMIBOLD);
    }

    element
}

fn image_frame_height(
    height: Option<u16>,
    min_height: Option<u16>,
    max_height: Option<u16>,
    aspect_ratio_x: Option<u16>,
    aspect_ratio_y: Option<u16>,
) -> f32 {
    let base = if let Some(height) = height {
        f32::from(height)
    } else if let (Some(width), Some(height)) = (aspect_ratio_x, aspect_ratio_y) {
        320.0 * (f32::from(height) / f32::from(width).max(1.0))
    } else {
        180.0
    };
    let minimum = f32::from(min_height.unwrap_or(96));
    let maximum = f32::from(max_height.unwrap_or(420)).max(minimum);
    base.clamp(minimum, maximum)
}

fn image_state_label(value: &str, default_value: &'static str) -> SharedString {
    let label = value.trim();
    if label.is_empty() {
        SharedString::from(default_value)
    } else {
        SharedString::from(label.to_string())
    }
}

fn image_replacement_element(label: SharedString, colors: ThemeColors) -> gpui::Div {
    div()
        .flex()
        .size_full()
        .items_center()
        .justify_center()
        .bg(token_color(ThemeToken::Surface, &colors))
        .text_size(px(12.0))
        .text_color(token_color(ThemeToken::MutedText, &colors))
        .child(label)
}

fn current_theme_colors(now: std::time::Instant, cx: &App) -> ThemeColors {
    let theme = cx.global::<ThemeState>();
    lerp_theme_colors(
        &LightColors::theme(),
        &DarkColors::theme(),
        theme.factor(now),
        theme.accent,
    )
}

fn token_color(token: ThemeToken, colors: &ThemeColors) -> Hsla {
    match token {
        ThemeToken::PrimaryText => colors.text_primary,
        ThemeToken::SecondaryText => colors.text_secondary,
        ThemeToken::MutedText => colors.text_muted,
        ThemeToken::Accent => colors.accent,
        ThemeToken::Surface => colors.settings_card_bg,
        ThemeToken::Border => colors.border,
        ThemeToken::Danger => colors.danger,
    }
}

fn text_size(size: TextSizeToken) -> gpui::Pixels {
    match size {
        TextSizeToken::Small => px(12.0),
        TextSizeToken::Body => px(14.0),
        TextSizeToken::Title => px(19.0),
    }
}

pub(crate) fn icon_path(name: &str) -> &'static str {
    match name {
        "settings" => lucide_gpui::icon!(settings),
        "alert" => lucide_gpui::icon!(circle_alert),
        "star" => lucide_gpui::icon!(star),
        "plug" => lucide_gpui::icon!(plug),
        "users" => lucide_gpui::icon!(users),
        "activity" => lucide_gpui::icon!(activity),
        "network" => lucide_gpui::icon!(network),
        _ => lucide_gpui::icon!(info),
    }
}

pub fn fallback_panel(message: impl Into<SharedString>) -> gpui::Div {
    let colors = LightColors::theme();
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(14.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(token_color(ThemeToken::Border, &colors))
        .bg(token_color(ThemeToken::Surface, &colors))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(token_color(ThemeToken::Danger, &colors))
                .child("Plugin view error"),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(token_color(ThemeToken::SecondaryText, &colors))
                .child(message.into()),
        )
}

pub fn loading_panel(message: impl Into<SharedString>) -> gpui::Div {
    let colors = LightColors::theme();
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .p(px(24.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(token_color(ThemeToken::Border, &colors))
        .bg(token_color(ThemeToken::Surface, &colors))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(token_color(ThemeToken::SecondaryText, &colors))
                .child(message.into()),
        )
}
