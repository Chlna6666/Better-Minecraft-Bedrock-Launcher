use gpui::{AnyView, Context, IntoElement, ParentElement, Render, Styled, Window, div};

pub struct RootView {
    view: AnyView,
}

impl RootView {
    pub fn new(view: impl Into<AnyView>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(target_os = "linux")]
        cx.default_global::<crate::ui::state::linux_onboarding::LinuxOnboardingState>();
        Self { view: view.into() }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().size_full().child(self.view.clone());

        #[cfg(target_os = "linux")]
        {
            let current_window_id = window.window_handle().window_id().as_u64();
            let main_window_id = cx.global::<crate::ui::window::debug::DebugState>().main_window_id;
            let agreement_visible = cx
                .global::<crate::ui::state::agreement::AgreementState>()
                .is_visible();
            let onboarding_visible = cx
                .global::<crate::ui::state::linux_onboarding::LinuxOnboardingState>()
                .visible;

            if !agreement_visible
                && onboarding_visible
                && main_window_id == Some(current_window_id)
            {
                root = root.child(
                    crate::ui::overlays::linux_onboarding::render_linux_onboarding_overlay(
                        cx.global::<crate::ui::state::linux_onboarding::LinuxOnboardingState>(),
                        window,
                        cx,
                    ),
                );
            }
        }

        root
    }
}
