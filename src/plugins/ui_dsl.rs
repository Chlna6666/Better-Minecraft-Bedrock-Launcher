use anyhow::{Result, bail};
use gpui::{
    AnyElement, App, FontWeight, Hsla, ImageSource, InteractiveElement, IntoElement, MouseButton,
    ObjectFit, ParentElement, SharedString, Styled, StyledImage, Window, div, img, px,
};

use crate::ui::components::icon::themed_icon;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

pub const MAX_VIEW_DEPTH: usize = 24;
pub const MAX_VIEW_NODES: usize = 512;
pub const MAX_STRING_BYTES: usize = 8 * 1024;
pub const MAX_TOTAL_TEXT_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutDirection {
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeToken {
    PrimaryText,
    SecondaryText,
    MutedText,
    Accent,
    Surface,
    Border,
    Danger,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextSizeToken {
    Small,
    Body,
    Title,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageFit {
    Cover,
    Contain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewStyle {
    pub direction: LayoutDirection,
    pub gap: u16,
    pub padding: u16,
    pub align: Align,
    pub color: Option<ThemeToken>,
    pub background: Option<ThemeToken>,
    pub text_size: TextSizeToken,
    pub emphasis: bool,
    pub full_width: bool,
    pub corner_radius: Option<u16>,
}

impl Default for ViewStyle {
    fn default() -> Self {
        Self {
            direction: LayoutDirection::Column,
            gap: 8,
            padding: 0,
            align: Align::Start,
            color: None,
            background: None,
            text_size: TextSizeToken::Body,
            emphasis: false,
            full_width: false,
            corner_radius: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewNode {
    Container {
        style: ViewStyle,
        children: Vec<ViewNode>,
    },
    Text {
        text: String,
        style: ViewStyle,
    },
    Button {
        label: String,
        action_id: String,
        action_value: Option<String>,
        style: ViewStyle,
    },
    Input {
        value: String,
        placeholder: String,
        action_id: String,
        style: ViewStyle,
    },
    Checkbox {
        label: String,
        checked: bool,
        action_id: String,
        action_value: Option<String>,
        style: ViewStyle,
    },
    Toggle {
        label: String,
        enabled: bool,
        action_id: String,
        action_value: Option<String>,
        style: ViewStyle,
    },
    Select {
        label: String,
        action_id: String,
        options: Vec<SelectOption>,
        selected: Option<String>,
        style: ViewStyle,
    },
    Progress {
        label: String,
        value: u64,
        total: Option<u64>,
        style: ViewStyle,
    },
    Link {
        label: String,
        url: String,
        tooltip: Option<String>,
        style: ViewStyle,
    },
    List {
        items: Vec<ViewNode>,
        style: ViewStyle,
    },
    Separator,
    Badge {
        label: String,
        style: ViewStyle,
    },
    Icon {
        name: String,
        style: ViewStyle,
    },
    Image {
        src: String,
        alt: String,
        caption: String,
        placeholder: String,
        fallback: String,
        style: ViewStyle,
        height: Option<u16>,
        min_height: Option<u16>,
        max_height: Option<u16>,
        aspect_ratio_x: Option<u16>,
        aspect_ratio_y: Option<u16>,
        corner_radius: Option<u16>,
        fit: ImageFit,
    },
    Spacer {
        size: u16,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectOption {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewTree {
    pub root: ViewNode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewLimits {
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_string_bytes: usize,
    pub max_total_text_bytes: usize,
}

impl Default for ViewLimits {
    fn default() -> Self {
        Self {
            max_depth: MAX_VIEW_DEPTH,
            max_nodes: MAX_VIEW_NODES,
            max_string_bytes: MAX_STRING_BYTES,
            max_total_text_bytes: MAX_TOTAL_TEXT_BYTES,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ViewStats {
    pub nodes: usize,
    pub total_text_bytes: usize,
}

impl ViewTree {
    pub fn validate(&self) -> Result<ViewStats> {
        validate_view_tree(self, &ViewLimits::default())
    }

    pub fn estimated_retained_bytes(&self) -> usize {
        estimated_node_bytes(&self.root)
    }
}

fn estimated_style_bytes(_style: &ViewStyle) -> usize {
    std::mem::size_of::<ViewStyle>()
}

fn estimated_string_bytes(value: &str) -> usize {
    std::mem::size_of::<String>().saturating_add(value.len())
}

fn estimated_node_bytes(node: &ViewNode) -> usize {
    let base = std::mem::size_of::<ViewNode>();
    match node {
        ViewNode::Container { style, children } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(
                children
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ViewNode>()),
            )
            .saturating_add(children.iter().map(estimated_node_bytes).sum::<usize>()),
        ViewNode::Text { text, style } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(text)),
        ViewNode::Button {
            label,
            action_id,
            action_value,
            style,
        } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(label))
            .saturating_add(estimated_string_bytes(action_id))
            .saturating_add(action_value.as_deref().map_or(0, estimated_string_bytes)),
        ViewNode::Input {
            value,
            placeholder,
            action_id,
            style,
        } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(value))
            .saturating_add(estimated_string_bytes(placeholder))
            .saturating_add(estimated_string_bytes(action_id)),
        ViewNode::Checkbox {
            label,
            action_id,
            action_value,
            style,
            ..
        }
        | ViewNode::Toggle {
            label,
            action_id,
            action_value,
            style,
            ..
        } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(label))
            .saturating_add(estimated_string_bytes(action_id))
            .saturating_add(action_value.as_deref().map_or(0, estimated_string_bytes)),
        ViewNode::Select {
            label,
            action_id,
            options,
            selected,
            style,
        } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(label))
            .saturating_add(estimated_string_bytes(action_id))
            .saturating_add(selected.as_deref().map_or(0, estimated_string_bytes))
            .saturating_add(
                options
                    .capacity()
                    .saturating_mul(std::mem::size_of::<SelectOption>()),
            )
            .saturating_add(
                options
                    .iter()
                    .map(|option| {
                        estimated_string_bytes(&option.label)
                            .saturating_add(estimated_string_bytes(&option.value))
                    })
                    .sum::<usize>(),
            ),
        ViewNode::Progress { label, style, .. } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(label)),
        ViewNode::Link {
            label,
            url,
            tooltip,
            style,
        } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(label))
            .saturating_add(estimated_string_bytes(url))
            .saturating_add(tooltip.as_deref().map_or(0, estimated_string_bytes)),
        ViewNode::List { items, style } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(
                items
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ViewNode>()),
            )
            .saturating_add(items.iter().map(estimated_node_bytes).sum::<usize>()),
        ViewNode::Separator => base,
        ViewNode::Badge { label, style } | ViewNode::Icon { name: label, style } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(label)),
        ViewNode::Image {
            src,
            alt,
            caption,
            placeholder,
            fallback,
            style,
            ..
        } => base
            .saturating_add(estimated_style_bytes(style))
            .saturating_add(estimated_string_bytes(src))
            .saturating_add(estimated_string_bytes(alt))
            .saturating_add(estimated_string_bytes(caption))
            .saturating_add(estimated_string_bytes(placeholder))
            .saturating_add(estimated_string_bytes(fallback)),
        ViewNode::Spacer { .. } => base,
    }
}

pub fn validate_view_tree(tree: &ViewTree, limits: &ViewLimits) -> Result<ViewStats> {
    let mut stats = ViewStats::default();
    validate_node(&tree.root, 1, limits, &mut stats)?;
    Ok(stats)
}

fn validate_node(
    node: &ViewNode,
    depth: usize,
    limits: &ViewLimits,
    stats: &mut ViewStats,
) -> Result<()> {
    if depth > limits.max_depth {
        bail!("plugin view exceeds max depth {}", limits.max_depth);
    }

    stats.nodes += 1;
    if stats.nodes > limits.max_nodes {
        bail!("plugin view exceeds max node count {}", limits.max_nodes);
    }

    match node {
        ViewNode::Container { children, .. } => {
            for child in children {
                validate_node(child, depth + 1, limits, stats)?;
            }
        }
        ViewNode::Text { text, .. } => validate_text(text, limits, stats)?,
        ViewNode::Button {
            label,
            action_id,
            action_value: _,
            ..
        } => {
            validate_text(label, limits, stats)?;
            validate_action_id(action_id)?;
        }
        ViewNode::Input {
            value,
            placeholder,
            action_id,
            ..
        } => {
            validate_text(value, limits, stats)?;
            validate_text(placeholder, limits, stats)?;
            validate_action_id(action_id)?;
        }
        ViewNode::Checkbox {
            label,
            action_id,
            action_value,
            ..
        }
        | ViewNode::Toggle {
            label,
            action_id,
            action_value,
            ..
        } => {
            validate_text(label, limits, stats)?;
            validate_action_id(action_id)?;
            if let Some(value) = action_value {
                validate_text(value, limits, stats)?;
            }
        }
        ViewNode::Select {
            label,
            action_id,
            options,
            selected,
            ..
        } => {
            validate_text(label, limits, stats)?;
            validate_action_id(action_id)?;
            if let Some(selected) = selected {
                validate_text(selected, limits, stats)?;
            }
            for option in options {
                validate_text(&option.label, limits, stats)?;
                validate_text(&option.value, limits, stats)?;
            }
        }
        ViewNode::Progress { label, .. } => validate_text(label, limits, stats)?,
        ViewNode::Link {
            label,
            url,
            tooltip,
            ..
        } => {
            validate_text(label, limits, stats)?;
            validate_text(url, limits, stats)?;
            if let Some(tooltip) = tooltip {
                validate_text(tooltip, limits, stats)?;
            }
        }
        ViewNode::List { items, .. } => {
            for item in items {
                validate_node(item, depth + 1, limits, stats)?;
            }
        }
        ViewNode::Separator => {}
        ViewNode::Badge { label, .. } => validate_text(label, limits, stats)?,
        ViewNode::Icon { name, .. } => validate_name(name, "icon name")?,
        ViewNode::Image {
            src,
            alt,
            caption,
            placeholder,
            fallback,
            ..
        } => {
            validate_text(src, limits, stats)?;
            validate_text(alt, limits, stats)?;
            validate_text(caption, limits, stats)?;
            validate_text(placeholder, limits, stats)?;
            validate_text(fallback, limits, stats)?;
        }
        ViewNode::Spacer { .. } => {}
    }

    Ok(())
}

fn validate_text(text: &str, limits: &ViewLimits, stats: &mut ViewStats) -> Result<()> {
    if text.len() > limits.max_string_bytes {
        bail!(
            "plugin view string exceeds max length {}",
            limits.max_string_bytes
        );
    }

    stats.total_text_bytes += text.len();
    if stats.total_text_bytes > limits.max_total_text_bytes {
        bail!(
            "plugin view text exceeds total max length {}",
            limits.max_total_text_bytes
        );
    }

    Ok(())
}

fn validate_action_id(action_id: &str) -> Result<()> {
    validate_name(action_id, "action id")
}

fn validate_name(value: &str, label: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        bail!("{label} must be 1..=128 bytes");
    }

    for byte in value.bytes() {
        let allowed = byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'.' | b'-' | b'_' | b':');
        if !allowed {
            bail!("{label} contains unsupported characters");
        }
    }

    Ok(())
}

pub fn render_view_tree(
    tree: &ViewTree,
    plugin_id: &str,
    page_id: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if let Err(error) = tree.validate() {
        return fallback_panel(format!("Invalid plugin view: {error}")).into_any_element();
    }

    let colors = current_theme_colors(cx);
    render_node(&tree.root, plugin_id, page_id, window, cx, colors)
}

pub fn render_validated_view_tree(
    tree: &ViewTree,
    plugin_id: &str,
    page_id: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let colors = current_theme_colors(cx);
    render_node(&tree.root, plugin_id, page_id, window, cx, colors)
}

fn render_node(
    node: &ViewNode,
    plugin_id: &str,
    page_id: Option<&str>,
    window: &mut Window,
    cx: &mut App,
    colors: ThemeColors,
) -> AnyElement {
    match node {
        ViewNode::Container { style, children } => {
            let mut element = styled_container(style, &colors);
            for child in children {
                element = element.child(render_node(child, plugin_id, page_id, window, cx, colors));
            }
            element.into_any_element()
        }
        ViewNode::Text { text, style } => styled_text(text, style, &colors).into_any_element(),
        ViewNode::Button {
            label,
            action_id,
            action_value,
            style,
        } => {
            let action_id = action_id.clone();
            let action_value = action_value.clone();
            let plugin_id = plugin_id.to_string();
            let page_id = page_id.map(str::to_string);
            let accent = token_color(ThemeToken::Accent, &colors);
            let label_color = if style.color == Some(ThemeToken::Accent) {
                colors.btn_primary_text
            } else {
                token_color(style.color.unwrap_or(ThemeToken::PrimaryText), &colors)
            };
            let radius = f32::from(style.corner_radius.unwrap_or(8));
            let mut button = styled_container(style, &colors)
                .px(px(12.0))
                .py(px(8.0))
                .rounded(px(radius))
                .border_1()
                .border_color(Hsla { a: 0.22, ..accent })
                .bg(Hsla { a: 0.92, ..accent })
                .text_size(text_size(style.text_size))
                .text_color(label_color)
                .font_weight(if style.emphasis {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .child(SharedString::from(label.clone()))
                .cursor_pointer();
            if style.full_width {
                button = button.w_full().justify_center();
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
        ViewNode::Input {
            value,
            placeholder,
            action_id,
            style,
        } => styled_text(&format!("{placeholder}{value}"), style, &colors)
            .px(px(12.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(token_color(ThemeToken::Border, &colors))
            .child(
                div()
                    .text_size(px(11.0))
                    .opacity(0.62)
                    .child(format!("action: {action_id}")),
            )
            .into_any_element(),
        ViewNode::Checkbox {
            label,
            checked,
            action_id,
            action_value,
            style,
        } => {
            let plugin_id = plugin_id.to_string();
            let page_id = page_id.map(str::to_string);
            let action_id = action_id.clone();
            let action_value = action_value
                .clone()
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
                .child(styled_text(label, style, &colors).p_0())
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
        ViewNode::Toggle {
            label,
            enabled,
            action_id,
            action_value,
            style,
        } => {
            let plugin_id = plugin_id.to_string();
            let page_id = page_id.map(str::to_string);
            let action_id = action_id.clone();
            let action_value = action_value
                .clone()
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
                .child(styled_text(label, style, &colors).p_0())
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
        ViewNode::Select {
            label,
            action_id,
            options,
            selected,
            style,
        } => {
            let mut element = styled_container(style, &colors)
                .gap(px(6.0))
                .child(styled_text(label, style, &colors).p_0());
            let mut row = div().flex().flex_row().flex_wrap().gap(px(6.0));
            for option in options {
                let plugin_id = plugin_id.to_string();
                let page_id = page_id.map(str::to_string);
                let action_id = action_id.clone();
                let option_value = option.value.clone();
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
        ViewNode::Progress {
            label,
            value,
            total,
            style,
        } => {
            let ratio = total
                .filter(|total| *total > 0)
                .map(|total| (*value as f64 / total as f64).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let width_percent = (ratio * 100.0) as f32;
            styled_container(style, &colors)
                .gap(px(6.0))
                .child(styled_text(label, style, &colors).p_0())
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
        ViewNode::Link {
            label,
            url,
            tooltip: _,
            style,
        } => {
            let plugin_id = plugin_id.to_string();
            let url = url.clone();
            styled_text(label, style, &colors)
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
        ViewNode::List { items, style } => {
            let mut element = styled_container(style, &colors);
            for item in items {
                element = element.child(render_node(item, plugin_id, page_id, window, cx, colors));
            }
            element.into_any_element()
        }
        ViewNode::Separator => div()
            .h(px(1.0))
            .w_full()
            .bg(token_color(ThemeToken::Border, &colors))
            .into_any_element(),
        ViewNode::Badge { label, style } => styled_text(label, style, &colors)
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(8.0))
            .bg(token_color(
                style.background.unwrap_or(ThemeToken::Surface),
                &colors,
            ))
            .into_any_element(),
        ViewNode::Icon { name, style } => {
            let path = icon_path(name);
            let color = token_color(style.color.unwrap_or(ThemeToken::SecondaryText), &colors);
            div()
                .flex()
                .items_center()
                .justify_center()
                .child(themed_icon(path, 18.0, color))
                .into_any_element()
        }
        ViewNode::Image {
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
        } => {
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
                    img(ImageSource::from(src.clone()))
                        .absolute()
                        .inset_0()
                        .size_full()
                        .rounded(radius)
                        .object_fit(object_fit)
                        .decode_to_bounds()
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
        ViewNode::Spacer { size } => div().h(px(f32::from(*size))).into_any_element(),
    }
}

fn styled_container(style: &ViewStyle, colors: &ThemeColors) -> gpui::Div {
    let mut element = div().flex().gap(px(f32::from(style.gap)));
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
    if let Some(background) = style.background {
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

fn styled_text(text: &str, style: &ViewStyle, colors: &ThemeColors) -> gpui::Div {
    let mut element = styled_container(style, colors)
        .text_color(token_color(
            style.color.unwrap_or(ThemeToken::PrimaryText),
            colors,
        ))
        .text_size(text_size(style.text_size))
        .child(SharedString::from(text.to_string()));

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

fn current_theme_colors(cx: &App) -> ThemeColors {
    let theme = cx.global::<ThemeState>();
    lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
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

fn icon_path(name: &str) -> &'static str {
    match name {
        "settings" => lucide_gpui::icons::icon_settings(),
        "alert" => lucide_gpui::icons::icon_circle_alert(),
        "star" => lucide_gpui::icons::icon_star(),
        "plug" => lucide_gpui::icons::icon_plug(),
        _ => lucide_gpui::icons::icon_info(),
    }
}

pub fn fallback_panel(message: impl Into<SharedString>) -> gpui::Div {
    let colors = LightColors::colors();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text_node(text: &str) -> ViewNode {
        ViewNode::Text {
            text: text.to_string(),
            style: ViewStyle::default(),
        }
    }

    #[test]
    fn validates_view_limits() {
        let tree = ViewTree {
            root: ViewNode::Container {
                style: ViewStyle::default(),
                children: vec![text_node("hello")],
            },
        };

        let stats = tree.validate().expect("valid tree should pass");
        assert_eq!(stats.nodes, 2);
        assert_eq!(stats.total_text_bytes, 5);
    }

    #[test]
    fn rejects_too_deep_view_tree() {
        let mut node = text_node("leaf");
        for _ in 0..MAX_VIEW_DEPTH {
            node = ViewNode::Container {
                style: ViewStyle::default(),
                children: vec![node],
            };
        }

        let tree = ViewTree { root: node };
        let error = tree.validate().expect_err("tree should be too deep");
        assert!(error.to_string().contains("max depth"));
    }

    #[test]
    fn rejects_invalid_action_id() {
        let tree = ViewTree {
            root: ViewNode::Button {
                label: "Run".to_string(),
                action_id: "Run Now!".to_string(),
                action_value: None,
                style: ViewStyle::default(),
            },
        };

        let error = tree.validate().expect_err("invalid action id should fail");
        assert!(error.to_string().contains("action id"));
    }
}
