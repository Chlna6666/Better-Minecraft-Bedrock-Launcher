use gpui::{
    AnyView, Context, IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
};

pub struct RootView {
    view: AnyView,
    _subscriptions: Vec<Subscription>,
}

impl RootView {
    pub fn new(view: impl Into<AnyView>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let subscriptions = {
            cx.default_global::<crate::ui::onboarding::state::OnboardingTourState>();
            let mut subscriptions = vec![
                cx.observe_global::<crate::ui::onboarding::state::OnboardingTourState>(
                    |_this, cx| cx.notify(),
                ),
                cx.observe_global::<crate::ui::state::agreement::AgreementState>(|_this, cx| {
                    cx.notify();
                }),
                cx.observe_global::<crate::ui::window::debug::DebugState>(|_this, cx| {
                    cx.notify();
                }),
            ];

            #[cfg(target_os = "linux")]
            {
                // 首次导览自己负责解释 Proton-GDK。不要同时叠加旧的 Linux runtime
                // “缺少运行环境”提示；真正处于安装中的进度层仍然保留优先级。
                let tour_visible = cx
                    .global::<crate::ui::onboarding::state::OnboardingTourState>()
                    .visible;
                let should_dismiss_runtime = tour_visible && {
                    let runtime = cx.global::<crate::ui::state::linux_runtime::LinuxRuntimeState>();
                    runtime.visible
                        && runtime.status
                            != crate::ui::state::linux_runtime::LinuxRuntimeStatus::Installing
                };
                if should_dismiss_runtime {
                    cx.update_global(
                        |runtime: &mut crate::ui::state::linux_runtime::LinuxRuntimeState, _cx| {
                            runtime.dismiss();
                        },
                    );
                }

                subscriptions.push(cx.observe_global::<
                    crate::ui::state::linux_runtime::LinuxRuntimeState,
                >(|_this, cx| {
                    let tour_visible = cx
                        .global::<crate::ui::onboarding::state::OnboardingTourState>()
                        .visible;
                    let should_dismiss = tour_visible && {
                        let runtime =
                            cx.global::<crate::ui::state::linux_runtime::LinuxRuntimeState>();
                        runtime.visible
                            && runtime.status
                                != crate::ui::state::linux_runtime::LinuxRuntimeStatus::Installing
                    };
                    if should_dismiss {
                        cx.update_global(
                            |runtime: &mut crate::ui::state::linux_runtime::LinuxRuntimeState,
                             _cx| {
                                runtime.dismiss();
                            },
                        );
                    }
                    cx.notify();
                }));
            }

            subscriptions
        };
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        let subscriptions = Vec::new();

        Self {
            view: view.into(),
            _subscriptions: subscriptions,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().size_full().child(self.view.clone());

        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            let current_window_id = window.window_handle().window_id().as_u64();
            let main_window_id = cx.global::<crate::ui::window::debug::DebugState>().main_window_id;
            let agreement_visible = cx
                .global::<crate::ui::state::agreement::AgreementState>()
                .is_visible();
            let tour_state = cx.global::<crate::ui::onboarding::state::OnboardingTourState>();

            #[cfg(target_os = "windows")]
            let platform_blocker_visible = {
                let prereq = cx.global::<crate::ui::state::launch_prereq::LaunchPrereqState>();
                prereq.visible && !prereq.is_onboarding()
            };
            #[cfg(target_os = "linux")]
            let platform_blocker_visible = {
                let runtime = cx.global::<crate::ui::state::linux_runtime::LinuxRuntimeState>();
                runtime.visible
                    && runtime.status
                        == crate::ui::state::linux_runtime::LinuxRuntimeStatus::Installing
            };

            if !agreement_visible
                && !platform_blocker_visible
                && tour_state.visible
                && main_window_id == Some(current_window_id)
            {
                root = root.child(crate::ui::onboarding::render_onboarding_tour(
                    tour_state, window, cx,
                ));
            }
        }

        root
    }
}
