use super::*;
use crate::ui::animation::request_animation_frame_if_active;
use crate::ui::state::bedrock_auth::BedrockAuthState;
use crate::ui::state::navigation::NavState;

pub(super) struct AppChromeView {
    _subscriptions: Vec<Subscription>,
}

impl AppChromeView {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![
            cx.observe_global::<BedrockAuthState>(|_, cx| cx.notify()),
            cx.observe_global::<gpui_router::RouterState>(|_, cx| cx.notify()),
            cx.observe_global::<NavState>(|_, cx| cx.notify()),
            cx.observe_global::<ThemeState>(|_, cx| cx.notify()),
            cx.observe_global::<UpdateState>(|_, cx| cx.notify()),
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(|_, cx| {
                cx.notify()
            }),
            cx.observe_global::<crate::plugins::runtime::PluginRegistry>(|_, cx| cx.notify()),
        ];
        subscriptions.push(cx.observe_window_bounds(window, |_, window, cx| {
            let show_labels = window.bounds().size.width >= px(1180.);
            cx.update_global(|state: &mut NavState, _cx| {
                state.set_labels_target(show_labels, Instant::now());
            });
            cx.notify();
        }));
        let show_labels = window.bounds().size.width >= px(1180.);
        cx.update_global(|state: &mut NavState, _cx| {
            state.set_labels_target_immediate(show_labels);
        });
        Self {
            _subscriptions: subscriptions,
        }
    }

    fn prepare_render_state(&self, now: Instant, window: &Window, cx: &App) -> TopbarRenderState {
        let theme = cx.global::<ThemeState>();
        let nav = cx.global::<NavState>();
        let auth = cx.global::<BedrockAuthState>();
        let (pill_left_steps, pill_right_steps) = nav.pill_edges(now);
        TopbarRenderState {
            theme_k: theme.factor(now),
            theme_target_dark: theme.target_dark,
            theme_animating: theme.is_animating(now),
            theme_accent: theme.accent,
            window_width: window.bounds().size.width,
            window_height: window.bounds().size.height,
            auth_snapshot: auth.snapshot.clone(),
            auth_dialog_open: auth.dialog_open,
            auth_pending_delete_account_id: auth.pending_delete_account_id.clone(),
            update_available: cx.global::<UpdateState>().available.is_some(),
            visual_active_index: nav.visual_active_index(),
            pill_left_steps,
            pill_right_steps,
            labels_layout_factor: nav.labels_layout_factor(now),
            labels_opacity_factor: nav.labels_opacity_factor(now),
            nav_animating: nav.is_animating(now),
            // 临时质量隔离测试：标题栏不启用 backdrop blur，只保留原 surface 与底边框。
            // 若底部色带消失，可确认是 blur 边缘采样而不是渐变样式。
            glass_effect_enabled: false,
            plugin_navigation_pages: std::sync::Arc::new(
                crate::plugins::runtime::navigation_pages(cx),
            ),
        }
    }
}

impl Render for AppChromeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let state = self.prepare_render_state(now, window, cx);
        request_animation_frame_if_active(window, state.theme_animating);
        request_animation_frame_if_active(window, state.nav_animating);
        let route = crate::ui::navigation::current_route_target(cx);
        let update_modal_open = cx.global::<UpdateState>().show_modal;
        chrome::render_app_chrome(state, route, update_modal_open)
    }
}
