use gpui::{BoundsObserver, Styled as _, bounds_observer};

use super::state::{OnboardingAnchor, OnboardingScene, OnboardingTourState};

pub fn observe(anchor: OnboardingAnchor) -> BoundsObserver {
    bounds_observer(move |bounds, _window, cx| {
        let Some(state) = cx.try_global::<OnboardingTourState>() else {
            return;
        };
        if !state.visible || !anchor_is_active(anchor, state.scene) {
            return;
        }

        if state.anchor(anchor) != Some(bounds) {
            cx.global_mut::<OnboardingTourState>()
                .set_anchor(anchor, bounds);
        }
    })
    .absolute()
    .inset_0()
}

const fn anchor_is_active(anchor: OnboardingAnchor, scene: OnboardingScene) -> bool {
    matches!(
        (anchor, scene),
        (
            OnboardingAnchor::DownloadToolbar,
            OnboardingScene::DownloadNavigation
                | OnboardingScene::GameDownload
                | OnboardingScene::ResourcePackDownload
                | OnboardingScene::ModDownload
        ) | (
            OnboardingAnchor::DownloadTabs,
            OnboardingScene::DownloadNavigation
        ) | (
            OnboardingAnchor::DownloadImport,
            OnboardingScene::ImportPackage
        ) | (OnboardingAnchor::TasksPage, OnboardingScene::TasksOverview)
            | (
                OnboardingAnchor::SettingsTabs,
                OnboardingScene::SettingsOverview | OnboardingScene::PlatformSetup
            )
            | (
                OnboardingAnchor::ToolsSidebar,
                OnboardingScene::ToolsOverview
            )
    )
}
