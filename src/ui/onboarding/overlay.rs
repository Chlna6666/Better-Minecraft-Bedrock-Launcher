use gpui::*;
use lucide_gpui::icons as lucide_icons;

use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::onboarding::state::{OnboardingScene, OnboardingTourState};
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

const PANEL_WIDTH: f32 = 398.0;
const PANEL_MARGIN: f32 = 22.0;
const PANEL_TOP: f32 = 82.0;
const DOWNLOAD_PANEL_TOP: f32 = 188.0;
const COMPACT_MARGIN: f32 = 14.0;
const CALLOUT_GAP: f32 = 10.0;
const CALLOUT_WIDTH: f32 = 280.0;

#[derive(Clone, Copy, Debug)]
struct RectF {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl RectF {
    fn right(self) -> f32 {
        self.x + self.w
    }

    fn bottom(self) -> f32 {
        self.y + self.h
    }

    fn padded(self, amount: f32) -> Self {
        Self {
            x: self.x - amount,
            y: self.y - amount,
            w: self.w + amount * 2.0,
            h: self.h + amount * 2.0,
        }
    }

    fn clamp_to_viewport(self, width: f32, height: f32, margin: f32) -> Self {
        let max_w = (width - margin * 2.0).max(1.0);
        let max_h = (height - margin * 2.0).max(1.0);
        let w = self.w.min(max_w);
        let h = self.h.min(max_h);
        Self {
            x: self.x.clamp(margin, (width - w - margin).max(margin)),
            y: self.y.clamp(margin, (height - h - margin).max(margin)),
            w,
            h,
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }
}

#[derive(Clone, Copy, Debug)]
enum PanelSide {
    Left,
    Right,
    RightLower,
}

#[derive(Clone, Copy, Debug)]
struct SceneGeometry {
    panel: RectF,
    focus: Option<RectF>,
    callout: Option<RectF>,
}

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
    let width = size.width / px(1.0);
    let height = size.height / px(1.0);
    let compact = width < 860.0 || height < 560.0;
    let geometry = scene_geometry(state.scene, width, height, compact);

    let mut root = div()
        .absolute()
        .inset_0()
        .child(render_dim_layer(compact));

    // Layer 2: spotlight。这里只负责目标框，不再把说明气泡作为它的子元素，
    // 避免提示被目标框自己的尺寸、裁剪或窗口边缘限制。
    if let Some(focus) = geometry.focus {
        root = root.child(render_spotlight(focus, &colors));
    }

    // Layer 3: 主教学面板。下载步骤主动下移，保证正在介绍的完整工具栏可见。
    root = root.child(render_panel_layer(
        geometry.panel,
        render_guide_panel(state, &colors, compact),
    ));

    // Layer 4: callout 永远最后绘制，是引导自己的最高层。
    if let Some(callout) = geometry.callout {
        if let Some(text) = scene_callout_text(state.scene) {
            root = root.child(render_callout_layer(callout, text, &colors));
        }
    }

    root.into_any_element()
}

fn render_dim_layer(compact: bool) -> Div {
    div().absolute().inset_0().bg(Hsla {
        a: if compact { 0.08 } else { 0.14 },
        ..black()
    })
}

fn render_panel_layer(bounds: RectF, panel: Div) -> Div {
    div()
        .absolute()
        .left(px(bounds.x))
        .top(px(bounds.y))
        .w(px(bounds.w))
        .h(px(bounds.h))
        .child(panel)
}

fn render_spotlight(bounds: RectF, colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .left(px(bounds.x))
        .top(px(bounds.y))
        .w(px(bounds.w))
        .h(px(bounds.h))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_2()
        .border_color(colors.accent)
        .bg(Hsla {
            a: 0.025,
            ..colors.accent
        })
}

fn render_callout_layer(bounds: RectF, text: &'static str, colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .left(px(bounds.x))
        .top(px(bounds.y))
        .w(px(bounds.w))
        .min_h(px(34.))
        .px(px(11.))
        .py(px(8.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.58,
            ..colors.accent
        })
        .bg(colors.accent)
        .shadow_lg()
        .flex()
        .items_start()
        .gap(px(8.))
        .child(
            div()
                .flex_none()
                .pt(px(1.))
                .child(
                    svg()
                        .path(lucide_icons::icon_mouse_pointer_2())
                        .size(px(14.))
                        .text_color(colors.btn_primary_text),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(11.))
                .line_height(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.btn_primary_text)
                .child(text),
        )
}

fn scene_geometry(scene: OnboardingScene, width: f32, height: f32, compact: bool) -> SceneGeometry {
    if compact {
        let panel_h = (height - 110.0).min(430.0).max(300.0);
        return SceneGeometry {
            panel: RectF {
                x: COMPACT_MARGIN,
                y: (height - COMPACT_MARGIN - panel_h).max(COMPACT_MARGIN),
                w: (width - COMPACT_MARGIN * 2.0).max(320.0),
                h: panel_h,
            }
            .clamp_to_viewport(width, height, COMPACT_MARGIN),
            focus: None,
            callout: None,
        };
    }

    let side = match scene {
        OnboardingScene::ImportPackage => PanelSide::Left,
        OnboardingScene::DownloadOverview => PanelSide::RightLower,
        #[cfg(target_os = "linux")]
        OnboardingScene::PlatformSetup => PanelSide::Left,
        _ => PanelSide::Right,
    };
    let panel = desktop_panel_rect(side, width, height);

    // 页面坐标来自当前统一 page_shell，而不是旧版本遗留的 154/438 等魔法数。
    let page_x = crate::ui::components::page_shell::PAGE_INSET_X / px(1.0);
    let page_y = crate::ui::components::page_shell::PAGE_INSET_TOP / px(1.0);
    let page_bottom = crate::ui::components::page_shell::PAGE_INSET_BOTTOM / px(1.0);
    let sidebar_w = crate::ui::components::page_shell::SPLIT_PAGE_SIDEBAR_WIDTH / px(1.0);

    let focus = match scene {
        OnboardingScene::DownloadOverview => Some(
            RectF {
                x: page_x,
                y: page_y,
                w: (width - page_x * 2.0).max(240.0),
                h: 68.0,
            }
            .padded(6.0)
            .clamp_to_viewport(width, height, 8.0),
        ),
        OnboardingScene::ImportPackage => {
            // download toolbar: page right inset 22 + toolbar right padding 20 +
            // refresh 32 + gap 12 + import 32。按真实控件布局反推导入按钮位置。
            let import_x = width - page_x - 20.0 - 32.0 - 12.0 - 32.0;
            Some(
                RectF {
                    x: import_x,
                    y: page_y + 14.0,
                    w: 32.0,
                    h: 32.0,
                }
                .padded(8.0)
                .clamp_to_viewport(width, height, 8.0),
            )
        }
        OnboardingScene::VersionManagement => Some(
            RectF {
                x: page_x,
                y: page_y,
                w: sidebar_w,
                h: (height - page_y - page_bottom).max(240.0),
            }
            .padded(5.0)
            .clamp_to_viewport(width, height, 8.0),
        ),
        _ => None,
    };

    let callout = focus.and_then(|focus| {
        let preferred = match scene {
            OnboardingScene::DownloadOverview => RectF {
                x: focus.x + 12.0,
                y: focus.bottom() + CALLOUT_GAP,
                w: CALLOUT_WIDTH,
                h: 44.0,
            },
            OnboardingScene::ImportPackage => RectF {
                x: focus.x - CALLOUT_WIDTH - CALLOUT_GAP,
                y: focus.y,
                w: CALLOUT_WIDTH,
                h: 62.0,
            },
            OnboardingScene::VersionManagement => RectF {
                x: focus.right() + CALLOUT_GAP,
                y: focus.y + 14.0,
                w: CALLOUT_WIDTH,
                h: 48.0,
            },
            _ => return None,
        };
        Some(place_callout(preferred, focus, panel, width, height))
    });

    SceneGeometry {
        panel,
        focus,
        callout,
    }
}

fn desktop_panel_rect(side: PanelSide, width: f32, height: f32) -> RectF {
    let top = match side {
        PanelSide::RightLower => DOWNLOAD_PANEL_TOP,
        _ => PANEL_TOP,
    };
    let available_h = (height - top - PANEL_MARGIN).max(320.0);
    let w = PANEL_WIDTH.min((width - PANEL_MARGIN * 2.0).max(320.0));
    let x = match side {
        PanelSide::Left => PANEL_MARGIN,
        PanelSide::Right | PanelSide::RightLower => width - PANEL_MARGIN - w,
    };
    RectF {
        x,
        y: top,
        w,
        h: available_h,
    }
    .clamp_to_viewport(width, height, PANEL_MARGIN)
}

fn place_callout(
    preferred: RectF,
    focus: RectF,
    panel: RectF,
    width: f32,
    height: f32,
) -> RectF {
    let margin = 10.0;
    let mut candidate = preferred.clamp_to_viewport(width, height, margin);
    if !candidate.intersects(panel) {
        return candidate;
    }

    // 首选位置与教学面板冲突时，依次尝试目标下方、上方、左侧、右侧。
    let candidates = [
        RectF {
            x: focus.x,
            y: focus.bottom() + CALLOUT_GAP,
            w: preferred.w,
            h: preferred.h,
        },
        RectF {
            x: focus.x,
            y: focus.y - preferred.h - CALLOUT_GAP,
            w: preferred.w,
            h: preferred.h,
        },
        RectF {
            x: focus.x - preferred.w - CALLOUT_GAP,
            y: focus.y,
            w: preferred.w,
            h: preferred.h,
        },
        RectF {
            x: focus.right() + CALLOUT_GAP,
            y: focus.y,
            w: preferred.w,
            h: preferred.h,
        },
    ];

    for item in candidates {
        let placed = item.clamp_to_viewport(width, height, margin);
        if !placed.intersects(panel) && !placed.intersects(focus) {
            return placed;
        }
    }

    candidate = candidate.clamp_to_viewport(width, height, margin);
    candidate
}

fn scene_callout_text(scene: OnboardingScene) -> Option<&'static str> {
    match scene {
        OnboardingScene::DownloadOverview => Some("下载页工具栏：游戏/资源包/模组、搜索、版本筛选、导入和刷新都在这里。"),
        OnboardingScene::ImportPackage => Some("上传按钮：点击这里导入 APPX、ZIP 或 MSIXVC，本地已有安装包不用重新下载。"),
        OnboardingScene::VersionManagement => Some("版本管理侧栏：先从这里选择要管理的本地 Minecraft 版本。"),
        _ => None,
    }
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
                .px(px(if compact { 18.0 } else { 22.0 }))
                .py(px(18.0))
                .child(render_scene_body(state, colors)),
        )
        .child(render_footer(state.scene, state, colors))
}

fn render_header(state: &OnboardingTourState, colors: &ThemeColors) -> Div {
    let (icon, title, subtitle) = scene_header(state.scene);
    div()
        .px(px(22.0))
        .pt(px(20.0))
        .pb(px(16.0))
        .border_b_1()
        .border_color(Hsla {
            a: 0.35,
            ..colors.border
        })
        .flex()
        .items_start()
        .gap(px(12.0))
        .child(
            div()
                .flex_none()
                .w(px(42.0))
                .h(px(42.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.15,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(20.0)).text_color(colors.accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .w_full()
                        .text_size(px(17.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(colors.text_secondary)
                        .child(subtitle),
                ),
        )
        .child(
            div()
                .flex_none()
                .px(px(9.0))
                .py(px(5.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.accent
                })
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.accent)
                .child(format!("{} / {}", state.scene.index(), OnboardingScene::COUNT)),
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
        .gap(px(14.0))
        .child(intro_text(
            colors,
            "接下来不会只显示说明文字。BMCBL 会自动进入真实页面，并把正在介绍的区域高亮出来。你可以直接操作底层页面，也可以只按“下一步”完成导览。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_download(),
            "下载游戏版本",
            "认识正式版、Preview、搜索、筛选和版本列表。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_upload(),
            "导入本地安装包",
            "已有 APPX、ZIP 或 MSIXVC 时直接导入，不需要重复下载。",
        ))
        .child(feature_card(
            colors,
            lucide_icons::icon_settings_2(),
            "管理本地版本",
            "下载完成后从管理页选择版本，再管理地图、资源、模组和版本设置。",
        ))
        .child(tip_box(
            colors,
            "绿色描边是当前要看的真实区域；绿色小提示属于引导最高层，不会再被主教学面板裁切。",
        ))
        .into_any_element()
}

fn render_download_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(route_badge(colors, "当前页面：下载 → 游戏"))
        .child(intro_text(
            colors,
            "顶部这一整行就是下载页的控制区。先决定要找什么，再从下方列表选择版本。",
        ))
        .child(numbered_step(
            colors,
            1,
            "保持“游戏”标签",
            "资源包和模组有各自独立页面；下载 Minecraft 本体时使用“游戏”。",
        ))
        .child(numbered_step(
            colors,
            2,
            "搜索和筛选",
            "知道版本号时直接搜索；不知道时保持默认筛选，优先选择最新正式版。",
        ))
        .child(numbered_step(
            colors,
            3,
            "从列表下载",
            "点击目标版本右侧的下载/安装操作。BMCBL 会完成下载、校验和解包。",
        ))
        .child(tip_box(
            colors,
            "Preview/测试版用于体验新内容。普通游玩不要因为版本号更大就优先选择 Preview。",
        ))
        .into_any_element()
}

fn render_import_package(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(route_badge(colors, "当前页面：下载 → 上传按钮"))
        .child(intro_text(
            colors,
            "如果你已经有 Minecraft 安装包，不需要重新下载。右上角高亮的是实际导入按钮。",
        ))
        .child(format_card(colors, "APPX", "常见的 Minecraft UWP 安装包。"))
        .child(format_card(colors, "ZIP", "BMCBL 支持的 ZIP 游戏版本包。"))
        .child(format_card(
            colors,
            "MSIXVC",
            "部分 GDK 版本使用的安装包格式，会进入对应解包流程。",
        ))
        .child(numbered_step(
            colors,
            1,
            "点击上传按钮",
            "系统文件选择器会筛选 BMCBL 支持的游戏安装包。",
        ))
        .child(numbered_step(
            colors,
            2,
            "选择文件并等待任务完成",
            "BMCBL 自动解析并整理版本目录，不需要手工复制到 versions。",
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
        .gap(px(14.0))
        .child(route_badge(colors, "当前页面：管理"))
        .child(intro_text(
            colors,
            "管理页是“本地已经有的版本”的入口。左侧固定侧栏用于选版本，右侧内容会跟随当前版本切换。",
        ))
        .child(numbered_step(
            colors,
            1,
            "左侧选择版本",
            "这里对应 BMCBL/versions 中的本地版本。顶部“+”也可以继续导入。",
        ))
        .child(numbered_step(
            colors,
            2,
            "右侧管理该版本",
            "地图、资源包、模组、截图、服务器和版本设置都围绕当前选中的版本。",
        ))
        .child(numbered_step(
            colors,
            3,
            "旧版本注意数据隔离",
            "跨大版本直接打开同一个世界可能发生不可逆升级；不理解高级选项时保持默认。",
        ))
        .into_any_element()
}

fn render_platform_setup(state: &OnboardingTourState, colors: &ThemeColors) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(14.0));

    #[cfg(target_os = "windows")]
    {
        body = body
            .child(route_badge(colors, "Windows：UWP 注册与数据保护"))
            .child(intro_text(
                colors,
                "旧版 UWP 切换涉及 Windows 包注册。BMCBL 把“先保护数据，再替换注册”作为运行时强制安全门，不依赖用户记住。",
            ))
            .child(numbered_step(
                colors,
                1,
                "BMCBL 散装 UWP 多版本",
                "切换版本时重新把 DevelopmentMode 注册指向目标版本目录。",
            ))
            .child(numbered_step(
                colors,
                2,
                "Store/外部 UWP 首次迁移",
                "检测到 games/com.mojang 时先备份和校验；备份失败会阻止卸载。",
            ))
            .child(numbered_step(
                colors,
                3,
                "注册成功后恢复",
                "只有目标数据目录为空时才恢复迁移备份，避免覆盖新数据。",
            ));
    }

    #[cfg(target_os = "linux")]
    {
        body = body
            .child(route_badge(colors, "当前页面：设置 → Proton-GDK"))
            .child(intro_text(
                colors,
                "Linux 不执行 Windows UWP 注册。这里需要确认的是 Proton-GDK/UMU 兼容运行环境。",
            ))
            .child(numbered_step(
                colors,
                1,
                "选择或安装 Proton-GDK",
                "没有可用 runner 时，游戏下载完成也无法正常启动。",
            ))
            .child(numbered_step(
                colors,
                2,
                "检查系统依赖",
                "缺少 32 位 glibc 等依赖时，Linux runtime 检测会给出明确提示。",
            ));
    }

    if state.platform_scanning {
        body = body.child(status_box(
            colors,
            lucide_icons::icon_loader_circle(),
            "正在检查当前平台环境…",
            false,
        ));
    } else if let Some(error) = state.error.as_deref() {
        body = body.child(error_box(colors, error));
    } else if let Some(summary) = &state.platform_summary {
        body = body.child(platform_summary_card(colors, summary));
    } else {
        body = body.child(tip_box(colors, "正在等待平台环境检测结果。"));
    }

    #[cfg(target_os = "linux")]
    {
        body = body.child(primary_action(colors, "打开 Proton-GDK 设置", |_, _, cx| {
            crate::ui::onboarding::open_platform_settings(cx);
        }));
    }

    body.into_any_element()
}

fn render_finish(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(
            div()
                .w_full()
                .py(px(18.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .w(px(52.0))
                        .h(px(52.0))
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
                                .size(px(25.0))
                                .text_color(colors.accent),
                        ),
                )
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("基本操作已经看完"),
                )
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .text_size(px(12.0))
                        .line_height(px(19.0))
                        .text_color(colors.text_secondary)
                        .child("以后可以从“设置 → 关于 → 交互式首次运行导览”重新打开。重新打开不会重置游戏、版本或设置。"),
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
        .px(px(22.0))
        .py(px(15.0))
        .border_t_1()
        .border_color(Hsla {
            a: 0.35,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(left)
        .child(next)
}

fn scene_header(scene: OnboardingScene) -> (&'static str, &'static str, &'static str) {
    match scene {
        OnboardingScene::Welcome => (
            lucide_icons::icon_route(),
            "欢迎使用 BMCBL",
            "跟着真实页面认识下载、导入和版本管理。",
        ),
        OnboardingScene::DownloadOverview => (
            lucide_icons::icon_download(),
            "怎么下载 Minecraft？",
            "已经自动切换到真实下载页。",
        ),
        OnboardingScene::ImportPackage => (
            lucide_icons::icon_upload(),
            "已经有安装包怎么办？",
            "高亮的是下载页真实导入按钮。",
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
                    "检查当前注册来源和需要保护的数据。",
                )
            }
            #[cfg(target_os = "linux")]
            {
                (
                    lucide_icons::icon_settings_2(),
                    "Linux 怎么运行 Bedrock？",
                    "检查 Proton-GDK / UMU 和系统环境。",
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

fn feature_card(
    colors: &ThemeColors,
    icon: &'static str,
    title: &'static str,
    description: &'static str,
) -> Div {
    div()
        .w_full()
        .p(px(13.0))
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
        .gap(px(11.0))
        .child(
            div()
                .flex_none()
                .w(px(36.0))
                .h(px(36.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.13,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(17.0)).text_color(colors.accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .w_full()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
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
        .gap(px(11.0))
        .child(
            div()
                .flex_none()
                .w(px(26.0))
                .h(px(26.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.14,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .w_full()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(colors.text_secondary)
                        .child(description),
                ),
        )
}

fn format_card(colors: &ThemeColors, format: &'static str, description: &'static str) -> Div {
    div()
        .w_full()
        .p(px(11.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.06,
            ..colors.accent
        })
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(
            div()
                .flex_none()
                .min_w(px(66.0))
                .px(px(9.0))
                .py(px(5.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.12,
                    ..colors.accent
                })
                .text_center()
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(format),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(colors.text_secondary)
                .child(description),
        )
}

fn platform_summary_card(
    colors: &ThemeColors,
    summary: &crate::ui::onboarding::state::OnboardingPlatformSummary,
) -> Div {
    let mut items = div().w_full().flex().flex_col().gap(px(9.0));
    for item in &summary.items {
        let icon = if item.warning {
            lucide_icons::icon_triangle_alert()
        } else {
            lucide_icons::icon_circle_check()
        };
        let color = if item.warning { colors.danger } else { colors.accent };
        items = items.child(
            div()
                .w_full()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_none()
                        .pt(px(1.0))
                        .child(svg().path(icon).size(px(15.0)).text_color(color)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .w_full()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_size(px(11.0))
                                .line_height(px(17.0))
                                .text_color(colors.text_secondary)
                                .child(item.value.clone()),
                        ),
                ),
        );
    }

    div()
        .w_full()
        .p(px(13.0))
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
        .gap(px(10.0))
        .child(
            div()
                .w_full()
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_primary)
                .child(summary.title.clone()),
        )
        .child(
            div()
                .w_full()
                .text_size(px(11.0))
                .line_height(px(17.0))
                .text_color(colors.text_secondary)
                .child(summary.detail.clone()),
        )
        .child(items)
}

fn route_badge(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .w_full()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.09,
            ..colors.accent
        })
        .flex()
        .items_center()
        .gap(px(7.0))
        .child(
            div()
                .flex_none()
                .child(svg().path(lucide_icons::icon_map_pin()).size(px(14.0)).text_color(colors.accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.accent)
                .child(label),
        )
}

fn intro_text(colors: &ThemeColors, text: &'static str) -> Div {
    div()
        .w_full()
        .text_size(px(12.0))
        .line_height(px(19.0))
        .text_color(colors.text_secondary)
        .child(text)
}

fn tip_box(colors: &ThemeColors, text: &'static str) -> Div {
    status_box(colors, lucide_icons::icon_info(), text, false)
}

fn error_box(colors: &ThemeColors, error: &str) -> Div {
    div()
        .w_full()
        .p(px(11.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.10,
            ..colors.danger
        })
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(
            div()
                .flex_none()
                .pt(px(1.0))
                .child(svg().path(lucide_icons::icon_triangle_alert()).size(px(15.0)).text_color(colors.danger)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(11.0))
                .line_height(px(17.0))
                .text_color(colors.text_secondary)
                .child(error.to_string()),
        )
}

fn status_box(
    colors: &ThemeColors,
    icon: &'static str,
    text: &'static str,
    danger: bool,
) -> Div {
    let color = if danger { colors.danger } else { colors.text_secondary };
    div()
        .w_full()
        .p(px(11.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.07,
            ..color
        })
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(
            div()
                .flex_none()
                .pt(px(1.0))
                .child(svg().path(icon).size(px(15.0)).text_color(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(11.0))
                .line_height(px(17.0))
                .text_color(colors.text_secondary)
                .child(text),
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
        .min_h(px(38.0))
        .px(px(15.0))
        .py(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
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
        .min_h(px(38.0))
        .px(px(14.0))
        .py(px(9.0))
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
        .text_size(px(12.0))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_toolbar_uses_current_page_shell_metrics() {
        let geometry = scene_geometry(OnboardingScene::DownloadOverview, 1215.0, 750.0, false);
        let focus = geometry.focus.expect("download focus");
        assert!(focus.x < 30.0);
        assert!(focus.w > 1100.0);
        assert!(geometry.panel.y > focus.bottom());
    }

    #[test]
    fn import_callout_stays_in_viewport_and_outside_panel() {
        let geometry = scene_geometry(OnboardingScene::ImportPackage, 1215.0, 750.0, false);
        let callout = geometry.callout.expect("import callout");
        assert!(callout.x >= 10.0);
        assert!(callout.right() <= 1205.0);
        assert!(!callout.intersects(geometry.panel));
    }

    #[test]
    fn manage_focus_matches_fixed_sidebar_width() {
        let geometry = scene_geometry(OnboardingScene::VersionManagement, 1215.0, 750.0, false);
        let focus = geometry.focus.expect("manage focus");
        assert!(focus.x < 30.0);
        assert!((focus.w - 290.0).abs() < 20.0);
    }
}
