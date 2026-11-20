pub(crate) mod elements;

use crate::ui::components::page_shell::{page_frame, page_panel, split_page, split_sidebar_panel};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::navigation::{self, RouteTarget};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::plugin::elements::render_validated_view_tree;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

pub struct PluginPageView {
    plugin_id: String,
    page_id: String,
    _subscriptions: Vec<Subscription>,
    window_bounds: Option<Subscription>,
}

impl PluginPageView {
    pub fn new(plugin_id: String, page_id: String, cx: &mut Context<Self>) -> Self {
        crate::plugins::runtime::request_page_render(cx, &plugin_id, &page_id);
        Self {
            plugin_id,
            page_id,
            window_bounds: None,
            _subscriptions: vec![
                cx.observe_global::<crate::plugins::runtime::PluginRegistry>(|_this, cx| {
                    cx.notify();
                }),
                cx.observe_global::<ThemeState>(|_this, cx| {
                    cx.notify();
                }),
            ],
        }
    }
}

impl Render for PluginPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.window_bounds.is_none() {
            self.window_bounds = Some(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));
        }
        let now = window.animation_time();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::theme(),
            &DarkColors::theme(),
            theme.factor(now),
            theme.accent,
        );

        let registry = cx.global::<crate::plugins::runtime::PluginRegistry>();
        let content = if let Some(tree) = registry.cached_page(&self.plugin_id, &self.page_id) {
            render_validated_view_tree(&tree, &self.plugin_id, Some(&self.page_id), window, cx)
        } else if let Some(error) = registry.cached_page_error(&self.plugin_id, &self.page_id) {
            crate::ui::views::plugin::elements::fallback_panel(error.to_string()).into_any_element()
        } else {
            crate::plugins::runtime::request_page_render(cx, &self.plugin_id, &self.page_id);
            crate::ui::views::plugin::elements::loading_panel("正在加载插件页面...")
                .into_any_element()
        };

        let panel = page_panel(&colors).flex_1().min_w(px(0.)).h_full().child(
            div()
                .id("plugin-page-scroll")
                .size_full()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .p(px(14.))
                .child(content),
        );
        let tool = matches!(
            navigation::current_route_target(cx),
            RouteTarget::Tool { .. }
        );
        page_frame(if tool {
            split_page(self.tools_sidebar(&colors, cx), panel)
        } else {
            div().size_full().flex().child(panel)
        })
    }
}

impl PluginPageView {
    fn tools_sidebar(&self, colors: &ThemeColors, cx: &gpui::App) -> impl IntoElement {
        let items = crate::plugins::runtime::tool_pages(cx)
            .into_iter()
            .filter_map(|page| {
                let entry = page.navigation?;
                let active = page.plugin_id == self.plugin_id && page.page_id == self.page_id;
                let target = RouteTarget::Tool {
                    plugin_id: page.plugin_id,
                    page_id: page.page_id,
                };
                let icon = crate::ui::views::plugin::elements::icon_path(
                    entry.icon.as_deref().unwrap_or("plug"),
                );
                Some(tool_tab(
                    colors,
                    target,
                    active,
                    icon,
                    entry.label,
                    page.title,
                ))
            });
        split_sidebar_panel(colors)
            .id("plugin-tools-sidebar")
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(14.))
            .child(crate::ui::onboarding::anchor::observe(
                crate::ui::onboarding::state::OnboardingAnchor::ToolsSidebar,
            ))
            .children(items)
    }
}

// The Tools sidebar gives every registered page the same navigation card.
fn tool_tab(
    colors: &ThemeColors,
    target: RouteTarget,
    selected: bool,
    icon: &'static str,
    label: String,
    title: SharedString,
) -> impl IntoElement {
    let accent = if selected {
        colors.accent
    } else {
        colors.text_secondary
    };
    let background = if selected {
        Hsla {
            a: 0.10,
            ..colors.accent
        }
    } else {
        Hsla {
            a: 0.40,
            ..colors.surface
        }
    };
    let border = if selected {
        Hsla {
            a: 0.24,
            ..colors.accent
        }
    } else {
        Hsla {
            a: 0.12,
            ..colors.border
        }
    };
    let has_description = !title.is_empty() && title.as_ref() != label.as_str();
    div()
        .id(SharedString::from(target.pathname().into_owned()))
        .w_full()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(border)
        .bg(background)
        .p(px(12.))
        .cursor_pointer()
        .active(|style| style.scale(0.97))
        .when(!selected, |item| {
            item.hover(|style| {
                style.bg(Hsla {
                    a: 0.60,
                    ..colors.surface_hover
                })
            })
        })
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .size(px(34.))
                .flex_none()
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.12, ..accent })
                .flex()
                .items_center()
                .justify_center()
                .child(crate::ui::components::icon::themed_icon(icon, 17., accent)),
        )
        .child(
            div()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(label),
                )
                .when(has_description, |item| {
                    item.child(
                        div()
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(if selected {
                                colors.text_secondary
                            } else {
                                colors.text_muted
                            })
                            .child(title),
                    )
                }),
        )
        .on_click(move |_, _, cx| navigation::navigate_target(cx, target.clone()))
}
