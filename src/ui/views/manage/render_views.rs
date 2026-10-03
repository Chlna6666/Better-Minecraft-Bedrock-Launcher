use super::*;

pub(super) struct ManageTabBodyView {
    parent: WeakEntity<ManagePageView>,
    _subscriptions: Vec<Subscription>,
    active: bool,
    last_signature: ManageTabBodyRenderSignature,
}

impl ManageTabBodyView {
    pub(super) fn new(parent: WeakEntity<ManagePageView>, cx: &mut Context<Self>) -> Self {
        let last_signature =
            ManageTabBodyRenderSignature::from_state(cx.global::<ManagePageState>());
        let subscriptions = vec![
            cx.observe_global::<ManagePageState>(|this, cx| {
                let signature =
                    ManageTabBodyRenderSignature::from_state(cx.global::<ManagePageState>());
                if this.last_signature != signature {
                    this.last_signature = signature;
                    if this.active {
                        cx.notify();
                    }
                }
            }),
            cx.observe_global::<ThemeState>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
            cx.observe_global::<I18n>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(
                |this, cx| {
                    if this.active {
                        cx.notify();
                    }
                },
            ),
        ];

        Self {
            parent,
            _subscriptions: subscriptions,
            active: false,
            last_signature,
        }
    }

    pub(super) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            cx.notify();
        }
    }
}

impl Render for ManageTabBodyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.parent
            .update(cx, |parent, parent_cx| {
                parent.prepare_render(window, parent_cx);
                let now = window.animation_time();
                let theme = parent_cx.global::<ThemeState>();
                let colors = lerp_theme_colors(
                    &LightColors::colors(),
                    &DarkColors::colors(),
                    theme.factor(now),
                    theme.accent,
                );
                let state = parent_cx.global::<ManagePageState>().clone();
                parent.render_tab_body(window, &colors, &state, now, parent_cx)
            })
            .unwrap_or_else(|_| div().size_full().into_any_element())
    }
}
