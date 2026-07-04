use crate::plugins::ui_dsl::render_validated_view_tree;
use crate::ui::state::theme::ThemeState;
use anyhow::{Result, anyhow};
use gpui::{
    App, AppContext, Context, IntoElement, Render, SharedString, Subscription, Window,
    WindowBounds, WindowOptions, px, size,
};

pub struct PluginWindowView {
    plugin_id: String,
    page_id: String,
    title: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl PluginWindowView {
    pub fn new(plugin_id: String, page_id: String, title: String, cx: &mut Context<Self>) -> Self {
        Self {
            plugin_id,
            page_id,
            title: SharedString::from(title),
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

impl Render for PluginWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match crate::plugins::runtime::render_page(cx, &self.plugin_id, &self.page_id) {
            Ok(tree) => {
                render_validated_view_tree(&tree, &self.plugin_id, Some(&self.page_id), window, cx)
                    .into_any_element()
            }
            Err(error) => {
                crate::plugins::ui_dsl::fallback_panel(error.to_string()).into_any_element()
            }
        }
    }
}

pub fn open_plugin_window(
    cx: &mut App,
    plugin_id: String,
    page_id: String,
    title: String,
) -> Result<u64> {
    let can_open = cx
        .global::<crate::plugins::runtime::PluginRegistry>()
        .page(&plugin_id, &page_id)
        .is_some();
    if !can_open {
        return Err(anyhow!("unknown plugin page {plugin_id}/{page_id}"));
    }

    let options = plugin_window_options(cx);
    let window_title = title.clone();
    let handle = cx.open_window(options, move |window, cx| {
        window.set_title(&window_title);
        window.activate_window();
        let view = cx.new(|cx| PluginWindowView::new(plugin_id, page_id, title, cx));
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    })?;

    Ok(handle.window_id().as_u64())
}

fn plugin_window_options(cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    let fixed_size = size(px(780.0), px(520.0));
    options.window_bounds = Some(WindowBounds::centered(fixed_size, cx));
    options.window_min_size = Some(size(px(520.0), px(360.0)));
    options.is_resizable = true;
    options.is_minimizable = true;
    options.is_movable = true;
    options
}
