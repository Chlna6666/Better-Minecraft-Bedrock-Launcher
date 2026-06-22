use crate::plugins::ui_dsl::render_validated_view_tree;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, lerp_theme_colors};
use gpui::{Context, IntoElement, ParentElement, Render, Styled, Subscription, Window, div, px};

pub struct PluginPageView {
    plugin_id: String,
    page_id: String,
    _subscriptions: Vec<Subscription>,
}

impl PluginPageView {
    pub fn new(plugin_id: String, page_id: String, cx: &mut Context<Self>) -> Self {
        Self {
            plugin_id,
            page_id,
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
        let now = std::time::Instant::now();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );

        let content = match crate::plugins::runtime::render_page(cx, &self.plugin_id, &self.page_id)
        {
            Ok(tree) => {
                render_validated_view_tree(&tree, &self.plugin_id, Some(&self.page_id), window, cx)
            }
            Err(error) => {
                crate::plugins::ui_dsl::fallback_panel(error.to_string()).into_any_element()
            }
        };

        div().size_full().p(px(24.0)).bg(colors.bg).child(content)
    }
}
