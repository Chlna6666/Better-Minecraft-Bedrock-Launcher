pub mod anchor;
mod guided_overlay;
pub mod state;
#[cfg(target_os = "windows")]
pub mod uwp_safety;

use gpui::{App, AppContext as _, BorrowAppContext as _, SharedString};

use state::{OnboardingScene, OnboardingTourState};
#[cfg(target_os = "linux")]
use state::{OnboardingPlatformSummary, OnboardingSummaryItem};

pub use guided_overlay::render_onboarding_tour;

pub fn reopen(cx: &mut App) {
    cx.update_default_global(|state: &mut OnboardingTourState, _cx| state.reopen());
    apply_scene_route(OnboardingScene::Welcome, cx);
}

pub fn advance(cx: &mut App) {
    let next = cx
        .read_global(|state: &OnboardingTourState, _cx| state.scene.next());
    cx.update_global(|state: &mut OnboardingTourState, _cx| state.set_scene(next));
    apply_scene_route(next, cx);
    #[cfg(target_os = "linux")]
    if next == OnboardingScene::PlatformSetup {
        start_platform_scan(cx);
    }
}

pub fn back(cx: &mut App) {
    let previous = cx
        .read_global(|state: &OnboardingTourState, _cx| state.scene.previous());
    cx.update_global(|state: &mut OnboardingTourState, _cx| {
        state.set_scene(previous);
    });
    apply_scene_route(previous, cx);
    #[cfg(target_os = "linux")]
    if previous == OnboardingScene::PlatformSetup {
        start_platform_scan(cx);
    }
}

pub fn skip(cx: &mut App) {
    complete(cx, None);
}

pub fn finish(cx: &mut App) {
    complete(cx, Some(crate::ui::navigation::AppRoute::Home));
}

pub fn finish_to_download(cx: &mut App) {
    complete(cx, Some(crate::ui::navigation::AppRoute::Download));
}

pub fn finish_to_manage(cx: &mut App) {
    complete(cx, Some(crate::ui::navigation::AppRoute::Manage));
}

#[cfg(target_os = "linux")]
pub fn open_platform_settings(cx: &mut App) {
    cx.update_global(
        |state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
            state.tab = crate::ui::views::settings::state::SettingsTab::ProtonGdk;
        },
    );
    crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Settings);
}

fn complete(cx: &mut App, route: Option<crate::ui::navigation::AppRoute>) {
    if let Err(error) = crate::config::onboarding::complete_current_onboarding() {
        cx.update_global(|state: &mut OnboardingTourState, _cx| {
            state.set_persist_error(format!("保存首次运行引导状态失败：{error}"));
        });
        return;
    }

    cx.update_global(|state: &mut OnboardingTourState, _cx| state.finish());

    #[cfg(target_os = "windows")]
    cx.update_global(
        |state: &mut crate::ui::state::launch_prereq::LaunchPrereqState, _cx| {
            if state.is_onboarding() {
                state.finish_onboarding();
            }
        },
    );

    if let Some(route) = route {
        prepare_route(route, cx);
        crate::ui::navigation::navigate_to(cx, route);
    }

    #[cfg(target_os = "windows")]
    uwp_safety::activate_pending(cx);
}

fn apply_scene_route(scene: OnboardingScene, cx: &mut App) {
    use crate::ui::views::download::state::DownloadTab;

    match scene {
        OnboardingScene::Welcome | OnboardingScene::Finish => {
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Home);
        }
        OnboardingScene::DownloadNavigation | OnboardingScene::GameDownload => {
            prepare_download(DownloadTab::Game, cx);
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Download);
        }
        OnboardingScene::ResourcePackDownload => {
            prepare_download(DownloadTab::ResourcePack, cx);
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Download);
        }
        OnboardingScene::ModDownload => {
            prepare_download(DownloadTab::Mod, cx);
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Download);
        }
        OnboardingScene::ImportPackage => {
            prepare_download(DownloadTab::Game, cx);
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Download);
        }
        OnboardingScene::TasksOverview => {
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Tasks);
        }
        OnboardingScene::ManageOverview | OnboardingScene::ManageContent => {
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Manage);
        }
        OnboardingScene::SettingsOverview => {
            cx.update_global(
                |state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                    state.tab = crate::ui::views::settings::state::SettingsTab::Launcher;
                },
            );
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Settings);
        }
        OnboardingScene::ToolsOverview => {
            cx.update_global(
                |state: &mut crate::ui::views::tools::state::ToolsPageState, _cx| {
                    state.tab = crate::ui::views::tools::state::ToolsTab::Online;
                },
            );
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Tools);
        }
        OnboardingScene::PlatformSetup => {
            #[cfg(target_os = "linux")]
            {
                cx.update_global(
                    |state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                        state.tab = crate::ui::views::settings::state::SettingsTab::ProtonGdk;
                    },
                );
                crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Settings);
            }
            #[cfg(not(target_os = "linux"))]
            {
                crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Home);
            }
        }
    }
}

fn prepare_route(route: crate::ui::navigation::AppRoute, cx: &mut App) {
    if route == crate::ui::navigation::AppRoute::Download {
        prepare_download(crate::ui::views::download::state::DownloadTab::Game, cx);
    }
}

fn prepare_download(tab: crate::ui::views::download::state::DownloadTab, cx: &mut App) {
    cx.update_global(
        |state: &mut crate::ui::views::download::state::DownloadPageState, _cx| {
            if state.tab != tab {
                state.tab_anim_from = state.tab;
                state.tab_anim_at = Some(std::time::Instant::now());
            }
            state.tab = tab;
            state.search_query = SharedString::from("");
            match tab {
                crate::ui::views::download::state::DownloadTab::Game => {
                    state.page_index = 0;
                    state.game_rows_scroll
                        .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                }
                crate::ui::views::download::state::DownloadTab::ResourcePack => {
                    state.curseforge_page_index = 0;
                    state.curseforge_results_scroll
                        .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                    state.curseforge_sidebar_scroll
                        .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                }
                crate::ui::views::download::state::DownloadTab::Mod => {
                    state.levilauncher_page_index = 0;
                    state
                        .levilauncher_scroll
                        .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
                }
            }
        },
    );
}

#[cfg(target_os = "linux")]
fn start_platform_scan(cx: &mut App) {
    let request_id = cx.update_global(|state: &mut OnboardingTourState, _cx| {
        state.begin_platform_scan()
    });

    cx.spawn(async move |cx| {
        let result = crate::tasks::runtime::run_io_blocking(build_platform_summary).await;
        cx.update(|cx| match result {
            Ok(summary) => {
                cx.update_global(|state: &mut OnboardingTourState, _cx| {
                    state.apply_platform_summary(request_id, summary);
                });
            }
            Err(error) => {
                cx.update_global(|state: &mut OnboardingTourState, _cx| {
                    state.set_error(request_id, format!("平台环境检测失败：{error}"));
                });
            }
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

#[cfg(target_os = "linux")]
fn build_platform_summary() -> OnboardingPlatformSummary {
    let check = crate::core::linux_runtime::check_linux_runtime();
    let versions = crate::utils::file_ops::bmcbl_subdir("versions");
    let local_versions = std::fs::read_dir(versions)
        .map(|entries| entries.flatten().filter(|entry| entry.path().is_dir()).count())
        .unwrap_or(0);
    let runner = check.runner.as_ref().map_or_else(
        || "未检测到可用 Proton/UMU runner".to_string(),
        |runner| {
            format!(
                "{} · {}",
                runner.kind.display_name(),
                runner.executable.display()
            )
        },
    );
    let ready = check.is_ready();

    OnboardingPlatformSummary {
        title: if ready {
            "Linux 兼容运行环境已就绪".to_string()
        } else {
            "还需要配置 Proton-GDK / UMU".to_string()
        },
        detail: if ready {
            "BMCBL 会直接使用 Linux 兼容运行环境启动 Bedrock，不执行 Windows UWP 注册或 Store 数据迁移。".to_string()
        } else {
            check
                .missing_reason
                .as_deref()
                .unwrap_or("请前往 Proton-GDK 设置页安装或选择兼容运行环境。")
                .to_string()
        },
        items: vec![
            OnboardingSummaryItem {
                label: "Linux 发行版".to_string(),
                value: check.distribution_name.to_string(),
                warning: false,
            },
            OnboardingSummaryItem {
                label: "Proton-GDK / UMU".to_string(),
                value: runner,
                warning: !ready,
            },
            OnboardingSummaryItem {
                label: "BMCBL 本地版本".to_string(),
                value: format!("{local_versions} 个版本目录"),
                warning: false,
            },
        ],
    }
}

pub(crate) fn scene_is(cx: &App, scene: OnboardingScene) -> bool {
    cx.try_global::<OnboardingTourState>()
        .is_some_and(|state| state.visible && state.scene == scene)
}

pub(crate) fn scene_label(scene: OnboardingScene) -> SharedString {
    SharedString::from(match scene {
        OnboardingScene::Welcome => "欢迎",
        OnboardingScene::DownloadNavigation => "下载页",
        OnboardingScene::GameDownload => "游戏下载",
        OnboardingScene::ResourcePackDownload => "CF 资源",
        OnboardingScene::ModDownload => "模组",
        OnboardingScene::ImportPackage => "导入",
        OnboardingScene::TasksOverview => "任务",
        OnboardingScene::ManageOverview => "版本管理",
        OnboardingScene::ManageContent => "实例内容",
        OnboardingScene::SettingsOverview => "设置",
        OnboardingScene::ToolsOverview => "工具",
        OnboardingScene::PlatformSetup => {
            #[cfg(target_os = "linux")]
            {
                "Proton-GDK"
            }
            #[cfg(not(target_os = "linux"))]
            {
                "完成"
            }
        }
        OnboardingScene::Finish => "完成",
    })
}
