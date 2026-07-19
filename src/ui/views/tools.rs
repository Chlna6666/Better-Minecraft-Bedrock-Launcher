use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::settings::state::SettingsPageState;
use crate::ui::views::tools::state::{ToolsPageState, ToolsTab};
use gpui::*;
use std::time::Duration;

use crate::ui::views::tools::online::actions;

pub(crate) mod online;
mod sidebar;
pub mod state;

pub struct ToolsPageView {
    _subscriptions: Vec<Subscription>,
    _online_refresh_task: Task<()>,
}

impl ToolsPageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global::<ToolsPageState>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global::<ThemeState>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global::<SettingsPageState>(|_, cx| {
                cx.notify();
            }),
        ];
        let online_refresh_task = cx.spawn(async move |_this, cx| {
            loop {
                Timer::after(Duration::from_secs(3)).await;
                if let Err(error) = cx.update(|cx| {
                    actions::refresh_status(cx);
                    actions::check_nat(cx);
                }) {
                    tracing::warn!("online refresh task update failed: {error:?}");
                }
            }
        });
        Self {
            _subscriptions: subscriptions,
            _online_refresh_task: online_refresh_task,
        }
    }
}

impl Render for ToolsPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = std::time::Instant::now();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let window_size = window.bounds().size;
        render_tools_page(colors, window_size.width, cx.global::<ToolsPageState>())
    }
}

pub fn render_tools_page(
    colors: ThemeColors,
    window_width: Pixels,
    state: &ToolsPageState,
) -> impl IntoElement {
    let sidebar = sidebar::render_sidebar(&colors, state.tab);
    let content: AnyElement = match state.tab {
        ToolsTab::Online => {
            online::render_online_panel(&colors, state, window_width).into_any_element()
        }
    };

    crate::ui::components::page_shell::page_frame(crate::ui::components::page_shell::split_page(
        sidebar, content,
    ))
}

pub fn render_tools_overlay(
    colors: &ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    state: &ToolsPageState,
) -> Option<AnyElement> {
    match state.tab {
        ToolsTab::Online => {
            online::render_online_overlay(colors, window_width, window_height, state)
        }
    }
}
