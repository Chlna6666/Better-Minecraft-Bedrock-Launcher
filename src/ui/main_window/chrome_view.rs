use super::*;
use crate::ui::state::bedrock_auth::BedrockAuthState;
use crate::ui::state::navigation::NavState;
use crate::ui::theme::{ThemeColors, dark_colors, lerp_theme_colors, light_colors};

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
        window.focus_prev(cx);
    } else {
        window.focus_next(cx);
    }
}

fn colors(window: &Window, cx: &App) -> (ThemeColors, bool) {
    let theme = cx.global::<ThemeState>();
    let now = window.animation_time();
    (
        lerp_theme_colors(
            light_colors(),
            dark_colors(),
            theme.factor(now),
            theme.accent,
        ),
        theme.is_animating(now),
    )
}

pub(super) struct AppChromeView {
    _subscriptions: Vec<Subscription>,
    brand: Entity<BrandChromeView>,
    nav: Entity<NavChromeView>,
    auth: Entity<AuthChromeView>,
    controls: Entity<WindowControlsView>,
    glass_effect_enabled: bool,
}

impl AppChromeView {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let brand = cx.new(BrandChromeView::new);
        let nav = cx.new(|cx| NavChromeView::new(window, cx));
        let auth = cx.new(|cx| AuthChromeView::new(window, cx));
        let controls = cx.new(WindowControlsView::new);
        let glass_effect_enabled = cx
            .global::<crate::ui::views::settings::state::SettingsPageState>()
            .glass_effect_enabled;
        let subscriptions = vec![
            cx.observe_global::<ThemeState>(|_, cx| cx.notify()),
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(
                |this, cx| {
                    let enabled = cx
                        .global::<crate::ui::views::settings::state::SettingsPageState>()
                        .glass_effect_enabled;
                    if this.glass_effect_enabled != enabled {
                        this.glass_effect_enabled = enabled;
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            _subscriptions: subscriptions,
            brand,
            nav,
            auth,
            controls,
            glass_effect_enabled,
        }
    }

    pub(super) fn set_auth_blocked(
        &mut self,
        blocked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.auth
            .update(cx, |auth, cx| auth.set_blocked(blocked, window, cx));
    }
}

impl Render for AppChromeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (colors, theme_animating) = colors(window, cx);
        if theme_animating {
            window.request_animation_frame();
        }
        chrome::render_shell(
            &colors,
            self.glass_effect_enabled,
            AnyView::from(self.brand.clone())
                .cached_by(
                    StyleRefinement::default()
                        .w(px(300.))
                        .h(px(60.))
                        .flex_none(),
                    &"chrome-brand",
                )
                .into_any_element(),
            AnyView::from(self.controls.clone())
                .cached_by(
                    StyleRefinement::default()
                        .w(px(124.))
                        .h(px(60.))
                        .flex_none(),
                    &"chrome-controls",
                )
                .into_any_element(),
            AnyView::from(self.nav.clone())
                .cached_by(StyleRefinement::default().size_full(), &"chrome-nav")
                .into_any_element(),
            AnyView::from(self.auth.clone())
                .cached_absolute_by(&"chrome-auth")
                .into_any_element(),
        )
    }
}

struct BrandChromeView {
    _subscriptions: Vec<Subscription>,
    update_available: bool,
    update_modal_open: bool,
}

impl BrandChromeView {
    fn new(cx: &mut Context<Self>) -> Self {
        let update = cx.global::<UpdateState>();
        let (update_available, update_modal_open) = (update.available.is_some(), update.show_modal);
        Self {
            _subscriptions: vec![
                cx.observe_global::<ThemeState>(|_, cx| cx.notify()),
                cx.observe_global::<I18n>(|_, cx| cx.notify()),
                cx.observe_global::<UpdateState>(|this, cx| {
                    let update = cx.global::<UpdateState>();
                    let available = update.available.is_some();
                    let modal_open = update.show_modal;
                    if this.update_available != available || this.update_modal_open != modal_open {
                        this.update_available = available;
                        this.update_modal_open = modal_open;
                        cx.notify();
                    }
                }),
            ],
            update_available,
            update_modal_open,
        }
    }
}

impl Render for BrandChromeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (colors, animating) = colors(window, cx);
        if animating {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .child(chrome::render_brand(
                &colors,
                self.update_available,
                self.update_modal_open,
            ))
    }
}

struct NavChromeView {
    _subscriptions: Vec<Subscription>,
    items: Vec<chrome::NavItem>,
    revision: (u64, u64),
    signature: (usize, usize, usize, bool),
}

impl NavChromeView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let revision = (
            cx.global::<crate::plugins::runtime::PluginRegistry>()
                .navigation_revision(),
            cx.global::<I18n>().revision(),
        );
        let items = chrome::navigation_items(cx);
        let signature = Self::nav_signature(cx);
        let subscriptions = vec![
            cx.observe_global::<NavState>(|this, cx| {
                let signature = Self::nav_signature(cx);
                if this.signature != signature {
                    this.signature = signature;
                    cx.notify();
                }
            }),
            cx.observe_global::<ThemeState>(|_, cx| cx.notify()),
            cx.observe_global::<I18n>(|this, cx| {
                let revision = cx.global::<I18n>().revision();
                if this.revision.1 != revision {
                    this.revision.1 = revision;
                    this.items = chrome::navigation_items(cx);
                    cx.notify();
                }
            }),
            cx.observe_global::<crate::plugins::runtime::PluginRegistry>(|this, cx| {
                let revision = cx
                    .global::<crate::plugins::runtime::PluginRegistry>()
                    .navigation_revision();
                if this.revision.0 != revision {
                    this.revision.0 = revision;
                    this.items = chrome::navigation_items(cx);
                    cx.notify();
                }
            }),
            cx.observe_window_bounds(window, |_, window, cx| {
                let show_labels = window.bounds().size.width >= px(1180.);
                cx.update_global(|state: &mut NavState, _| {
                    state.set_labels_target(show_labels, Instant::now());
                });
                cx.notify();
            }),
        ];
        let show_labels = window.bounds().size.width >= px(1180.);
        cx.update_global(|state: &mut NavState, _| {
            state.set_labels_target_immediate(show_labels);
        });
        Self {
            _subscriptions: subscriptions,
            items,
            revision,
            signature,
        }
    }

    fn nav_signature(cx: &App) -> (usize, usize, usize, bool) {
        let nav = cx.global::<NavState>();
        (
            nav.visual_active_index(),
            nav.pill_from_index,
            nav.pill_to_index,
            nav.labels_target_visible,
        )
    }
}

impl Render for NavChromeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        let (colors, theme_animating) = colors(window, cx);
        if theme_animating {
            window.request_animation_frame();
        }
        let nav = cx.global::<NavState>();
        let (pill_left_steps, pill_right_steps) = nav.pill_edges(now);
        let state = chrome::NavRenderState {
            window_width: window.bounds().size.width,
            visual_active_index: nav.visual_active_index(),
            pill_left_steps,
            pill_right_steps,
            labels_layout_factor: nav.labels_layout_factor(now),
            labels_opacity_factor: nav.labels_opacity_factor(now),
            nav_animating: nav.is_animating(now),
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(chrome::render_nav(state, &self.items, &colors))
    }
}

struct AuthChromeView {
    _subscriptions: Vec<Subscription>,
    trigger_focus: FocusHandle,
    panel_focus: FocusHandle,
    trigger_bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    reduced_motion: bool,
    was_open: bool,
    blocked: bool,
    glass_effect_enabled: bool,
}

impl AuthChromeView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let trigger_focus = cx.focus_handle().tab_stop(true);
        let panel_focus = cx.focus_handle();
        let was_open = cx.global::<BedrockAuthState>().dialog_open;
        if was_open {
            window.focus(&panel_focus, cx);
        }
        let glass_effect_enabled = cx
            .global::<crate::ui::views::settings::state::SettingsPageState>()
            .glass_effect_enabled;
        let subscriptions = vec![
            cx.observe_global_in::<BedrockAuthState>(window, |this, window, cx| {
                let open = cx.global::<BedrockAuthState>().dialog_open;
                if open != this.was_open {
                    this.was_open = open;
                    if open && !this.blocked {
                        window.focus(&this.panel_focus, cx);
                    } else if !this.blocked && this.panel_focus.contains_focused(window, cx) {
                        window.focus(&this.trigger_focus, cx);
                    }
                }
                cx.notify();
            }),
            cx.observe_global::<ThemeState>(|_, cx| cx.notify()),
            cx.observe_global::<I18n>(|_, cx| cx.notify()),
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(
                |this, cx| {
                    let enabled = cx
                        .global::<crate::ui::views::settings::state::SettingsPageState>()
                        .glass_effect_enabled;
                    if this.glass_effect_enabled != enabled {
                        this.glass_effect_enabled = enabled;
                        cx.notify();
                    }
                },
            ),
            cx.observe_window_bounds(window, |_, _, cx| cx.notify()),
            cx.observe_window_activation(window, |this, _, cx| {
                this.reduced_motion = crate::core::ui_prefs::reduced_motion();
                cx.notify();
            }),
        ];
        Self {
            _subscriptions: subscriptions,
            trigger_focus,
            panel_focus,
            trigger_bounds: Rc::new(std::cell::Cell::new(None)),
            reduced_motion: crate::core::ui_prefs::reduced_motion(),
            was_open,
            blocked: false,
            glass_effect_enabled,
        }
    }

    fn set_blocked(&mut self, blocked: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked == blocked {
            return;
        }
        self.blocked = blocked;

        if blocked && cx.global::<BedrockAuthState>().dialog_open {
            // A modal should close the auth popover, not blur the entire GPUI window. Keeping
            // window focus intact avoids stale hover/focus/material state on the account chip.
            cx.update_global(|state: &mut BedrockAuthState, _| state.close_dialog());
            self.was_open = false;
        }

        cx.notify();
    }
}

impl Render for AuthChromeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (colors, theme_animating) = colors(window, cx);
        let state = chrome::auth::RenderState::new(
            cx.global::<BedrockAuthState>(),
            window.animation_time(),
            self.reduced_motion,
            (self.trigger_focus.clone(), self.panel_focus.clone()),
            self.trigger_bounds.clone(),
        )
        .blocked(self.blocked);
        if theme_animating || state.animating {
            window.request_animation_frame();
        }
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .absolute()
                    .top(px(11.))
                    .right(px(147.))
                    .w(px(180.))
                    .h(px(38.))
                    .flex()
                    .justify_end()
                    .child(chrome::auth::trigger(&state, &colors)),
            )
            .when(state.visible(), |root| {
                root.child(chrome::auth::panel(
                    &state,
                    &colors,
                    window.bounds().size,
                    self.glass_effect_enabled,
                ))
            })
    }
}

struct WindowControlsView {
    _subscriptions: Vec<Subscription>,
}

impl WindowControlsView {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            _subscriptions: vec![cx.observe_global::<ThemeState>(|_, cx| cx.notify())],
        }
    }
}

impl Render for WindowControlsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (colors, animating) = colors(window, cx);
        if animating {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_end()
            .child(chrome::render_controls(
                cx.global::<ThemeState>().target_dark,
                &colors,
            ))
    }
}
