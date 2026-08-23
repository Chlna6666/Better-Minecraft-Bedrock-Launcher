use gpui::{Bounds, BoundsObserver, Styled as _, bounds_observer, point, px, size};

use super::state::{OnboardingAnchor, OnboardingScene, OnboardingTourState};

pub fn observe(anchor: OnboardingAnchor) -> BoundsObserver {
    bounds_observer(move |bounds, _window, cx| {
        let Some(state) = cx.try_global::<OnboardingTourState>() else {
            return;
        };
        if !state.visible || !anchor_is_active(anchor, state.scene) {
            return;
        }

        let scene = state.scene;
        let anchor_changed = state.anchor(anchor) != Some(bounds);
        let download_tabs = if anchor == OnboardingAnchor::DownloadToolbar
            && scene == OnboardingScene::DownloadNavigation
        {
            // Download toolbar 当前真实布局：20px 左内边距、14px 上内边距，
            // 三个 104px 标签加两侧 3px 容器 padding。基于 toolbar 的真实窗口
            // Bounds 派生，避免再依赖窗口宽度/DPI 的魔法坐标。
            Some(Bounds::new(
                point(bounds.origin.x + px(20.0), bounds.origin.y + px(14.0)),
                size(px(318.0), px(38.0)),
            ))
        } else {
            None
        };
        let tabs_changed = download_tabs.is_some_and(|tabs| {
            state.anchor(OnboardingAnchor::DownloadTabs) != Some(tabs)
        });

        if anchor_changed || tabs_changed {
            let state = cx.global_mut::<OnboardingTourState>();
            if anchor_changed {
                state.set_anchor(anchor, bounds);
            }
            if let Some(tabs) = download_tabs {
                if tabs_changed {
                    state.set_anchor(OnboardingAnchor::DownloadTabs, tabs);
                }
            }
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
        ) | (OnboardingAnchor::DownloadImport, OnboardingScene::ImportPackage)
            | (OnboardingAnchor::TasksPage, OnboardingScene::TasksOverview)
            | (
                OnboardingAnchor::SettingsTabs,
                OnboardingScene::SettingsOverview | OnboardingScene::PlatformSetup
            )
            | (OnboardingAnchor::ToolsSidebar, OnboardingScene::ToolsOverview)
    )
}
