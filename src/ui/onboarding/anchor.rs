use gpui::{BoundsObserver, Styled as _, bounds_observer};

use super::state::{OnboardingAnchor, OnboardingScene, OnboardingTourState};

pub fn observe(anchor: OnboardingAnchor) -> BoundsObserver {
    bounds_observer(move |bounds, _window, cx| {
        let should_update = cx.try_global::<OnboardingTourState>().is_some_and(|state| {
            state.visible
                && anchor_is_active(anchor, state.scene)
                && state.anchor(anchor) != Some(bounds)
        });
        if should_update {
            cx.global_mut::<OnboardingTourState>().set_anchor(anchor, bounds);
        }
    })
    .absolute()
    .inset_0()
}

const fn anchor_is_active(anchor: OnboardingAnchor, scene: OnboardingScene) -> bool {
    matches!(
        (anchor, scene),
        (OnboardingAnchor::DownloadToolbar, OnboardingScene::DownloadOverview)
            | (OnboardingAnchor::DownloadImport, OnboardingScene::ImportPackage)
            | (OnboardingAnchor::VersionSidebar, OnboardingScene::VersionManagement)
    )
}
