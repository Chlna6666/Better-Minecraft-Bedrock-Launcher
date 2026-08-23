use gpui::{
    AnyView, Context, InteractiveElement as _, IntoElement, ParentElement, Render, Styled,
    Subscription, Window, div,
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

            #[cfg(target_os = "windows")]
            {
                cx.default_global::<crate::ui::onboarding::uwp_safety::UwpSafetyGuideState>();
                subscriptions.push(cx.observe_global::<
                    crate::ui::onboarding::uwp_safety::UwpSafetyGuideState,
                >(|_this, cx| cx.notify()));
                subscriptions.push(cx.observe_global::<
                    crate::ui::views::download::state::DownloadPageState,
                >(|_this, cx| {
                    let uwp_download = cx
                        .global::<crate::ui::views::download::state::DownloadPageState>()
                        .game_dialog
                        .as_ref()
                        .and_then(|dialog| {
                            (!dialog.is_gdk
                                && matches!(
                                    dialog.kind,
                                    crate::ui::views::download::state::GameDialogKind::ConfirmDownload
                                ))
                            .then(|| (dialog.package_id.clone(), dialog.version_type))
                        });

                    if let Some((package_id, version_type)) = uwp_download {
                        crate::ui::onboarding::uwp_safety::request_download(
                            package_id,
                            version_type,
                            cx,
                        );
                    } else {
                        crate::ui::onboarding::uwp_safety::clear_download_context(cx);
                    }
                }));
            }

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
            let (tour_visible, tour_scene) = {
                let tour = cx.global::<crate::ui::onboarding::state::OnboardingTourState>();
                (tour.visible, tour.scene)
            };

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
                && tour_visible
                && main_window_id == Some(current_window_id)
            {
                // 管理页教学使用纯 UI 演示数据，不允许任何点击穿透到背后的真实实例。
                // 演示层和教学面板随后绘制在此透明 hitbox 之上，仍可正常使用“上一步/下一步”。
                if tour_scene == crate::ui::onboarding::state::OnboardingScene::ManageOverview {
                    root = root.child(div().absolute().inset_0().occlude());
                }

                let tour_state = cx.global::<crate::ui::onboarding::state::OnboardingTourState>();
                root = root.child(crate::ui::onboarding::render_onboarding_tour(
                    tour_state, window, cx,
                ));
            }

            #[cfg(target_os = "windows")]
            if !agreement_visible
                && !platform_blocker_visible
                && !tour_visible
                && main_window_id == Some(current_window_id)
                && cx
                    .global::<crate::ui::onboarding::uwp_safety::UwpSafetyGuideState>()
                    .visible
            {
                let guide =
                    cx.global::<crate::ui::onboarding::uwp_safety::UwpSafetyGuideState>();
                root = root.child(
                    crate::ui::onboarding::uwp_safety::render_uwp_safety_guide(
                        guide, window, cx,
                    ),
                );
            }
        }

        root
    }
}
