use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::state::{OnboardingAnchor, OnboardingScene, OnboardingTourState};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

const PANEL_WIDTH: f32 = 390.0;
const PANEL_MARGIN: f32 = 22.0;
const PANEL_TOP: f32 = 84.0;
const CALLOUT_WIDTH: f32 = 310.0;
const CALLOUT_GAP: f32 = 10.0;

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

    fn clamp(self, width: f32, height: f32, margin: f32) -> Self {
        let max_w = (width - margin * 2.0).max(1.0);
        let max_h = (height - margin * 2.0).max(1.0);
        let w = self.w.min(max_w);
        let h = self.h.min(max_h);
        Self {
            x: self.x.clamp(margin, (width - margin - w).max(margin)),
            y: self.y.clamp(margin, (height - margin - h).max(margin)),
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
    let compact = width < 900.0 || height < 580.0;
    let geometry = scene_geometry(state, width, height, compact);

    let mut root = div().absolute().inset_0();

    if state.scene == OnboardingScene::ManageOverview && !compact {
        root = root.child(render_manage_demo_layer(width, height, &colors));
    }

    root = root.child(render_dim_layer(geometry.focus, compact, state.scene));

    if let Some(focus) = geometry.focus {
        root = root.child(render_spotlight(focus, &colors));
    }

    if let Some(callout) = geometry.callout
        && let Some(text) = scene_callout_text(state.scene)
    {
        root = root.child(render_callout(callout, text, &colors));
    }

    root = root.child(
        div()
            .absolute()
            .left(px(geometry.panel.x))
            .top(px(geometry.panel.y))
            .w(px(geometry.panel.w))
            .h(px(geometry.panel.h))
            .child(render_guide_panel(state, &colors, compact)),
    );

    root.into_any_element()
}

fn scene_geometry(
    state: &OnboardingTourState,
    width: f32,
    height: f32,
    compact: bool,
) -> SceneGeometry {
    if compact {
        return SceneGeometry {
            panel: RectF {
                x: 14.0,
                y: 74.0,
                w: (width - 28.0).max(320.0),
                h: (height - 92.0).max(320.0),
            }
            .clamp(width, height, 10.0),
            focus: None,
            callout: None,
        };
    }

    let panel_left = matches!(
        state.scene,
        OnboardingScene::GameDownload
            | OnboardingScene::ResourcePackDownload
            | OnboardingScene::ModDownload
            | OnboardingScene::ImportPackage
    );
    let panel_w = PANEL_WIDTH.min((width - PANEL_MARGIN * 2.0).max(320.0));
    let panel = RectF {
        x: if panel_left {
            PANEL_MARGIN
        } else {
            width - PANEL_MARGIN - panel_w
        },
        y: PANEL_TOP,
        w: panel_w,
        h: (height - PANEL_TOP - PANEL_MARGIN).max(330.0),
    }
    .clamp(width, height, PANEL_MARGIN);

    let focus = observed_focus(state, width, height).or_else(|| fallback_focus(state.scene, width, height));
    let callout = focus.and_then(|focus| scene_callout_bounds(state.scene, focus, panel, width, height));

    SceneGeometry {
        panel,
        focus,
        callout,
    }
}

fn observed_focus(state: &OnboardingTourState, width: f32, height: f32) -> Option<RectF> {
    let (anchor, padding) = match state.scene {
        OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => (OnboardingAnchor::DownloadToolbar, 3.0),
        OnboardingScene::ImportPackage => (OnboardingAnchor::DownloadImport, 4.0),
        OnboardingScene::ManageOverview => (OnboardingAnchor::VersionSidebar, 3.0),
        _ => return None,
    };
    let bounds = state.anchor(anchor)?;
    Some(
        RectF {
            x: bounds.origin.x / px(1.0),
            y: bounds.origin.y / px(1.0),
            w: bounds.size.width / px(1.0),
            h: bounds.size.height / px(1.0),
        }
        .padded(padding)
        .clamp(width, height, 6.0),
    )
}

fn fallback_focus(scene: OnboardingScene, width: f32, height: f32) -> Option<RectF> {
    let page_x = crate::ui::components::page_shell::PAGE_INSET_X / px(1.0);
    let page_y = crate::ui::components::page_shell::PAGE_INSET_TOP / px(1.0);
    let page_bottom = crate::ui::components::page_shell::PAGE_INSET_BOTTOM / px(1.0);
    let sidebar_w = crate::ui::components::page_shell::SPLIT_PAGE_SIDEBAR_WIDTH / px(1.0);

    match scene {
        OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => Some(
            RectF {
                x: page_x,
                y: page_y,
                w: (width - page_x * 2.0).max(240.0),
                h: 68.0,
            }
            .padded(3.0)
            .clamp(width, height, 6.0),
        ),
        OnboardingScene::ImportPackage => Some(
            RectF {
                x: width - page_x - 20.0 - 32.0 - 12.0 - 32.0,
                y: page_y + 14.0,
                w: 32.0,
                h: 32.0,
            }
            .padded(4.0)
            .clamp(width, height, 6.0),
        ),
        OnboardingScene::ManageOverview => Some(
            RectF {
                x: page_x,
                y: page_y,
                w: sidebar_w,
                h: (height - page_y - page_bottom).max(260.0),
            }
            .clamp(width, height, 6.0),
        ),
        _ => None,
    }
}

fn render_dim_layer(
    focus: Option<RectF>,
    compact: bool,
    scene: OnboardingScene,
) -> AnyElement {
    let alpha = if compact { 0.10 } else { 0.18 };
    if scene == OnboardingScene::ManageOverview {
        return div().absolute().inset_0().into_any_element();
    }

    let Some(focus) = focus else {
        return div()
            .absolute()
            .inset_0()
            .bg(Hsla { a: alpha, ..black() })
            .occlude()
            .into_any_element();
    };

    rounded_cutout(
        Bounds::new(
            point(px(focus.x), px(focus.y)),
            size(px(focus.w), px(focus.h)),
        ),
        px(crate::ui::theme::tokens::radius::MD),
        Hsla { a: alpha, ..black() },
    )
    .window_space()
    .absolute()
    .inset_0()
    .block_mouse()
    .into_any_element()
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
            a: 0.018,
            ..colors.accent
        })
}

fn scene_callout_bounds(
    scene: OnboardingScene,
    focus: RectF,
    panel: RectF,
    width: f32,
    height: f32,
) -> Option<RectF> {
    let preferred = match scene {
        OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => RectF {
            x: focus.right() - CALLOUT_WIDTH,
            y: focus.bottom() + CALLOUT_GAP,
            w: CALLOUT_WIDTH,
            h: 58.0,
        },
        OnboardingScene::ImportPackage => RectF {
            x: focus.x - CALLOUT_WIDTH - CALLOUT_GAP,
            y: focus.y,
            w: CALLOUT_WIDTH,
            h: 62.0,
        },
        _ => return None,
    };
    Some(place_callout(preferred, focus, panel, width, height))
}

fn place_callout(
    preferred: RectF,
    focus: RectF,
    panel: RectF,
    width: f32,
    height: f32,
) -> RectF {
    let candidate = preferred.clamp(width, height, 8.0);
    if !candidate.intersects(panel) && !candidate.intersects(focus) {
        return candidate;
    }

    for candidate in [
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
    ] {
        let candidate = candidate.clamp(width, height, 8.0);
        if !candidate.intersects(panel) && !candidate.intersects(focus) {
            return candidate;
        }
    }

    preferred.clamp(width, height, 8.0)
}

fn render_callout(bounds: RectF, text: &'static str, colors: &ThemeColors) -> Div {
    div()
        .absolute()
        .left(px(bounds.x))
        .top(px(bounds.y))
        .w(px(bounds.w))
        .min_h(px(42.0))
        .px(px(12.0))
        .py(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.62,
            ..colors.accent
        })
        .bg(colors.accent)
        .shadow_lg()
        .occlude()
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(
            svg()
                .path(lucide_icons::icon_map_pin())
                .size(px(15.0))
                .text_color(colors.btn_primary_text),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(11.0))
                .line_height(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.btn_primary_text)
                .child(text),
        )
}

fn scene_callout_text(scene: OnboardingScene) -> Option<&'static str> {
    match scene {
        OnboardingScene::GameDownload => Some(
            "游戏页：标签、搜索、加载器/版本通道筛选、导入和刷新都在这一行；列表里的按钮负责下载或安装。",
        ),
        OnboardingScene::ResourcePackDownload => Some(
            "资源包页：这里会切换到 CurseForge。可按游戏版本、分类、排序和关键字筛选，再选择本地版本安装。",
        ),
        OnboardingScene::ModDownload => Some(
            "模组页：面向 LeviLamina/LeviLauncher 生态。先确认加载器与游戏版本兼容，再选择目标实例安装。",
        ),
        OnboardingScene::ImportPackage => Some(
            "这是实际上传按钮。已有 APPX、ZIP 或 MSIXVC 时直接导入，不需要重新下载同一版本。",
        ),
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
        .min_h(px(0.0))
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
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .px(px(if compact { 18.0 } else { 22.0 }))
                .py(px(18.0))
                .child(render_scene_body(state, colors)),
        )
        .child(render_footer(state, colors))
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
                        .text_size(px(17.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
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
        OnboardingScene::GameDownload => render_game_download(colors),
        OnboardingScene::ResourcePackDownload => render_resource_download(colors),
        OnboardingScene::ModDownload => render_mod_download(colors),
        OnboardingScene::ImportPackage => render_import(colors),
        OnboardingScene::ManageOverview => render_manage_overview(colors),
        OnboardingScene::PlatformSetup => render_platform(state, colors),
        OnboardingScene::Finish => render_finish(colors),
    }
}

fn render_welcome(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(13.0))
        .child(intro(
            colors,
            "这次导览会进入真实页面，而不是只展示说明文字。下载游戏、CurseForge 资源包、模组、导入、版本管理和平台环境都会分别说明。",
        ))
        .child(feature(colors, lucide_icons::icon_download(), "游戏下载", "选择版本、通道、加载器并开始下载/安装。"))
        .child(feature(colors, lucide_icons::icon_package(), "CurseForge 资源", "搜索和筛选资源包，再安装到指定本地实例。"))
        .child(feature(colors, lucide_icons::icon_layers(), "模组", "识别 LeviLamina/加载器兼容关系和安装目标。"))
        .child(feature(colors, lucide_icons::icon_settings_2(), "版本管理", "用演示数据认识地图、资源、模组、截图、服务器和版本工具。"))
        .child(tip(colors, "导览中的“演示版本/演示数据”只用于 UI 说明，不会创建目录、写配置或启动任何任务。"))
        .into_any_element()
}

fn render_game_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(13.0))
        .child(route_badge(colors, "当前页面：下载 → 游戏"))
        .child(intro(colors, "游戏页负责 Minecraft Bedrock 本体。当前已经自动切到“游戏”标签。"))
        .child(step(colors, 1, "版本通道", "正式版用于日常游玩；Preview/测试版用于提前体验新内容。"))
        .child(step(colors, 2, "加载器筛选", "原版和 LeviLamina 可以分开筛选。安装加载器前先确认该游戏版本受支持。"))
        .child(step(colors, 3, "下载 / 安装", "列表右侧会根据本地状态显示下载、安装或已有版本。任务进度由统一任务系统管理。"))
        .child(tip(colors, "知道版本号时直接搜索；不确定时保持“正式版 + 原版”是最稳妥的选择。"))
        .into_any_element()
}

fn render_resource_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(13.0))
        .child(route_badge(colors, "当前页面：下载 → 资源包 / CurseForge"))
        .child(intro(colors, "资源包页使用 CurseForge 数据源。导览会保留真实分类栏和结果区域，网络慢时看到加载状态也是正常的。"))
        .child(step(colors, 1, "分类和搜索", "按材质、界面、光影等类别缩小范围，也可以直接搜索项目名。"))
        .child(step(colors, 2, "游戏版本与排序", "先选目标 Minecraft 版本，再按精选、热门、更新、名称或下载量排序。"))
        .child(step(colors, 3, "安装目标", "打开项目后选择具体文件和本地游戏实例；资源会写入对应版本的数据目录，而不是覆盖游戏本体。"))
        .child(tip(colors, "CurseForge 项目版本与 Minecraft 版本不匹配时，不建议强行安装。"))
        .into_any_element()
}

fn render_mod_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(13.0))
        .child(route_badge(colors, "当前页面：下载 → 模组"))
        .child(intro(colors, "模组页面向 BMCBL 支持的 Bedrock 客户端模组生态，不等同于 Java Edition 的 Forge/Fabric。"))
        .child(step(colors, 1, "加载器", "先选择 LeviLamina 等加载器类型，再选择加载器版本。"))
        .child(step(colors, 2, "兼容性", "游戏版本、加载器版本、模组版本三者必须匹配；导览不会自动替你忽略兼容检查。"))
        .child(step(colors, 3, "安装到实例", "确认目标本地版本后再安装，避免把模组放进错误版本目录。"))
        .child(tip(colors, "如果一个模组要求特定 LeviLamina 版本，优先满足模组声明，而不是只选最新加载器。"))
        .into_any_element()
}

fn render_import(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(13.0))
        .child(route_badge(colors, "当前页面：下载 → 上传按钮"))
        .child(intro(colors, "已经有游戏安装包时直接导入，不需要重复下载。高亮区域就是实际上传按钮。"))
        .child(format_card(colors, "APPX", "常见的 Minecraft UWP 安装包。"))
        .child(format_card(colors, "ZIP", "BMCBL 支持的游戏版本压缩包。"))
        .child(format_card(colors, "MSIXVC", "部分 GDK 版本使用的容器格式，会进入对应解包流程。"))
        .child(step(colors, 1, "点击上传", "选择受支持的本地文件。"))
        .child(step(colors, 2, "等待导入任务", "BMCBL 自动解析并整理到 versions，不需要手动复制文件。"))
        .into_any_element()
}

fn render_manage_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(13.0))
        .child(route_badge(colors, "当前页面：管理 · 当前显示引导演示数据"))
        .child(intro(colors, "即使第一次使用还没有真实版本，这一步也会在页面上投影一个“演示版本”，让每个管理入口都能看见。"))
        .child(step(colors, 1, "左侧版本列表", "真实使用时这里来自 BMCBL/versions。选择版本后，右侧所有操作都绑定该实例。"))
        .child(step(colors, 2, "顶部实例工具", "打开目录、创建快捷方式、版本设置、删除和启动都属于实例级操作。"))
        .child(step(colors, 3, "功能标签", "概览、模组、资源包、皮肤包、地图、截图、服务器分别管理不同数据。"))
        .child(step(colors, 4, "高级数据操作", "地图和 level.dat 编辑会修改真实存档；正式操作前应确认目标版本与备份。"))
        .child(tip(colors, "页面里的绿色“引导演示数据”区域完全是临时 UI，不会被保存到 ManagePageState。"))
        .into_any_element()
}

fn render_platform(state: &OnboardingTourState, colors: &ThemeColors) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(13.0));

    #[cfg(target_os = "windows")]
    {
        body = body
            .child(route_badge(colors, "Windows：UWP 注册与数据保护"))
            .child(intro(colors, "旧版 UWP 可以在多个 BMCBL 版本目录之间重新注册。Store/外部 UWP 切换前的数据保护由运行时安全门强制执行。"))
            .child(step(colors, 1, "BMCBL 散装 UWP", "使用 DevelopmentMode 指向对应版本目录，切换版本时重新注册目标目录。"))
            .child(step(colors, 2, "Store/外部 UWP 迁移", "发现 games/com.mojang 数据时先备份和校验；失败就阻止卸载。"))
            .child(step(colors, 3, "恢复条件", "注册成功后只有目标数据目录为空才恢复备份，避免覆盖新数据。"));
    }

    #[cfg(target_os = "linux")]
    {
        body = body
            .child(route_badge(colors, "Linux：Proton-GDK / UMU"))
            .child(intro(colors, "Linux 不执行 Windows UWP 注册，也不检查 Store UWP。需要确认的是兼容运行环境和系统依赖。"))
            .child(step(colors, 1, "Runner", "选择或安装可用 Proton-GDK/UMU runner。"))
            .child(step(colors, 2, "系统依赖", "缺少 32 位 glibc 等依赖时，Linux runtime 检测会给出原因。"));
    }

    if state.platform_scanning {
        body = body.child(status(colors, lucide_icons::icon_loader_circle(), "正在检查当前平台环境…", false));
    } else if let Some(error) = state.error.as_deref() {
        body = body.child(dynamic_status(colors, lucide_icons::icon_triangle_alert(), error, true));
    } else if let Some(summary) = &state.platform_summary {
        body = body.child(platform_summary(colors, summary));
    } else {
        body = body.child(tip(colors, "正在等待平台环境检测结果。"));
    }

    body.into_any_element()
}

fn render_finish(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(13.0))
        .child(
            div()
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
                        .child(svg().path(lucide_icons::icon_circle_check()).size(px(25.0)).text_color(colors.accent)),
                )
                .child(
                    div()
                        .text_size(px(18.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("首次使用的关键路径已经看完"),
                ),
        )
        .child(feature(colors, lucide_icons::icon_download(), "下一步：下载", "回到游戏下载页选择正式版。"))
        .child(feature(colors, lucide_icons::icon_settings_2(), "下一步：管理", "已经有版本时直接进入版本管理。"))
        .child(tip(colors, "以后可以从“设置 → 关于 → 首次运行设置向导”重新打开，不会重置版本或游戏数据。"))
        .into_any_element()
}

fn render_footer(state: &OnboardingTourState, colors: &ThemeColors) -> Div {
    let scene = state.scene;
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
        OnboardingScene::Welcome => (lucide_icons::icon_route(), "欢迎使用 BMCBL", "先认识真实工作流，再开始操作。"),
        OnboardingScene::GameDownload => (lucide_icons::icon_download(), "下载 Minecraft", "游戏本体、版本通道与加载器。"),
        OnboardingScene::ResourcePackDownload => (lucide_icons::icon_package(), "CurseForge 资源包", "分类、搜索、版本筛选与安装目标。"),
        OnboardingScene::ModDownload => (lucide_icons::icon_layers(), "客户端模组", "加载器兼容关系与目标实例。"),
        OnboardingScene::ImportPackage => (lucide_icons::icon_upload(), "导入本地安装包", "APPX、ZIP、MSIXVC 不需要重复下载。"),
        OnboardingScene::ManageOverview => (lucide_icons::icon_settings_2(), "版本管理功能", "使用临时演示数据认识每个入口。"),
        OnboardingScene::PlatformSetup => {
            #[cfg(target_os = "windows")]
            {
                (lucide_icons::icon_shield_check(), "Windows UWP 数据保护", "检查注册来源和需要保护的数据。")
            }
            #[cfg(target_os = "linux")]
            {
                (lucide_icons::icon_settings_2(), "Linux 运行环境", "检查 Proton-GDK / UMU 与依赖。")
            }
        }
        OnboardingScene::Finish => (lucide_icons::icon_circle_check(), "导览完成", "现在可以开始下载或管理真实版本。"),
    }
}

fn render_manage_demo_layer(width: f32, height: f32, colors: &ThemeColors) -> AnyElement {
    let page_x = crate::ui::components::page_shell::PAGE_INSET_X / px(1.0);
    let page_y = crate::ui::components::page_shell::PAGE_INSET_TOP / px(1.0);
    let page_bottom = crate::ui::components::page_shell::PAGE_INSET_BOTTOM / px(1.0);
    let sidebar_w = crate::ui::components::page_shell::SPLIT_PAGE_SIDEBAR_WIDTH / px(1.0);
    let full_h = (height - page_y - page_bottom).max(300.0);
    let gap = 12.0;
    let content_x = page_x + sidebar_w + gap;
    let content_w = (width - content_x - page_x).max(360.0);

    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .absolute()
                .left(px(page_x))
                .top(px(page_y))
                .w(px(sidebar_w))
                .h(px(full_h))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .border_2()
                .border_color(Hsla { a: 0.55, ..colors.accent })
                .bg(Hsla { a: 0.97, ..colors.bg })
                .shadow_lg()
                .p(px(12.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(demo_badge(colors, "引导演示数据"))
                .child(demo_version(colors, "演示版本 1.21.100", "UWP · 正式版", true))
                .child(demo_version(colors, "演示 Preview", "GDK · Preview", false))
                .child(tip(colors, "真实版本会从 BMCBL/versions 自动读取。")),
        )
        .child(
            div()
                .absolute()
                .left(px(content_x))
                .top(px(page_y))
                .w(px(content_w))
                .h(px(full_h))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .border_2()
                .border_color(Hsla { a: 0.55, ..colors.accent })
                .bg(Hsla { a: 0.97, ..colors.bg })
                .shadow_lg()
                .p(px(14.0))
                .flex()
                .flex_col()
                .gap(px(11.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(div().text_size(px(16.0)).font_weight(FontWeight::BOLD).text_color(colors.text_primary).child("演示版本 1.21.100"))
                                .child(div().text_size(px(11.0)).text_color(colors.text_secondary).child("以下按钮和项目只是说明，不会执行真实操作")),
                        )
                        .child(demo_badge(colors, "不会写入磁盘")),
                )
                .child(render_demo_tool_row(colors))
                .child(render_demo_tabs(colors))
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scrollbar()
                        .child(render_demo_feature_grid(colors)),
                ),
        )
        .into_any_element()
}

fn demo_version(colors: &ThemeColors, name: &'static str, detail: &'static str, selected: bool) -> Div {
    div()
        .w_full()
        .p(px(11.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(if selected { Hsla { a: 0.48, ..colors.accent } } else { Hsla { a: 0.30, ..colors.border } })
        .bg(if selected { Hsla { a: 0.08, ..colors.accent } } else { Hsla { a: 0.65, ..colors.surface } })
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(name))
        .child(div().text_size(px(10.0)).text_color(colors.text_secondary).child(detail))
}

fn render_demo_tool_row(colors: &ThemeColors) -> Div {
    let tools = [
        (lucide_icons::icon_folder_open(), "目录"),
        (lucide_icons::icon_external_link(), "快捷方式"),
        (lucide_icons::icon_settings(), "版本设置"),
        (lucide_icons::icon_trash_2(), "删除"),
        (lucide_icons::icon_play(), "启动"),
    ];
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .children(tools.into_iter().map(|(icon, label)| {
            div()
                .px(px(9.0))
                .py(px(7.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.70, ..colors.surface })
                .border_1()
                .border_color(Hsla { a: 0.28, ..colors.border })
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(svg().path(icon).size(px(13.0)).text_color(colors.text_secondary))
                .child(div().text_size(px(10.0)).text_color(colors.text_secondary).child(label))
        }))
}

fn render_demo_tabs(colors: &ThemeColors) -> Div {
    let tabs = ["概览", "模组", "资源包", "皮肤包", "地图", "截图", "服务器"];
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .children(tabs.into_iter().enumerate().map(|(index, label)| {
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(if index == 0 { Hsla { a: 0.12, ..colors.accent } } else { Hsla { a: 0.60, ..colors.surface } })
                .text_size(px(10.0))
                .font_weight(if index == 0 { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(if index == 0 { colors.accent } else { colors.text_secondary })
                .child(label)
        }))
}

fn render_demo_feature_grid(colors: &ThemeColors) -> Div {
    let features = [
        (lucide_icons::icon_chart_no_axes_combined(), "概览", "版本信息、数据位置和实例状态。"),
        (lucide_icons::icon_blocks(), "模组", "启用、禁用、导入和查看客户端模组。"),
        (lucide_icons::icon_package(), "资源包", "管理资源包/行为包以及排序和导入。"),
        (lucide_icons::icon_user_round(), "皮肤包", "预览和管理皮肤包内容。"),
        (lucide_icons::icon_map(), "地图", "导入、导出、查看世界，并可进入 level.dat/地图工具。"),
        (lucide_icons::icon_image(), "截图", "按时间查看并打开游戏截图。"),
        (lucide_icons::icon_server(), "服务器", "管理 servers.dat 项目并查看 MOTD/延迟。"),
        (lucide_icons::icon_settings(), "版本设置", "隔离、重定向、调试、鼠标和启动行为。"),
    ];

    div()
        .w_full()
        .flex()
        .flex_wrap()
        .gap(px(9.0))
        .children(features.into_iter().map(|(icon, title, detail)| {
            div()
                .w(px(210.0))
                .min_h(px(82.0))
                .p(px(10.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .border_1()
                .border_color(Hsla { a: 0.28, ..colors.border })
                .bg(Hsla { a: 0.65, ..colors.surface })
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(svg().path(icon).size(px(16.0)).text_color(colors.accent))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(title))
                        .child(div().text_size(px(10.0)).line_height(px(15.0)).text_color(colors.text_secondary).child(detail)),
                )
        }))
}

fn demo_badge(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(crate::ui::theme::tokens::radius::FULL))
        .bg(Hsla { a: 0.12, ..colors.accent })
        .text_size(px(9.0))
        .font_weight(FontWeight::BOLD)
        .text_color(colors.accent)
        .child(label)
}

fn feature(colors: &ThemeColors, icon: &'static str, title: &'static str, detail: &'static str) -> Div {
    div()
        .w_full()
        .p(px(12.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.34, ..colors.border })
        .bg(Hsla { a: 0.62, ..colors.surface })
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(svg().path(icon).size(px(17.0)).text_color(colors.accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(title))
                .child(div().text_size(px(11.0)).line_height(px(17.0)).text_color(colors.text_secondary).child(detail)),
        )
}

fn step(colors: &ThemeColors, number: usize, title: &'static str, detail: &'static str) -> Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(
            div()
                .flex_none()
                .w(px(25.0))
                .h(px(25.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla { a: 0.13, ..colors.accent })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.0))
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
                .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(title))
                .child(div().text_size(px(11.0)).line_height(px(17.0)).text_color(colors.text_secondary).child(detail)),
        )
}

fn format_card(colors: &ThemeColors, format: &'static str, detail: &'static str) -> Div {
    div()
        .w_full()
        .p(px(10.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla { a: 0.06, ..colors.accent })
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .min_w(px(70.0))
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.12, ..colors.accent })
                .text_center()
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(format),
        )
        .child(div().flex_1().text_size(px(11.0)).text_color(colors.text_secondary).child(detail))
}

fn intro(colors: &ThemeColors, text: &'static str) -> Div {
    div()
        .w_full()
        .text_size(px(12.0))
        .line_height(px(19.0))
        .text_color(colors.text_secondary)
        .child(text)
}

fn route_badge(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .w_full()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla { a: 0.09, ..colors.accent })
        .flex()
        .items_center()
        .gap(px(7.0))
        .child(svg().path(lucide_icons::icon_map_pin()).size(px(14.0)).text_color(colors.accent))
        .child(div().flex_1().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.accent).child(label))
}

fn tip(colors: &ThemeColors, text: &'static str) -> Div {
    status(colors, lucide_icons::icon_info(), text, false)
}

fn status(colors: &ThemeColors, icon: &'static str, text: &'static str, danger: bool) -> Div {
    dynamic_status(colors, icon, text, danger)
}

fn dynamic_status(colors: &ThemeColors, icon: &'static str, text: &str, danger: bool) -> Div {
    let color = if danger { colors.danger } else { colors.text_secondary };
    div()
        .w_full()
        .p(px(10.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla { a: 0.07, ..color })
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(svg().path(icon).size(px(14.0)).text_color(color))
        .child(div().flex_1().min_w(px(0.0)).text_size(px(11.0)).line_height(px(17.0)).text_color(colors.text_secondary).child(text.to_string()))
}

fn platform_summary(
    colors: &ThemeColors,
    summary: &crate::ui::onboarding::state::OnboardingPlatformSummary,
) -> Div {
    let mut items = div().w_full().flex().flex_col().gap(px(8.0));
    for item in &summary.items {
        let color = if item.warning { colors.danger } else { colors.accent };
        let icon = if item.warning { lucide_icons::icon_triangle_alert() } else { lucide_icons::icon_circle_check() };
        items = items.child(
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(svg().path(icon).size(px(14.0)).text_color(color))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(item.label.clone()))
                        .child(div().text_size(px(10.0)).line_height(px(16.0)).text_color(colors.text_secondary).child(item.value.clone())),
                ),
        );
    }

    div()
        .w_full()
        .p(px(12.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.34, ..colors.border })
        .bg(Hsla { a: 0.68, ..colors.surface })
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(div().text_size(px(12.0)).font_weight(FontWeight::BOLD).text_color(colors.text_primary).child(summary.title.clone()))
        .child(div().text_size(px(10.0)).line_height(px(16.0)).text_color(colors.text_secondary).child(summary.detail.clone()))
        .child(items)
}

fn primary_button(colors: &ThemeColors, label: &'static str, enabled: bool) -> Stateful<Div> {
    let mut button = div()
        .id(SharedString::from(format!("onboarding-guided-primary-{label}")))
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
        .id(SharedString::from(format!("onboarding-guided-secondary-{label}")))
        .min_h(px(38.0))
        .px(px(14.0))
        .py(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.44, ..colors.border })
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
