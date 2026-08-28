use super::*;
use crate::ui::animation::request_animation_frame_if_active;
use crate::ui::state::bedrock_auth::BedrockAuthState;
use crate::ui::state::navigation::NavState;

// Bubble-only fallback: inputs and editors keep their own Tab actions.
pub(super) fn navigate_focus(event: &KeyDownEvent, window: &mut Window, cx: &mut App) {
    let mut modifiers = event.keystroke.modifiers;
    modifiers.shift = false;
    if event.keystroke.key != "tab" || modifiers.modified() {
        return;
    }
    cx.stop_propagation();
    cx.update_global(|state: &mut BedrockAuthState, _| {
        if state.dialog_open {
            state.keyboard_navigation = true;
            state.dialog_motion.snap_to(1.0);
        }
    });
    if event.keystroke.modifiers.shift {
        window.focus_prev();
    } else {
        window.focus_next();
    }
}

pub(super) struct AppChromeView {
    _subscriptions: Vec<Subscription>,
    auth_trigger_focus: FocusHandle,
    auth_panel_focus: FocusHandle,
    reduced_motion: bool,
    auth_trigger_bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    auth_was_open: bool,
    auth_blocked: bool,
}

impl AppChromeView {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![
            cx.observe_global_in::<BedrockAuthState>(window, |this, window, cx| {
                let open = cx.global::<BedrockAuthState>().dialog_open;
                if open != this.auth_was_open {
                    this.auth_was_open = open;
                    if open && !this.auth_blocked {
                        window.focus(&this.auth_panel_focus);
                    } else if !this.auth_blocked
                        && this.auth_panel_focus.contains_focused(window, cx)
                    {
                        window.focus(&this.auth_trigger_focus);
                    }
                }
                cx.notify();
            }),
            cx.observe_global::<gpui_router::RouterState>(|_, cx| cx.notify()),
            cx.observe_global::<NavState>(|_, cx| cx.notify()),
            cx.observe_global::<ThemeState>(|_, cx| cx.notify()),
            cx.observe_global::<I18n>(|_, cx| cx.notify()),
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
        subscriptions.push(cx.observe_window_activation(window, |this, _, cx| {
            this.reduced_motion = crate::core::ui_prefs::reduced_motion();
            cx.notify();
        }));
        let show_labels = window.bounds().size.width >= px(1180.);
        cx.update_global(|state: &mut NavState, _cx| {
            state.set_labels_target_immediate(show_labels);
        });
        let auth_panel_focus = cx.focus_handle();
        let auth_was_open = cx.global::<BedrockAuthState>().dialog_open;
        if auth_was_open {
            window.focus(&auth_panel_focus);
        }
        Self {
            _subscriptions: subscriptions,
            auth_trigger_focus: cx.focus_handle().tab_stop(true),
            auth_panel_focus,
            reduced_motion: crate::core::ui_prefs::reduced_motion(),
            auth_trigger_bounds: Rc::new(std::cell::Cell::new(None)),
            auth_was_open,
            auth_blocked: false,
        }
    }

    pub(super) fn set_auth_blocked(
        &mut self,
        blocked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.auth_blocked == blocked {
            return;
        }
        self.auth_blocked = blocked;
        if blocked
            && (self.auth_panel_focus.contains_focused(window, cx)
                || self.auth_trigger_focus.is_focused(window))
        {
            window.blur();
        } else if !blocked && cx.global::<BedrockAuthState>().dialog_open {
            window.focus(&self.auth_panel_focus);
        }
        cx.notify();
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
            auth: chrome::auth::RenderState::new(
                auth,
                now,
                self.reduced_motion,
                (
                    self.auth_trigger_focus.clone(),
                    self.auth_panel_focus.clone(),
                ),
                self.auth_trigger_bounds.clone(),
            )
            .blocked(self.auth_blocked),
            update_available: cx.global::<UpdateState>().available.is_some(),
            visual_active_index: nav.visual_active_index(),
            pill_left_steps,
            pill_right_steps,
            labels_layout_factor: nav.labels_layout_factor(now),
            labels_opacity_factor: nav.labels_opacity_factor(now),
            nav_animating: nav.is_animating(now),
            glass_effect_enabled: cx
                .global::<crate::ui::views::settings::state::SettingsPageState>()
                .glass_effect_enabled,
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
        request_animation_frame_if_active(window, state.auth.animating);
        let route = crate::ui::navigation::current_route_target(cx);
        let update_modal_open = cx.global::<UpdateState>().show_modal;
        chrome::render_app_chrome(state, route, update_modal_open)
    }
}
