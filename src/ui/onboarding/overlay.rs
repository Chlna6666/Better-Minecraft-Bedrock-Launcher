use gpui::*;
use lucide_gpui::icons as lucide_icons;

use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::onboarding::state::{OnboardingScene, OnboardingTourState};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

pub fn render_onboarding_tour(
    state: &OnboardingTourState,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    let theme = cx.global::<ThemeState>();
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(std::time::Instant::now()),
        theme.accent,
    );
    let size = window.bounds().size;
    let compact = size.width < px(860.) || size.height < px(560.);
    let panel_on_left = matches!(state.scene, OnboardingScene::ImportPackage)
        || (cfg!(target_os = "linux") && state.scene == OnboardingScene::PlatformSetup);

    let panel = render_guide_panel(state, &colors, compact);
    let panel_layer = if compact {
        div()
            .absolute()
            .left(px(14.))
            .right(px(14.))
            .bottom(px(14.))
            .h((size.height - px(110.)).min(px(430.)).max(px(300.)))
            .child(panel)
    } else if panel_on_left {
        div()
            .absolute()
            .left(px(22.))
            .top(px(82.))
            .bottom(px(22.))
            .w(px(398.))
            .child(panel)
    } else {
        div()
            .absolute()
            .right(px(22.))
            .top(px(82.))
            .bottom(px(22.))
            .w(px(398.))
            .child(panel)
    };

    let mut root = div()
        .absolute()
        .inset_0()
        .child(div().absolute().inset_0().bg(Hsla {
            a: if compact { 0.10 } else { 0.16 },
            ..black()
        }));

    if let Some(spotlight) = render_spotlight(state.scene, size, &colors, compact) {
        root = root.child(spotlight);
    }

    root.child(panel_layer).into_any_element()
}

fn render_guide_panel(
    state: &OnboardingTourState,
    colors: &ThemeColors,
    compact: bool,
) -> Div {
    div()
        .size_full()
        .min_h(px(0.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: 0.58,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.985,
            ..colors.bg
        })
        .shadow_lg()
        .occlude()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(render_header(state, colors))
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .px(px(if compact { 18. } else { 22. }))
                .py(px(18.))
                .child(render_scene_body(state, colors)),
        )
        .child(render_footer(state.scene, state, colors))
}

fn render_header(state: &OnboardingTourState, colors: &ThemeColors) -> Div {
    let (icon, title, subtitle) = scene_header(state.scene);
    div()
        .px(px(22.))
        .pt(px(20.))
        .pb(px(16.))
        .border_b_1()
        .border_color(Hsla {
            a: 0.35,
            ..colors.border
        })
        .flex()
        .items_start()
        .gap(px(12.))
        .child(
            div()
                .flex_none()
                .w(px(42.))
                .h(px(42.))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.15,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(20.)).text_color(colors.accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .w_full()
                        .text_size(px(17.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(colors.text_secondary)
                        .child(subtitle),
                ),
        )
        .child(
            div()
                .flex_none()
                .px(px(9.))
                .py(px(5.))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.accent
                })
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.accent)
                .child(format!(
                    "{} / {}",
                    state.scene.index(),
                    OnboardingScene::COUNT
                )),
        )
}

fn render_scene_body(state: &OnboardingTourState, colors: &ThemeColors) -> AnyElement {
    match state.scene {
        OnboardingScene::Welcome => render_welcome(colors),
        OnboardingScene::DownloadOverview => render_download_overview(colors),
        OnboardingScene::ImportPackage => render_import_package(colors),
        OnboardingScene::VersionManagement => render_version_management(colors),
        OnboardingScene::PlatformSetup => render_platform_setup(state, colors),
        OnboardingScene::Finish => render_finish(colors),
    }
}

fn render_welcome(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(intro_text(
            colors,
            "这不是一组需要背下来的说明。接下来 BMCBL 会自动切换到真实页面，告诉你每个常用入口在哪里、什么时候使用。",
        ))
        .child(lesson_card(
            colors,
            lucide_icons::icon_download(),
            "先学会下载游戏",
            "带你进入下载页，认识正式版、Preview、搜索和版本列表。",
        ))
        .child(lesson_card(
            colors,
            lucide_icons::icon_upload(),
            "已有安装包也能导入",
            "告诉你 APPX、ZIP、MSIXVC 应该从哪里导入，不需要手动复制版本文件。",
        ))
        .child(lesson_card(
            colors,
            lucide_icons::icon_settings_2(),
            "最后认识版本管理",
            "下载完成后在哪里启动、管理版本和处理平台相关运行环境。",
        ))
        .child(tip_box(
            colors,
            "导览期间底下是真实的 BMCBL 页面，你可以直接点击和操作；不想操作也可以只按“下一步”。",
        ))
        .into_any_element()
}

fn render_download_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(route_badge(colors, "当前页面：下载 → 游戏"))
        .child(intro_text(
            colors,
            "这里负责获取 Minecraft Bedrock 游戏版本。第一次使用时，先看顶部筛选，再从下面的版本列表选择你需要的版本。",
        ))
        .child(numbered_step(
            colors,
            1,
            "选择版本通道",
            "“正式”适合日常游玩；Preview/测试版用于体验新功能。不了解时保持“全部”即可。",
        ))
        .child(numbered_step(
            colors,
            2,
            "找到目标版本",
            "可以直接滚动列表，也可以用顶部搜索框输入版本号。",
        ))
        .child(numbered_step(
            colors,
            3,
            "点击版本的下载/安装操作",
            "BMCBL 会创建任务并完成下载、校验和解包；完成后版本会进入本地版本列表。",
        ))
        .child(tip_box(
            colors,
            "不知道选哪个版本时，优先选择最新正式版。不要为了“版本号更大”盲目选择 Preview。",
        ))
        .into_any_element()
}

fn render_import_package(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(route_badge(colors, "当前页面：下载 → 右上角上传按钮"))
        .child(intro_text(
            colors,
            "如果你已经有 Minecraft 安装包，不需要重新下载。点击下载页右上角的上传图标即可选择本地文件。",
        ))
        .child(format_card(colors, "APPX", "常见的 Minecraft UWP 安装包。"))
        .child(format_card(
            colors,
            "ZIP",
            "BMCBL 支持的 ZIP 游戏版本包，会按版本导入链路解包。",
        ))
        .child(format_card(
            colors,
            "MSIXVC",
            "部分 GDK 版本使用的安装包格式，由 BMCBL 进入对应解包流程。",
        ))
        .child(numbered_step(
            colors,
            1,
            "点击上传图标",
            "文件选择器只会显示受支持的游戏安装包格式。",
        ))
        .child(numbered_step(
            colors,
            2,
            "选择文件后等待任务完成",
            "导入不会要求你手动整理 versions 目录；完成后可以直接去“管理”页查看。",
        ))
        .child(primary_action(
            colors,
            "现在选择一个安装包",
            |_, window, cx| pick_and_import_local_version(window, cx),
        ))
        .into_any_element()
}

fn render_version_management(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(route_badge(colors, "当前页面：管理"))
        .child(intro_text(
            colors,
            "下载或导入完成后，游戏版本都会在这里管理。左侧选择版本，右侧查看这个版本对应的资源、地图、模组和设置。",
        ))
        .child(numbered_step(
            colors,
            1,
            "左侧选择一个版本",
            "列表代表 BMCBL/versions 中已经存在的游戏版本；顶部“+”也可以继续导入版本。",
        ))
        .child(numbered_step(
            colors,
            2,
            "右侧管理这个版本的数据",
            "地图、资源包、模组、截图和服务器等内容都围绕当前选中的版本显示。",
        ))
        .child(numbered_step(
            colors,
            3,
            "需要隔离旧版本数据时再改设置",
            "跨大版本打开世界可能产生不可逆升级。不了解选项含义时保持默认，不要随意关闭数据保护。",
        ))
        .child(tip_box(
            colors,
            "版本文件和游戏存档不是一回事：删除一个 versions 目录前，先确认你是否还需要这个版本。",
        ))
        .into_any_element()
}

fn render_platform_setup(state: &OnboardingTourState, colors: &ThemeColors) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(14.));

    #[cfg(target_os = "windows")]
    {
        body = body
            .child(route_badge(colors, "Windows：UWP 注册与数据保护"))
            .child(intro_text(
                colors,
                "旧版 UWP 版本切换涉及 Windows 包注册。BMCBL 会把“先保护数据，再替换注册”作为强制安全门，而不是把风险交给用户记住。",
            ))
            .child(numbered_step(
                colors,
                1,
                "BMCBL 散装版本之间切换",
                "目标版本使用 DevelopmentMode 注册，切换时重新指向对应版本目录。",
            ))
            .child(numbered_step(
                colors,
                2,
                "首次从 Store/外部 UWP 切换",
                "如果检测到 games/com.mojang 数据，会先复制到迁移备份并校验；备份失败直接停止，不允许卸载。",
            ))
            .child(numbered_step(
                colors,
                3,
                "新版本注册成功后",
                "只有目标数据目录为空时才自动恢复备份，避免覆盖已经存在的新数据。",
            ));
    }

    #[cfg(target_os = "linux")]
    {
        body = body
            .child(route_badge(colors, "当前页面：设置 → Proton-GDK"))
            .child(intro_text(
                colors,
                "Linux 不注册 Windows UWP。这里真正需要确认的是 Proton-GDK/UMU 兼容运行环境是否可用。",
            ))
            .child(numbered_step(
                colors,
                1,
                "选择或安装 Proton-GDK",
                "BMCBL 启动 Bedrock 时使用这里配置的 runner。没有运行环境时游戏下载完成也无法正常启动。",
            ))
            .child(numbered_step(
                colors,
                2,
                "处理必要的系统依赖",
                "如果系统缺少 32 位 glibc，BMCBL 的 Linux runtime 检测会给出明确提示；这与 UWP 无关。",
            ));
    }

    if state.platform_scanning {
        body = body.child(
            div()
                .w_full()
                .p(px(14.))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.08,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    svg()
                        .path(lucide_icons::icon_loader_circle())
                        .size(px(17.))
                        .text_color(colors.accent),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child("正在检测当前电脑的实际环境…"),
                ),
        );
    } else if let Some(summary) = &state.platform_summary {
        body = body.child(platform_summary_card(colors, summary));
    }

    if let Some(error) = &state.error {
        body = body.child(error_box(colors, error));
    }

    #[cfg(target_os = "linux")]
    {
        body = body.child(secondary_action(colors, "打开 Proton-GDK 设置", |_, _, cx| {
            crate::ui::onboarding::open_platform_settings(cx);
        }));
    }

    body.into_any_element()
}

fn render_finish(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .w_full()
                .py(px(18.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(52.))
                        .h(px(52.))
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(Hsla {
                            a: 0.14,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(lucide_icons::icon_circle_check())
                                .size(px(25.))
                                .text_color(colors.accent),
                        ),
                )
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("基本操作已经看完"),
                )
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .text_size(px(12.))
                        .line_height(px(19.))
                        .text_color(colors.text_secondary)
                        .child("以后忘记入口，可以从“设置 → 关于 → 交互式首次运行导览”重新打开，不会重置你的游戏或配置。"),
                ),
        )
        .child(primary_action(colors, "去下载游戏版本", |_, _, cx| {
            crate::ui::onboarding::finish_to_download(cx);
        }))
        .child(secondary_action(colors, "查看已有版本", |_, _, cx| {
            crate::ui::onboarding::finish_to_manage(cx);
        }))
        .into_any_element()
}

fn render_footer(
    scene: OnboardingScene,
    state: &OnboardingTourState,
    colors: &ThemeColors,
) -> Div {
    let left_label = if scene == OnboardingScene::Welcome {
        "跳过引导"
    } else {
        "上一步"
    };
    let left = secondary_button(colors, left_label).on_mouse_down(
        MouseButton::Left,
        move |_, _, cx| {
            if scene == OnboardingScene::Welcome {
                crate::ui::onboarding::skip(cx);
            } else {
                crate::ui::onboarding::back(cx);
            }
        },
    );

    let next_enabled = scene != OnboardingScene::PlatformSetup || !state.platform_scanning;
    let next_label = if scene == OnboardingScene::Finish {
        "完成"
    } else {
        "下一步"
    };
    let mut next = primary_button(colors, next_label, next_enabled);
    if next_enabled {
        next = next.on_mouse_down(MouseButton::Left, move |_, _, cx| {
            if scene == OnboardingScene::Finish {
                crate::ui::onboarding::finish(cx);
            } else {
                crate::ui::onboarding::advance(cx);
            }
        });
    }

    div()
        .px(px(22.))
        .py(px(15.))
        .border_t_1()
        .border_color(Hsla {
            a: 0.35,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .child(left)
        .child(next)
}

fn render_spotlight(
    scene: OnboardingScene,
    size: Size<Pixels>,
    colors: &ThemeColors,
    compact: bool,
) -> Option<AnyElement> {
    if compact {
        return None;
    }

    let width = size.width / px(1.0);
    let height = size.height / px(1.0);
    let frame = match scene {
        OnboardingScene::DownloadOverview => Some((
            154.0,
            82.0,
            (width - 598.0).max(260.0),
            86.0,
            "这里是下载页的搜索、筛选和操作区",
        )),
        OnboardingScene::ImportPackage => Some((
            (width - 170.0).max(450.0),
            86.0,
            92.0,
            70.0,
            "上传图标：导入 APPX / ZIP / MSIXVC",
        )),
        OnboardingScene::VersionManagement => Some((
            154.0,
            92.0,
            (width - 598.0).max(230.0).min(300.0),
            (height - 126.0).max(280.0),
            "左侧版本列表：先选择要管理的版本",
        )),
        #[cfg(target_os = "linux")]
        OnboardingScene::PlatformSetup => Some((
            438.0,
            92.0,
            (width - 460.0).max(280.0),
            (height - 126.0).max(280.0),
            "这里是 Proton-GDK 配置页面",
        )),
        _ => None,
    }?;

    Some(
        div()
            .absolute()
            .left(px(frame.0))
            .top(px(frame.1))
            .w(px(frame.2))
            .h(px(frame.3))
            .rounded(px(crate::ui::theme::tokens::radius::MD))
            .border_2()
            .border_color(colors.accent)
            .bg(Hsla {
                a: 0.035,
                ..colors.accent
            })
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(-30.))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .bg(colors.accent)
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.btn_primary_text)
                    .child(frame.4),
            )
            .into_any_element(),
    )
}

fn scene_header(scene: OnboardingScene) -> (&'static str, &'static str, &'static str) {
    match scene {
        OnboardingScene::Welcome => (
            lucide_icons::icon_route(),
            "欢迎使用 BMCBL",
            "先认识最常用的路径，之后再逐步了解高级功能。",
        ),
        OnboardingScene::DownloadOverview => (
            lucide_icons::icon_download(),
            "怎么下载 Minecraft？",
            "已经自动切换到真实下载页。",
        ),
        OnboardingScene::ImportPackage => (
            lucide_icons::icon_upload(),
            "已经有安装包怎么办？",
            "继续留在下载页，认识本地版本导入入口。",
        ),
        OnboardingScene::VersionManagement => (
            lucide_icons::icon_settings_2(),
            "下载完成后去哪里？",
            "已经自动切换到版本管理页。",
        ),
        OnboardingScene::PlatformSetup => {
            #[cfg(target_os = "windows")]
            {
                (
                    lucide_icons::icon_shield_check(),
                    "Windows UWP 数据怎么保护？",
                    "检查当前注册来源和需要保护的 Minecraft 数据。",
                )
            }
            #[cfg(target_os = "linux")]
            {
                (
                    lucide_icons::icon_settings_2(),
                    "Linux 怎么运行 Bedrock？",
                    "检查 Proton-GDK / UMU 和当前系统环境。",
                )
            }
        }
        OnboardingScene::Finish => (
            lucide_icons::icon_circle_check(),
            "可以开始使用了",
            "选择下一步要做的事情，或者直接完成导览。",
        ),
    }
}

fn lesson_card(
    colors: &ThemeColors,
    icon: &'static str,
    title: &'static str,
    description: &'static str,
) -> Div {
    div()
        .w_full()
        .p(px(13.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.42,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.68,
            ..colors.surface
        })
        .flex()
        .items_start()
        .gap(px(11.))
        .child(
            div()
                .flex_none()
                .w(px(36.))
                .h(px(36.))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.13,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(17.)).text_color(colors.accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .w_full()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(colors.text_secondary)
                        .child(description),
                ),
        )
}

fn numbered_step(
    colors: &ThemeColors,
    number: usize,
    title: &'static str,
    description: &'static str,
) -> Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(11.))
        .child(
            div()
                .flex_none()
                .w(px(26.))
                .h(px(26.))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.14,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .w_full()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(colors.text_secondary)
                        .child(description),
                ),
        )
}

fn format_card(colors: &ThemeColors, format: &'static str, description: &'static str) -> Div {
    div()
        .w_full()
        .p(px(11.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.06,
            ..colors.accent
        })
        .flex()
        .items_start()
        .gap(px(10.))
        .child(
            div()
                .flex_none()
                .min_w(px(66.))
                .px(px(9.))
                .py(px(5.))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.12,
                    ..colors.accent
                })
                .text_center()
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(format),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(colors.text_secondary)
                .child(description),
        )
}

fn platform_summary_card(
    colors: &ThemeColors,
    summary: &crate::ui::onboarding::state::OnboardingPlatformSummary,
) -> Div {
    let mut items = div().w_full().flex().flex_col().gap(px(9.));
    for item in &summary.items {
        let icon = if item.warning {
            lucide_icons::icon_triangle_alert()
        } else {
            lucide_icons::icon_circle_check()
        };
        let color = if item.warning {
            colors.danger
        } else {
            colors.accent
        };
        items = items.child(
            div()
                .w_full()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(
                    div().flex_none().pt(px(1.)).child(
                        svg().path(icon).size(px(15.)).text_color(color),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .w_full()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_size(px(11.))
                                .line_height(px(17.))
                                .text_color(colors.text_secondary)
                                .child(item.value.clone()),
                        ),
                ),
        );
    }

    div()
        .w_full()
        .p(px(13.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.38,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .w_full()
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_primary)
                .child(summary.title.clone()),
        )
        .child(
            div()
                .w_full()
                .text_size(px(11.))
                .line_height(px(17.))
                .text_color(colors.text_secondary)
                .child(summary.detail.clone()),
        )
        .child(items)
}

fn route_badge(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .w_full()
        .px(px(10.))
        .py(px(8.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.09,
            ..colors.accent
        })
        .flex()
        .items_center()
        .gap(px(7.))
        .child(
            div().flex_none().child(
                svg()
                    .path(lucide_icons::icon_map_pin())
                    .size(px(14.))
                    .text_color(colors.accent),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.accent)
                .child(label),
        )
}

fn intro_text(colors: &ThemeColors, text: &'static str) -> Div {
    div()
        .w_full()
        .text_size(px(12.))
        .line_height(px(19.))
        .text_color(colors.text_secondary)
        .child(text)
}

fn tip_box(colors: &ThemeColors, text: &'static str) -> Div {
    div()
        .w_full()
        .p(px(11.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.07,
            ..colors.text_secondary
        })
        .flex()
        .items_start()
        .gap(px(8.))
        .child(
            div().flex_none().pt(px(1.)).child(
                svg()
                    .path(lucide_icons::icon_info())
                    .size(px(15.))
                    .text_color(colors.text_secondary),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(11.))
                .line_height(px(17.))
                .text_color(colors.text_secondary)
                .child(text),
        )
}

fn error_box(colors: &ThemeColors, error: &str) -> Div {
    div()
        .w_full()
        .p(px(11.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.10,
            ..colors.danger
        })
        .flex()
        .items_start()
        .gap(px(8.))
        .child(
            div().flex_none().pt(px(1.)).child(
                svg()
                    .path(lucide_icons::icon_triangle_alert())
                    .size(px(15.))
                    .text_color(colors.danger),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(11.))
                .line_height(px(17.))
                .text_color(colors.text_secondary)
                .child(error.to_string()),
        )
}

fn primary_action(
    colors: &ThemeColors,
    label: &'static str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    primary_button(colors, label, true).on_mouse_down(MouseButton::Left, on_click)
}

fn secondary_action(
    colors: &ThemeColors,
    label: &'static str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    secondary_button(colors, label).on_mouse_down(MouseButton::Left, on_click)
}

fn primary_button(colors: &ThemeColors, label: &'static str, enabled: bool) -> Stateful<Div> {
    let mut button = div()
        .id(SharedString::from(format!("onboarding-tour-primary-{label}")))
        .min_h(px(38.))
        .px(px(15.))
        .py(px(9.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label);
    if enabled {
        button = button
            .bg(colors.accent)
            .text_color(colors.btn_primary_text)
            .cursor_pointer()
            .hover(|this| this.bg(colors.accent_hover));
    } else {
        button = button.bg(colors.surface).text_color(colors.text_muted);
    }
    button
}

fn secondary_button(colors: &ThemeColors, label: &'static str) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("onboarding-tour-secondary-{label}")))
        .min_h(px(38.))
        .px(px(14.))
        .py(px(9.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.44,
            ..colors.border
        })
        .bg(colors.surface)
        .text_color(colors.text_primary)
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

fn pick_and_import_local_version(window: &Window, cx: &mut App) {
    let Some(path) = crate::utils::file_picker::pick_file_path_with_filter_for_window(
        window,
        "Minecraft 游戏版本安装包",
        crate::core::minecraft::local_package::LOCAL_GAME_PACKAGE_EXTENSIONS,
    ) else {
        return;
    };

    cx.spawn(async move |cx| {
        let result = crate::core::minecraft::local_package::start_local_game_package_import(path).await;
        cx.update(|cx| match result {
            Ok(_) => crate::ui::components::toast::push(
                cx,
                SharedString::from("游戏版本导入任务已开始"),
            ),
            Err(error) => crate::ui::components::toast::error(cx, SharedString::from(error)),
        })
    })
    .detach();
}
