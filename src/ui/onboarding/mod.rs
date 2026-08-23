pub mod anchor;
mod guided_overlay;
pub mod state;

use gpui::{App, AppContext as _, BorrowAppContext as _, SharedString};

use state::{
    OnboardingPlatformSummary, OnboardingScene, OnboardingSummaryItem, OnboardingTourState,
};

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
}

fn apply_scene_route(scene: OnboardingScene, cx: &mut App) {
    use crate::ui::views::download::state::DownloadTab;

    match scene {
        OnboardingScene::Welcome | OnboardingScene::Finish => {
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Home);
        }
        OnboardingScene::GameDownload => {
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
        OnboardingScene::ManageOverview => {
            crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Manage);
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
            #[cfg(target_os = "windows")]
            {
                crate::ui::navigation::navigate_to(cx, crate::ui::navigation::AppRoute::Manage);
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

#[cfg(target_os = "windows")]
fn build_platform_summary() -> OnboardingPlatformSummary {
    let environment = crate::core::minecraft::uwp_migration::scan_onboarding_environment();
    let release = &environment.release;
    let preview = &environment.preview;
    let protected_data = release.data_present || preview.data_present;

    let registration_text = |summary: &crate::core::minecraft::uwp_migration::MinecraftDataSummary| {
        if !summary.registered {
            return "未注册".to_string();
        }
        let version = summary.registered_version.as_deref().unwrap_or("未知版本");
        if summary.bmcbl_managed_registration {
            format!("BMCBL 散装 DevelopmentMode · {version}")
        } else if summary.development_mode {
            format!("外部 DevelopmentMode · {version}")
        } else {
            format!("Microsoft Store / 外部安装 · {version}")
        }
    };

    let data_text = |summary: &crate::core::minecraft::uwp_migration::MinecraftDataSummary| {
        if summary.data_present {
            format!(
                "{} 个世界 · {} 个资源包 · {}",
                summary.worlds,
                summary.resource_packs,
                human_bytes(summary.total_size)
            )
        } else {
            "未发现 games/com.mojang 用户数据".to_string()
        }
    };

    OnboardingPlatformSummary {
        title: if protected_data {
            "检测到现有 Minecraft UWP 数据".to_string()
        } else {
            "当前没有需要迁移的 Store UWP 数据".to_string()
        },
        detail: if protected_data {
            "BMCBL 在替换 Store/外部 UWP 注册之前会先备份并校验数据；备份失败会直接阻止卸载。".to_string()
        } else {
            "以后如果检测到 Store/外部 UWP 数据，同一套强制备份安全门仍会自动生效。".to_string()
        },
        items: vec![
            OnboardingSummaryItem {
                label: "正式版注册".to_string(),
                value: format!("{} · {}", registration_text(release), data_text(release)),
                warning: release.data_present && !release.bmcbl_managed_registration,
            },
            OnboardingSummaryItem {
                label: "Preview 注册".to_string(),
                value: format!("{} · {}", registration_text(preview), data_text(preview)),
                warning: preview.data_present && !preview.bmcbl_managed_registration,
            },
            OnboardingSummaryItem {
                label: "BMCBL 本地版本".to_string(),
                value: format!("{} 个版本目录", environment.bmcbl_versions),
                warning: false,
            },
        ],
    }
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

fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

pub(crate) fn scene_is(cx: &App, scene: OnboardingScene) -> bool {
    cx.try_global::<OnboardingTourState>()
        .is_some_and(|state| state.visible && state.scene == scene)
}

pub(crate) fn scene_label(scene: OnboardingScene) -> SharedString {
    SharedString::from(match scene {
        OnboardingScene::Welcome => "欢迎",
        OnboardingScene::GameDownload => "游戏下载",
        OnboardingScene::ResourcePackDownload => "CF 资源",
        OnboardingScene::ModDownload => "模组",
        OnboardingScene::ImportPackage => "导入版本",
        OnboardingScene::ManageOverview => "版本管理",
        OnboardingScene::PlatformSetup => {
            #[cfg(target_os = "windows")]
            {
                "UWP 数据保护"
            }
            #[cfg(target_os = "linux")]
            {
                "Proton-GDK"
            }
        }
        OnboardingScene::Finish => "完成",
    })
}