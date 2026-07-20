use crate::ui::components::modal;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::SettingsPageState;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::rc::Rc;

use super::icon_btn;

const DEPENDENCIES_MODAL_RADIUS: f32 = 18.0;
const DEPENDENCIES_MODAL_OUTER_PADDING: f32 = 18.0;
const DEPENDENCIES_MODAL_MAX_WIDTH: f32 = 940.0;
const DEPENDENCIES_MODAL_MAX_HEIGHT: f32 = 720.0;
const DEPENDENCIES_MODAL_MIN_WIDTH: f32 = 300.0;
const DEPENDENCIES_MODAL_MIN_HEIGHT: f32 = 300.0;
const CARGO_MANIFEST: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
const DEPENDENCY_METADATA: &[DependencyMetadata] =
    include!(concat!(env!("OUT_DIR"), "/dependency_metadata.rs"));

#[derive(Clone, Debug, Eq, PartialEq)]
struct DependencyGroup {
    name: String,
    items: Vec<DependencyItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DependencyItem {
    name: String,
    details: String,
    version: String,
    license: String,
    source_url: String,
    source_kind: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DependencyMetadata {
    name: &'static str,
    version: &'static str,
    license: &'static str,
    source_url: &'static str,
    source_kind: &'static str,
}

pub(super) fn open_dependencies_modal(cx: &mut App) {
    cx.update_global(|state: &mut SettingsPageState, _cx| {
        state.open_about_dependencies();
    });
}

pub(super) fn render_dependencies_modal(
    colors: &ThemeColors,
    i18n: &I18n,
    settings: &SettingsPageState,
    window_width: Pixels,
    window_height: Pixels,
) -> AnyElement {
    let overlay_background = hsla(0., 0., 0., 0.26);
    let close = Rc::new(|cx: &mut App| {
        cx.update_global(|state: &mut SettingsPageState, _cx| {
            state.close_about_dependencies();
        });
    });
    let (card_width, card_height) = dependency_modal_size(window_width, window_height);
    let scroll_handle = settings.about_dependencies_scroll_handle.clone();

    let groups = parse_dependency_groups_with_metadata(CARGO_MANIFEST, DEPENDENCY_METADATA)
        .unwrap_or_default();
    let dependency_count: usize = groups.iter().map(|group| group.items.len()).sum();
    let dependency_count = SharedString::from(dependency_count.to_string());
    let font_family = SharedString::from(crate::utils::font_settings::DEFAULT_APP_FONT_FAMILY);
    let close_button = close.clone();

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(18.))
        .py(px(14.))
        .rounded_t(px(DEPENDENCIES_MODAL_RADIUS))
        .bg(colors.settings_panel_bg)
        .border_b_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    svg()
                        .path(lucide_icons::icon_package())
                        .w(px(16.))
                        .h(px(16.))
                        .text_color(colors.accent),
                )
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(i18n.t("AboutSection.dependencies.title")),
                ),
        )
        .child(icon_btn(
            colors,
            i18n.t("common.close"),
            lucide_icons::icon_x(),
            true,
            close_button,
        ));

    let scroll_handle_for_event = scroll_handle.clone();
    let scroll_content = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(14.))
        .pb(px(2.))
        .child(render_font_info(colors, i18n, font_family))
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .px(px(2.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(i18n.t("AboutSection.dependencies.manifest")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(i18n.t_args(
                            "AboutSection.dependencies.count",
                            crate::i18n_args![("count", dependency_count.as_ref())],
                        )),
                ),
        )
        .children(
            groups
                .into_iter()
                .map(|group| render_dependency_group(colors, i18n, group).into_any_element()),
        );

    let card = div()
        .id("about-dependencies-modal")
        .relative()
        .w(card_width)
        .max_w(card_width)
        .h(card_height)
        .max_h(card_height)
        .rounded(px(DEPENDENCIES_MODAL_RADIUS))
        .overflow_hidden()
        .border_1()
        .border_color(Hsla {
            a: 0.34,
            ..colors.border
        })
        .bg(colors.settings_panel_bg)
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.30,
                ..rgb(0x000000).into()
            },
            blur_radius: px(40.),
            spread_radius: px(0.),
            offset: point(px(0.), px(16.)),
        }])
        .flex()
        .flex_col()
        .child(
            div()
                .absolute()
                .inset(px(1.))
                .rounded(px(DEPENDENCIES_MODAL_RADIUS - 1.0))
                .border_1()
                .border_color(Hsla {
                    a: 0.16,
                    ..colors.border
                }),
        )
        .child(header)
        .on_scroll_wheel(|_event, _window, cx| cx.stop_propagation())
        .child(
            div().flex_1().min_h(px(0.)).p(px(16.)).child(
                div()
                    .id("about-dependencies-body")
                    .size_full()
                    .overflow_y_scroll()
                    .scrollbar_width(px(0.))
                    .track_scroll(&scroll_handle)
                    .on_scroll_wheel(move |event, window, cx| {
                        let offset = scroll_handle_for_event.offset();
                        let max_offset = scroll_handle_for_event.max_offset();
                        let delta_y = scroll_event_delta_y(event);
                        let at_bottom = offset.y <= -max_offset.height;
                        let at_top = offset.y >= px(0.);

                        if (at_bottom && delta_y < Pixels::ZERO)
                            || (at_top && delta_y > Pixels::ZERO)
                        {
                            scroll_handle_for_event.set_offset(point(
                                offset.x,
                                offset.y.clamp(-max_offset.height, px(0.)),
                            ));
                            window.prevent_default();
                        }
                        cx.stop_propagation();
                    })
                    .child(scroll_content),
            ),
        );

    modal::modal_layer_dismissible(
        div()
            .w_full()
            .h_full()
            .p(px(DEPENDENCIES_MODAL_OUTER_PADDING))
            .flex()
            .items_center()
            .justify_center()
            .child(card),
        overlay_background,
        close,
    )
}

fn dependency_modal_size(window_width: Pixels, window_height: Pixels) -> (Pixels, Pixels) {
    let (width, height) = dependency_modal_size_px(window_width / px(1.), window_height / px(1.));
    (px(width), px(height))
}

fn dependency_modal_size_px(window_width: f32, window_height: f32) -> (f32, f32) {
    (
        modal_axis_size_px(
            window_width,
            DEPENDENCIES_MODAL_MIN_WIDTH,
            DEPENDENCIES_MODAL_MAX_WIDTH,
        ),
        modal_axis_size_px(
            window_height,
            DEPENDENCIES_MODAL_MIN_HEIGHT,
            DEPENDENCIES_MODAL_MAX_HEIGHT,
        ),
    )
}

fn modal_axis_size_px(window_size: f32, minimum: f32, maximum: f32) -> f32 {
    let available = (window_size - DEPENDENCIES_MODAL_OUTER_PADDING * 2.0).max(0.0);

    if available <= minimum {
        available
    } else {
        available.min(maximum)
    }
}

fn scroll_event_delta_y(event: &ScrollWheelEvent) -> Pixels {
    match event.delta {
        ScrollDelta::Pixels(delta) => delta.y,
        ScrollDelta::Lines(delta) => px(delta.y * 20.0),
    }
}

fn render_font_info(colors: &ThemeColors, i18n: &I18n, font_family: SharedString) -> Div {
    div()
        .flex_none()
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.border
        })
        .bg(colors.settings_card_bg)
        .p(px(14.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(i18n.t("AboutSection.dependencies.font_title")),
        )
        .child(info_row(
            colors,
            i18n.t("AboutSection.dependencies.font_family"),
            font_family,
        ))
}

fn info_row(colors: &ThemeColors, label: SharedString, value: SharedString) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .w(px(86.))
                .flex_none()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(colors.text_primary)
                .whitespace_normal()
                .child(value),
        )
}

fn render_dependency_group(colors: &ThemeColors, i18n: &I18n, group: DependencyGroup) -> Div {
    let count = group.items.len();

    div()
        .flex_none()
        .rounded(px(16.))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.border
        })
        .bg(colors.settings_card_bg)
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(14.))
                .py(px(11.))
                .border_b_1()
                .border_color(Hsla {
                    a: 0.16,
                    ..colors.border
                })
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(SharedString::from(group.name)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(SharedString::from(count.to_string())),
                ),
        )
        .children(group.items.into_iter().enumerate().map(|(index, item)| {
            render_dependency_item(colors, i18n, index, item).into_any_element()
        }))
}

fn render_dependency_item(
    colors: &ThemeColors,
    i18n: &I18n,
    index: usize,
    item: DependencyItem,
) -> Div {
    let border_alpha = if index == 0 { 0.0 } else { 0.12 };
    let source_url = item.source_url.clone();
    let source_button = (!source_url.is_empty()).then(|| {
        dependency_source_button(colors, item.name.clone(), source_url).into_any_element()
    });

    div()
        .px(px(14.))
        .py(px(10.))
        .flex_none()
        .border_t_1()
        .border_color(Hsla {
            a: border_alpha,
            ..colors.border
        })
        .flex()
        .items_start()
        .gap(px(14.))
        .child(
            div()
                .w(px(170.))
                .flex_none()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(SharedString::from(item.name)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(7.))
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(colors.text_secondary)
                        .whitespace_normal()
                        .child(SharedString::from(item.details)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(7.))
                        .child(metadata_chip(
                            colors,
                            i18n.t("AboutSection.dependencies.version"),
                            SharedString::from(item.version),
                            i18n,
                        ))
                        .child(metadata_chip(
                            colors,
                            i18n.t("AboutSection.dependencies.license"),
                            SharedString::from(item.license),
                            i18n,
                        ))
                        .child(metadata_chip(
                            colors,
                            i18n.t("AboutSection.dependencies.source"),
                            source_kind_label(i18n, &item.source_kind),
                            i18n,
                        )),
                ),
        )
        .when_some(source_button, |this, button| this.child(button))
}

fn metadata_chip(
    colors: &ThemeColors,
    label: SharedString,
    value: SharedString,
    i18n: &I18n,
) -> Div {
    let value = if value.as_ref().trim().is_empty() {
        i18n.t("AboutSection.dependencies.unknown")
    } else {
        value
    };

    div()
        .max_w(px(260.))
        .rounded(px(9.))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(colors.settings_field_bg)
        .px(px(8.))
        .py(px(4.))
        .flex()
        .items_center()
        .gap(px(5.))
        .child(
            div()
                .text_size(px(10.))
                .text_color(colors.text_secondary)
                .child(label),
        )
        .child(
            div()
                .min_w(px(0.))
                .text_size(px(10.))
                .text_color(colors.text_primary)
                .whitespace_normal()
                .child(value),
        )
}

fn dependency_source_button(
    colors: &ThemeColors,
    dependency_name: String,
    source_url: String,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!(
            "dependency-source-{}",
            dependency_name
        )))
        .w(px(32.))
        .h(px(32.))
        .rounded(px(10.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(Hsla {
            a: 0.22,
            ..colors.border
        })
        .bg(colors.settings_field_bg)
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .child(
            svg()
                .path(lucide_icons::icon_external_link())
                .w(px(15.))
                .h(px(15.))
                .text_color(colors.text_primary),
        )
        .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
            cx.open_url(source_url.as_str());
        })
}

fn source_kind_label(i18n: &I18n, source_kind: &str) -> SharedString {
    match source_kind {
        "registry" => i18n.t("AboutSection.dependencies.source_registry"),
        "git" => i18n.t("AboutSection.dependencies.source_git"),
        "path" => i18n.t("AboutSection.dependencies.source_path"),
        _ => SharedString::from(source_kind.to_string()),
    }
}

fn parse_dependency_groups(manifest: &str) -> Result<Vec<DependencyGroup>, toml::de::Error> {
    parse_dependency_groups_with_metadata(manifest, &[])
}

fn parse_dependency_groups_with_metadata(
    manifest: &str,
    metadata: &[DependencyMetadata],
) -> Result<Vec<DependencyGroup>, toml::de::Error> {
    let value = toml::from_str::<toml::Value>(manifest)?;
    let mut groups = Vec::new();

    collect_dependency_group(
        &value,
        "dependencies",
        "dependencies",
        metadata,
        &mut groups,
    );
    collect_dependency_group(
        &value,
        "build-dependencies",
        "build-dependencies",
        metadata,
        &mut groups,
    );
    collect_dependency_group(
        &value,
        "dev-dependencies",
        "dev-dependencies",
        metadata,
        &mut groups,
    );

    if let Some(targets) = value.get("target").and_then(toml::Value::as_table) {
        let mut target_names = targets.keys().cloned().collect::<Vec<_>>();
        target_names.sort();
        for target_name in target_names {
            let Some(target_value) = targets.get(&target_name) else {
                continue;
            };
            collect_dependency_group(
                target_value,
                "dependencies",
                &format!("target.'{target_name}'.dependencies"),
                metadata,
                &mut groups,
            );
        }
    }

    Ok(groups)
}

fn collect_dependency_group(
    value: &toml::Value,
    key: &str,
    group_name: &str,
    metadata: &[DependencyMetadata],
    groups: &mut Vec<DependencyGroup>,
) {
    let Some(table) = value.get(key).and_then(toml::Value::as_table) else {
        return;
    };

    let mut items = table
        .iter()
        .map(|(name, value)| DependencyItem {
            name: name.to_string(),
            details: dependency_details(value),
            version: dependency_version(value)
                .or_else(|| metadata_field(metadata, name, |item| item.version))
                .unwrap_or_default(),
            license: metadata_field(metadata, name, |item| item.license).unwrap_or_default(),
            source_url: metadata_field(metadata, name, |item| item.source_url)
                .or_else(|| dependency_source_url(name, value))
                .unwrap_or_default(),
            source_kind: metadata_field(metadata, name, |item| item.source_kind)
                .or_else(|| dependency_source_kind(value))
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| item.name.to_ascii_lowercase());

    if !items.is_empty() {
        groups.push(DependencyGroup {
            name: group_name.to_string(),
            items,
        });
    }
}

fn metadata_field(
    metadata: &[DependencyMetadata],
    name: &str,
    field: impl Fn(&DependencyMetadata) -> &'static str,
) -> Option<String> {
    metadata
        .iter()
        .find(|item| item.name.eq_ignore_ascii_case(name))
        .map(field)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn dependency_version(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(version) => Some(version.clone()),
        toml::Value::Table(table) => table
            .get("version")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn dependency_source_url(name: &str, value: &toml::Value) -> Option<String> {
    if let toml::Value::Table(table) = value {
        if let Some(git) = table.get("git").and_then(toml::Value::as_str) {
            return Some(clean_git_url(git));
        }

        if table.get("path").is_some() {
            return None;
        }
    }

    Some(format!("https://crates.io/crates/{name}"))
}

fn dependency_source_kind(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(_) => Some("registry".to_string()),
        toml::Value::Table(table) if table.get("git").is_some() => Some("git".to_string()),
        toml::Value::Table(table) if table.get("path").is_some() => Some("path".to_string()),
        toml::Value::Table(table) if table.get("version").is_some() => Some("registry".to_string()),
        _ => None,
    }
}

fn clean_git_url(url: &str) -> String {
    let without_prefix = url.strip_prefix("git+").unwrap_or(url);
    without_prefix
        .split(['?', '#'])
        .next()
        .unwrap_or(without_prefix)
        .to_string()
}

fn dependency_details(value: &toml::Value) -> String {
    match value {
        toml::Value::String(version) => version.clone(),
        toml::Value::Table(table) => ordered_dependency_table_keys(table)
            .into_iter()
            .map(|(key, value)| format!("{key} = {}", dependency_value(value)))
            .collect::<Vec<_>>()
            .join(", "),
        other => dependency_value(other),
    }
}

fn ordered_dependency_table_keys<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
) -> Vec<(&'a str, &'a toml::Value)> {
    let preferred_order = [
        "version",
        "path",
        "git",
        "branch",
        "tag",
        "rev",
        "features",
        "default-features",
    ];
    let mut entries = Vec::with_capacity(table.len());

    for key in preferred_order {
        if let Some(value) = table.get(key) {
            entries.push((key, value));
        }
    }

    let mut remaining = table
        .iter()
        .filter(|(key, _value)| !preferred_order.contains(&key.as_str()))
        .map(|(key, value)| (key.as_str(), value))
        .collect::<Vec<_>>();
    remaining.sort_by_key(|(key, _value)| key.to_ascii_lowercase());
    entries.extend(remaining);

    entries
}

fn dependency_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => format!("\"{value}\""),
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Float(value) => value.to_string(),
        toml::Value::Boolean(value) => value.to_string(),
        toml::Value::Datetime(value) => value.to_string(),
        toml::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(dependency_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        toml::Value::Table(table) => format!(
            "{{ {} }}",
            ordered_dependency_table_keys(table)
                .into_iter()
                .map(|(key, value)| format!("{key} = {}", dependency_value(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DependencyMetadata, dependency_modal_size_px, parse_dependency_groups,
        parse_dependency_groups_with_metadata,
    };

    #[test]
    fn dependency_modal_size_fits_small_windows() {
        assert_eq!(dependency_modal_size_px(1200.0, 900.0), (940.0, 720.0));
        assert_eq!(dependency_modal_size_px(420.0, 360.0), (384.0, 324.0));
        assert_eq!(dependency_modal_size_px(280.0, 240.0), (244.0, 204.0));
    }

    #[test]
    fn parse_dependency_groups_preserves_inline_table_details() {
        let manifest = r#"
[dependencies]
serde = { version = "1.0", features = ["derive", "rc"] }
gpui = { path = "vendor/gpui" }
simple = "0.1"

[build-dependencies]
chrono = "0.4"

[target.'cfg(windows)'.dependencies]
winreg = "0.56"
"#;

        let groups = parse_dependency_groups(manifest).expect("manifest should parse");

        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].name, "dependencies");
        assert_eq!(groups[0].items[0].name, "gpui");
        assert_eq!(groups[0].items[0].details, "path = \"vendor/gpui\"");
        assert_eq!(groups[0].items[1].name, "serde");
        assert_eq!(
            groups[0].items[1].details,
            "version = \"1.0\", features = [\"derive\", \"rc\"]"
        );
        assert_eq!(groups[0].items[2].name, "simple");
        assert_eq!(groups[0].items[2].details, "0.1");
        assert_eq!(groups[1].name, "build-dependencies");
        assert_eq!(groups[2].name, "target.'cfg(windows)'.dependencies");
    }

    #[test]
    fn parse_dependency_groups_adds_license_and_source_metadata() {
        let manifest = r#"
[dependencies]
serde = { version = "1.0", features = ["derive"] }
gpui = { path = "vendor/gpui" }
"#;
        let metadata = [
            DependencyMetadata {
                name: "serde",
                version: "1.0.219",
                license: "MIT OR Apache-2.0",
                source_url: "https://github.com/serde-rs/serde",
                source_kind: "registry",
            },
            DependencyMetadata {
                name: "gpui",
                version: "0.1.0",
                license: "Apache-2.0",
                source_url: "https://github.com/zed-industries/zed",
                source_kind: "path",
            },
        ];

        let groups = parse_dependency_groups_with_metadata(manifest, &metadata)
            .expect("manifest should parse");

        let gpui = &groups[0].items[0];
        assert_eq!(gpui.name, "gpui");
        assert_eq!(gpui.license, "Apache-2.0");
        assert_eq!(gpui.source_url, "https://github.com/zed-industries/zed");
        assert_eq!(gpui.source_kind, "path");

        let serde = &groups[0].items[1];
        assert_eq!(serde.name, "serde");
        assert_eq!(serde.license, "MIT OR Apache-2.0");
        assert_eq!(serde.source_url, "https://github.com/serde-rs/serde");
        assert_eq!(serde.source_kind, "registry");
    }
}
