use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::state::{OnboardingAnchor, OnboardingScene, OnboardingTourState};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

const PANEL_WIDTH: f32 = 334.0;
const PANEL_HEIGHT: f32 = 334.0;
const PANEL_MARGIN: f32 = 22.0;
const PAGE_TOP: f32 = 72.0;
const CALLOUT_WIDTH: f32 = 300.0;
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
    let compact = width < 900.0 || height < 620.0;
    let geometry = scene_geometry(state, width, height, compact);

    let mut root = div().absolute().inset_0();

    match state.scene {
        OnboardingScene::TasksOverview if !compact => {
            root = root.child(render_tasks_demo_layer(width, height, &colors));
        }
        OnboardingScene::ManageOverview | OnboardingScene::ManageContent if !compact => {
            root = root.child(render_manage_demo_layer(state.scene, width, height, &colors));
        }
        _ => {}
    }

    root = root.child(render_dim_layer(geometry.focus, compact, state.scene));

    if let Some(focus) = geometry.focus {
        root = root.child(render_spotlight(focus, &colors));
    }

    if let Some(callout) = geometry.callout {
        if let Some(text) = scene_callout_text(state.scene) {
            root = root.child(render_callout(callout, text, &colors));
        }
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
                x: 12.0,
                y: (height - 360.0).max(78.0),
                w: (width - 24.0).max(320.0),
                h: (height - 92.0).min(350.0).max(300.0),
            }
            .clamp(width, height, 10.0),
            focus: None,
            callout: None,
        };
    }

    let focus = observed_focus(state, width, height)
        .or_else(|| fallback_focus(state.scene, width, height));
    let panel = panel_bounds(state.scene, focus, width, height);
    let callout = focus.and_then(|focus| {
        scene_callout_bounds(state.scene, focus, panel, width, height)
    });

    SceneGeometry {
        panel,
        focus,
        callout,
    }
}

fn panel_bounds(scene: OnboardingScene, focus: Option<RectF>, width: f32, height: f32) -> RectF {
    let panel_w = PANEL_WIDTH.min((width - PANEL_MARGIN * 2.0).max(300.0));
    let panel_h = match scene {
        OnboardingScene::Welcome | OnboardingScene::Finish => 390.0,
        OnboardingScene::PlatformSetup => 370.0,
        _ => PANEL_HEIGHT,
    }
    .min((height - PAGE_TOP - PANEL_MARGIN).max(290.0));

    let bottom = height - PANEL_MARGIN - panel_h;
    let left = PANEL_MARGIN;
    let right = width - PANEL_MARGIN - panel_w;

    let preferred = match scene {
        OnboardingScene::Welcome | OnboardingScene::Finish => RectF {
            x: (width - panel_w) * 0.5,
            y: (height - panel_h) * 0.5,
            w: panel_w,
            h: panel_h,
        },
        OnboardingScene::DownloadNavigation
        | OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload
        | OnboardingScene::ImportPackage => RectF {
            x: left,
            y: bottom,
            w: panel_w,
            h: panel_h,
        },
        OnboardingScene::TasksOverview | OnboardingScene::ManageOverview => RectF {
            x: right,
            y: bottom,
            w: panel_w,
            h: panel_h,
        },
        OnboardingScene::ManageContent => RectF {
            x: left,
            y: bottom,
            w: panel_w,
            h: panel_h,
        },
        OnboardingScene::SettingsOverview
        | OnboardingScene::ToolsOverview
        | OnboardingScene::PlatformSetup => RectF {
            x: right,
            y: bottom,
            w: panel_w,
            h: panel_h,
        },
    };

    let preferred = preferred.clamp(width, height, PANEL_MARGIN);
    if focus.is_some_and(|focus| preferred.intersects(focus)) {
        let alternate = RectF {
            x: if preferred.x < width * 0.5 { right } else { left },
            ..preferred
        }
        .clamp(width, height, PANEL_MARGIN);
        if !focus.is_some_and(|focus| alternate.intersects(focus)) {
            return alternate;
        }
    }
    preferred
}

fn observed_focus(state: &OnboardingTourState, width: f32, height: f32) -> Option<RectF> {
    let (anchor, padding) = match state.scene {
        OnboardingScene::DownloadNavigation => (OnboardingAnchor::DownloadTabs, 4.0),
        OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => (OnboardingAnchor::DownloadToolbar, 3.0),
        OnboardingScene::ImportPackage => (OnboardingAnchor::DownloadImport, 5.0),
        OnboardingScene::TasksOverview => (OnboardingAnchor::TasksPage, 0.0),
        OnboardingScene::SettingsOverview | OnboardingScene::PlatformSetup => {
            (OnboardingAnchor::SettingsTabs, 4.0)
        }
        OnboardingScene::ToolsOverview => (OnboardingAnchor::ToolsSidebar, 4.0),
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
        OnboardingScene::DownloadNavigation => Some(RectF {
            x: page_x + 20.0,
            y: page_y + 14.0,
            w: 320.0,
            h: 40.0,
        }),
        OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => Some(
            RectF {
                x: page_x,
                y: page_y,
                w: (width - page_x * 2.0).max(240.0),
                h: 68.0,
            }
            .padded(3.0),
        ),
        OnboardingScene::ImportPackage => Some(
            RectF {
                x: width - page_x - 96.0,
                y: page_y + 14.0,
                w: 32.0,
                h: 32.0,
            }
            .padded(5.0),
        ),
        OnboardingScene::TasksOverview => Some(RectF {
            x: page_x,
            y: page_y,
            w: width - page_x * 2.0,
            h: height - page_y - page_bottom,
        }),
        OnboardingScene::ManageOverview => Some(RectF {
            x: page_x,
            y: page_y,
            w: sidebar_w,
            h: height - page_y - page_bottom,
        }),
        OnboardingScene::ManageContent => Some(RectF {
            x: page_x + sidebar_w + 12.0,
            y: page_y,
            w: width - page_x * 2.0 - sidebar_w - 12.0,
            h: height - page_y - page_bottom,
        }),
        OnboardingScene::SettingsOverview | OnboardingScene::PlatformSetup => Some(RectF {
            x: page_x + 12.0,
            y: page_y + 12.0,
            w: width - page_x * 2.0 - 24.0,
            h: 54.0,
        }),
        OnboardingScene::ToolsOverview => Some(RectF {
            x: page_x,
            y: page_y,
            w: sidebar_w,
            h: height - page_y - page_bottom,
        }),
        _ => None,
    }
    .map(|bounds| bounds.clamp(width, height, 6.0))
}

fn render_dim_layer(
    focus: Option<RectF>,
    compact: bool,
    scene: OnboardingScene,
) -> AnyElement {
    if matches!(
        scene,
        OnboardingScene::TasksOverview
            | OnboardingScene::ManageOverview
            | OnboardingScene::ManageContent
    ) {
        return div().absolute().inset_0().into_any_element();
    }

    let alpha = if compact { 0.08 } else { 0.14 };
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
            a: 0.014,
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
        OnboardingScene::DownloadNavigation
        | OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => RectF {
            x: focus.right() - CALLOUT_WIDTH,
            y: focus.bottom() + CALLOUT_GAP,
            w: CALLOUT_WIDTH,
            h: 52.0,
        },
        OnboardingScene::ImportPackage => RectF {
            x: focus.x - CALLOUT_WIDTH - CALLOUT_GAP,
            y: focus.y,
            w: CALLOUT_WIDTH,
            h: 54.0,
        },
        OnboardingScene::SettingsOverview | OnboardingScene::ToolsOverview => RectF {
            x: focus.x,
            y: focus.bottom() + CALLOUT_GAP,
            w: CALLOUT_WIDTH,
            h: 52.0,
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
        .px(px(11.0))
        .py(px(8.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.56,
            ..colors.accent
        })
        .bg(colors.accent)
        .shadow_lg()
        .occlude()
        .flex()
        .items_start()
        .gap(px(7.0))
        .child(
            svg()
                .path(lucide_icons::icon_map_pin())
                .size(px(14.0))
                .text_color(colors.btn_primary_text),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(10.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.btn_primary_text)
                .child(text),
        )
}

fn scene_callout_text(scene: OnboardingScene) -> Option<&'static str> {
    match scene {
        OnboardingScene::DownloadNavigation => {
            Some("先记住这三个入口：游戏本体、CurseForge 资源包、客户端模组。")
        }
        OnboardingScene::GameDownload => {
            Some("这里负责搜索、版本通道和加载器筛选；真正下载按钮在下面的版本列表。")
        }
        OnboardingScene::ResourcePackDownload => {
            Some("这里会切到 CurseForge，可按版本、分类、排序和关键字缩小结果。")
        }
        OnboardingScene::ModDownload => {
            Some("模组页先确认加载器和游戏版本，再选择目标本地实例。")
        }
        OnboardingScene::ImportPackage => {
            Some("已有 APPX、ZIP、MSIXVC 就点这里导入，不必重新下载。")
        }
        OnboardingScene::SettingsOverview => {
            Some("设置按游戏、启动器、外观、插件和关于分组；这里随时可以回来调整。")
        }
        OnboardingScene::ToolsOverview => {
            Some("工具页当前包含联机大厅；这里不是日常启动必经步骤，需要时再用。")
        }
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
            a: 0.54,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.982,
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
                .px(px(if compact { 15.0 } else { 17.0 }))
                .py(px(13.0))
                .child(render_scene_body(state, colors)),
        )
        .child(render_footer(state, colors))
}

fn render_header(state: &OnboardingTourState, colors: &ThemeColors) -> Div {
    let (icon, title, subtitle) = scene_header(state.scene);
    div()
        .px(px(17.0))
        .pt(px(15.0))
        .pb(px(12.0))
        .border_b_1()
        .border_color(Hsla {
            a: 0.30,
            ..colors.border
        })
        .flex()
        .items_start()
        .gap(px(10.0))
        .child(
            div()
                .flex_none()
                .w(px(38.0))
                .h(px(38.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.14,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(18.0)).text_color(colors.accent)),
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
                        .text_size(px(15.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .line_height(px(16.0))
                        .text_color(colors.text_secondary)
                        .child(subtitle),
                ),
        )
        .child(
            div()
                .flex_none()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.accent
                })
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.accent)
                .child(format!("{} / {}", state.scene.index(), OnboardingScene::COUNT)),
        )
}

fn render_scene_body(state: &OnboardingTourState, colors: &ThemeColors) -> AnyElement {
    match state.scene {
        OnboardingScene::Welcome => render_welcome(colors),
        OnboardingScene::DownloadNavigation => render_download_navigation(colors),
        OnboardingScene::GameDownload => render_game_download(colors),
        OnboardingScene::ResourcePackDownload => render_resource_download(colors),
        OnboardingScene::ModDownload => render_mod_download(colors),
        OnboardingScene::ImportPackage => render_import(colors),
        OnboardingScene::TasksOverview => render_tasks_overview(colors),
        OnboardingScene::ManageOverview => render_manage_overview(colors),
        OnboardingScene::ManageContent => render_manage_content(colors),
        OnboardingScene::SettingsOverview => render_settings_overview(colors),
        OnboardingScene::ToolsOverview => render_tools_overview(colors),
        OnboardingScene::PlatformSetup => render_platform(state, colors),
        OnboardingScene::Finish => render_finish(colors),
    }
}

fn render_welcome(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(intro(
            colors,
            "不需要先记住所有功能。接下来只带你走一遍“从下载安装到能管理”的真实路径。",
        ))
        .child(feature(colors, lucide_icons::icon_download(), "先获得游戏", "下载或导入 Minecraft，再去任务页看进度。"))
        .child(feature(colors, lucide_icons::icon_settings_2(), "再管理实例", "版本、模组、资源包、地图和服务器都按实例管理。"))
        .child(feature(colors, lucide_icons::icon_wrench(), "最后认识设置与工具", "这些不是第一次启动必须配置，知道入口即可。"))
        .child(tip(colors, "演示数据只存在于导览 UI，不会创建版本、任务或修改磁盘。"))
        .into_any_element()
}

fn render_download_navigation(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "下载页：先认三个标签"))
        .child(feature(colors, lucide_icons::icon_box(), "游戏", "Minecraft Bedrock 本体和不同版本。"))
        .child(feature(colors, lucide_icons::icon_package(), "资源包", "CurseForge 资源内容，安装到指定实例。"))
        .child(feature(colors, lucide_icons::icon_layers(), "模组", "LeviLamina / LeviLauncher 客户端模组生态。"))
        .child(tip(colors, "下一步开始逐个进入这些真实页面。"))
        .into_any_element()
}

fn render_game_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "下载 → 游戏"))
        .child(step(colors, 1, "先筛选", "正式版适合日常使用；知道版本号时直接搜索最快。"))
        .child(step(colors, 2, "再看加载器", "原版最简单；需要 LeviLamina 时再选择对应加载器。"))
        .child(step(colors, 3, "最后点列表按钮", "下载、安装、已有版本状态都显示在每个版本右侧。"))
        .child(tip(colors, "不确定选什么：正式版 + 原版。"))
        .into_any_element()
}

fn render_resource_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "下载 → 资源包 / CurseForge"))
        .child(step(colors, 1, "找项目", "用分类和搜索缩小范围。"))
        .child(step(colors, 2, "确认兼容版本", "先选 Minecraft 版本，再看具体文件。"))
        .child(step(colors, 3, "选择安装目标", "资源最终写入你指定的本地游戏实例，不会覆盖游戏本体。"))
        .child(tip(colors, "版本不匹配时不要强行安装。"))
        .into_any_element()
}

fn render_mod_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "下载 → 模组"))
        .child(step(colors, 1, "加载器", "先确定 LeviLamina 等加载器类型与版本。"))
        .child(step(colors, 2, "兼容关系", "游戏版本、加载器版本、模组版本必须匹配。"))
        .child(step(colors, 3, "目标实例", "安装前确认目标版本，避免把模组放错目录。"))
        .child(tip(colors, "这里是 Bedrock 客户端模组，不是 Java Forge/Fabric。"))
        .into_any_element()
}

fn render_import(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "下载 → 右上角上传按钮"))
        .child(format_card(colors, "APPX", "常见 UWP 安装包。"))
        .child(format_card(colors, "ZIP", "BMCBL 支持的游戏版本压缩包。"))
        .child(format_card(colors, "MSIXVC", "部分 GDK 版本使用的容器格式。"))
        .child(tip(colors, "选择文件后交给任务系统处理，不需要手动复制到 versions。"))
        .into_any_element()
}

fn render_tasks_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "任务页：下载之后看这里"))
        .child(step(colors, 1, "进度", "下载量、百分比、速度、线程和 ETA 会集中显示。"))
        .child(step(colors, 2, "控制", "支持的任务可以暂停或取消；完成项可以移除。"))
        .child(step(colors, 3, "错误", "导入或安装失败时，错误摘要也会留在这里。"))
        .child(tip(colors, "页面中的 4 条任务都是演示数据，不会真正占用带宽。"))
        .into_any_element()
}

fn render_manage_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "管理：先选一个版本"))
        .child(step(colors, 1, "左边选实例", "真实使用时来自 BMCBL/versions；不同版本互相独立。"))
        .child(step(colors, 2, "顶部是实例级操作", "打开目录、快捷方式、版本设置、删除和启动都只作用于当前版本。"))
        .child(tip(colors, "这里显示的是只读演示版本，不会执行任何真实操作。"))
        .into_any_element()
}

fn render_manage_content(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "管理：再管理这个版本里的内容"))
        .child(step(colors, 1, "功能标签", "统计、模组、资源包、皮肤、地图、截图、服务器分别管理不同数据。"))
        .child(step(colors, 2, "内容操作", "搜索、启用/禁用、导入、删除等操作都以当前实例为边界。"))
        .child(step(colors, 3, "存档操作要谨慎", "地图和 level.dat 修改的是实际世界，正式操作前应保留备份。"))
        .child(tip(colors, "演示页当前模拟“资源包”列表，让结构更接近真实管理界面。"))
        .into_any_element()
}

fn render_settings_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "设置：不是第一次使用必须全部修改"))
        .child(step(colors, 1, "游戏", "启动后的启动器行为和少量游戏/UWP 选项。"))
        .child(step(colors, 2, "启动器", "下载线程、代理、CurseForge API、日志、更新和渲染设置。"))
        .child(step(colors, 3, "外观 / 插件 / 关于", "主题背景、WASM 插件、版本信息和重新打开本导览。"))
        .child(tip(colors, "遇到下载慢或网络问题时，优先回到“启动器”设置检查代理和下载配置。"))
        .into_any_element()
}

fn render_tools_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(route_badge(colors, "工具：按需使用，不是启动必经步骤"))
        .child(step(colors, 1, "联机大厅", "通过 EasyTier 创建或加入房间，让不同网络中的玩家互联。"))
        .child(step(colors, 2, "网络状态", "NAT、节点、玩家和房间状态会在真实页面持续更新。"))
        .child(step(colors, 3, "遇到联机问题", "先看页面状态和阻塞原因，再调整 P2P / bootstrap 等高级设置。"))
        .child(tip(colors, "以后新增工具也会从左侧列表进入。"))
        .into_any_element()
}

fn render_platform(state: &OnboardingTourState, colors: &ThemeColors) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(10.0));

    #[cfg(target_os = "windows")]
    {
        body = body
            .child(route_badge(colors, "Windows：UWP 注册与数据安全"))
            .child(step(colors, 1, "切换旧版 UWP", "BMCBL 会把散装 DevelopmentMode 注册重新指向目标版本目录。"))
            .child(step(colors, 2, "遇到 Store/外部注册", "先备份并校验 games/com.mojang；失败就停止替换。"));
    }

    #[cfg(target_os = "linux")]
    {
        body = body
            .child(route_badge(colors, "Linux：Proton-GDK / UMU"))
            .child(step(colors, 1, "Linux 不做 UWP", "不会检查 Microsoft Store 注册，也不会执行 Windows UWP 数据迁移。"))
            .child(step(colors, 2, "需要的是兼容运行环境", "确认 Proton-GDK / UMU runner 和系统依赖即可。"));
    }

    if state.platform_scanning {
        body = body.child(status(colors, lucide_icons::icon_loader_circle(), "正在检测当前电脑…", false));
    } else if let Some(error) = state.error.as_deref() {
        body = body.child(dynamic_status(colors, lucide_icons::icon_triangle_alert(), error, true));
    } else if let Some(summary) = &state.platform_summary {
        body = body.child(platform_summary(colors, summary));
    } else {
        body = body.child(tip(colors, "等待环境检测结果。"));
    }

    body.into_any_element()
}

fn render_finish(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(11.0))
        .child(
            div()
                .py(px(10.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(7.0))
                .child(
                    div()
                        .w(px(48.0))
                        .h(px(48.0))
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
                                .size(px(23.0))
                                .text_color(colors.accent),
                        ),
                )
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("现在知道下一步去哪了"),
                ),
        )
        .child(feature(colors, lucide_icons::icon_download(), "没有游戏", "去下载页选正式版，或导入已有安装包。"))
        .child(feature(colors, lucide_icons::icon_settings_2(), "已经有版本", "去管理页选择实例并启动或管理内容。"))
        .child(tip(colors, "以后可在“设置 → 关于”重新打开导览，不会重置任何数据。"))
        .into_any_element()
}

fn render_footer(state: &OnboardingTourState, colors: &ThemeColors) -> Div {
    let scene = state.scene;
    let left_label = if scene == OnboardingScene::Welcome {
        "跳过"
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
        .px(px(16.0))
        .py(px(11.0))
        .border_t_1()
        .border_color(Hsla {
            a: 0.30,
            ..colors.border
        })
        .flex()
        .items_center()
        .justify_between()
        .gap(px(9.0))
        .child(left)
        .child(next)
}

fn scene_header(scene: OnboardingScene) -> (&'static str, &'static str, &'static str) {
    match scene {
        OnboardingScene::Welcome => (lucide_icons::icon_route(), "欢迎使用 BMCBL", "只学习第一次真正会用到的路径。"),
        OnboardingScene::DownloadNavigation => (lucide_icons::icon_download(), "先认识下载页", "游戏、资源包、模组是三个不同入口。"),
        OnboardingScene::GameDownload => (lucide_icons::icon_box(), "下载 Minecraft", "筛选版本，然后从列表开始任务。"),
        OnboardingScene::ResourcePackDownload => (lucide_icons::icon_package(), "CurseForge 资源包", "找项目、确认版本、选择安装目标。"),
        OnboardingScene::ModDownload => (lucide_icons::icon_layers(), "客户端模组", "先看兼容关系，再安装到实例。"),
        OnboardingScene::ImportPackage => (lucide_icons::icon_upload(), "导入已有安装包", "已有文件就不需要重新下载。"),
        OnboardingScene::TasksOverview => (lucide_icons::icon_activity(), "任务去哪里看？", "下载、安装、导入和错误都集中在这里。"),
        OnboardingScene::ManageOverview => (lucide_icons::icon_settings_2(), "管理一个版本", "先选实例，再做启动和实例级操作。"),
        OnboardingScene::ManageContent => (lucide_icons::icon_package(), "管理实例内容", "模组、资源包、地图等都属于当前版本。"),
        OnboardingScene::SettingsOverview => (lucide_icons::icon_settings(), "设置在哪里？", "大多数选项保持默认即可，需要时再回来。"),
        OnboardingScene::ToolsOverview => (lucide_icons::icon_wrench(), "工具在哪里？", "高级能力集中在这里，当前主要是联机大厅。"),
        OnboardingScene::PlatformSetup => {
            #[cfg(target_os = "windows")]
            {
                (lucide_icons::icon_shield_check(), "Windows 数据安全", "切换 UWP 前先保护现有世界数据。")
            }
            #[cfg(target_os = "linux")]
            {
                (lucide_icons::icon_box(), "Linux 运行环境", "确认 Proton-GDK / UMU，而不是 Windows UWP。")
            }
        }
        OnboardingScene::Finish => (lucide_icons::icon_circle_check(), "导览完成", "现在可以按自己的情况开始。"),
    }
}

fn render_tasks_demo_layer(width: f32, height: f32, colors: &ThemeColors) -> AnyElement {
    let page_x = crate::ui::components::page_shell::PAGE_INSET_X / px(1.0);
    let page_y = crate::ui::components::page_shell::PAGE_INSET_TOP / px(1.0);
    let page_bottom = crate::ui::components::page_shell::PAGE_INSET_BOTTOM / px(1.0);
    let page_w = (width - page_x * 2.0).max(560.0);
    let page_h = (height - page_y - page_bottom).max(420.0);

    div()
        .absolute()
        .inset_0()
        .occlude()
        .child(
            div()
                .absolute()
                .left(px(page_x))
                .top(px(page_y))
                .w(px(page_w))
                .h(px(page_h))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .border_1()
                .border_color(Hsla { a: 0.30, ..colors.border })
                .bg(Hsla { a: 0.98, ..colors.bg })
                .shadow_lg()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(22.0))
                        .py(px(14.0))
                        .border_b_1()
                        .border_color(Hsla { a: 0.18, ..colors.border })
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_size(px(18.0)).font_weight(FontWeight::BOLD).text_color(colors.text_primary).child("任务管理器"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(14.0))
                                .child(demo_badge(colors, "活动任务 2"))
                                .child(demo_badge(colors, "总线程 12")),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .p(px(16.0))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(demo_task_card(colors, lucide_icons::icon_download(), "Minecraft 1.21.100", "下载游戏包", "68%", 0.68, "18.4 MB/s · 12 线程 · ETA 00:42", false, false))
                        .child(demo_task_card(colors, lucide_icons::icon_package(), "Faithful 32x Bedrock", "安装资源包", "安装中", 0.88, "正在写入目标实例的 resource_packs", false, false))
                        .child(demo_task_card(colors, lucide_icons::icon_layers(), "LeviLamina 模组依赖", "安装完成", "完成", 1.0, "已安装到演示版本 1.21.100", false, true))
                        .child(demo_task_card(colors, lucide_icons::icon_upload(), "旧版 APPX 导入", "解析安装包", "失败", 0.37, "示例错误：安装包不完整，可在这里看到原因", true, true)),
                ),
        )
        .into_any_element()
}

fn demo_task_card(
    colors: &ThemeColors,
    icon: &'static str,
    title: &'static str,
    stage: &'static str,
    status: &'static str,
    progress: f32,
    detail: &'static str,
    danger: bool,
    terminal: bool,
) -> Div {
    let accent = if danger { colors.danger } else { colors.accent };
    div()
        .w_full()
        .px(px(14.0))
        .py(px(11.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.24, ..if danger { colors.danger } else { colors.border } })
        .bg(Hsla { a: 0.72, ..colors.surface })
        .flex()
        .items_center()
        .gap(px(12.0))
        .child(
            div()
                .flex_none()
                .size(px(38.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.10, ..accent })
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(icon).size(px(19.0)).text_color(accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(title))
                        .child(div().text_size(px(10.0)).font_weight(FontWeight::SEMIBOLD).text_color(accent).child(status)),
                )
                .child(div().text_size(px(9.5)).text_color(colors.text_secondary).child(stage))
                .child(
                    div()
                        .w_full()
                        .h(px(5.0))
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(Hsla { a: 0.12, ..colors.text_secondary })
                        .child(
                            div()
                                .w(relative(progress.clamp(0.0, 1.0)))
                                .h_full()
                                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                                .bg(accent),
                        ),
                )
                .child(div().text_size(px(9.0)).text_color(if danger { colors.danger } else { colors.text_muted }).child(detail)),
        )
        .child(
            div()
                .flex_none()
                .size(px(28.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.06, ..colors.text_secondary })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(if terminal { lucide_icons::icon_x() } else { lucide_icons::icon_pause() })
                        .size(px(13.0))
                        .text_color(colors.text_secondary),
                ),
        )
}

fn render_manage_demo_layer(
    scene: OnboardingScene,
    width: f32,
    height: f32,
    colors: &ThemeColors,
) -> AnyElement {
    let page_x = crate::ui::components::page_shell::PAGE_INSET_X / px(1.0);
    let page_y = crate::ui::components::page_shell::PAGE_INSET_TOP / px(1.0);
    let page_bottom = crate::ui::components::page_shell::PAGE_INSET_BOTTOM / px(1.0);
    let sidebar_w = crate::ui::components::page_shell::SPLIT_PAGE_SIDEBAR_WIDTH / px(1.0);
    let full_h = (height - page_y - page_bottom).max(360.0);
    let gap = 12.0;
    let content_x = page_x + sidebar_w + gap;
    let content_w = (width - content_x - page_x).max(420.0);
    let content_is_resource = scene == OnboardingScene::ManageContent;

    div()
        .absolute()
        .inset_0()
        .occlude()
        .child(
            div()
                .absolute()
                .left(px(page_x))
                .top(px(page_y))
                .w(px(sidebar_w))
                .h(px(full_h))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .border_1()
                .border_color(Hsla { a: 0.34, ..colors.border })
                .bg(Hsla { a: 0.98, ..colors.bg })
                .shadow_lg()
                .p(px(12.0))
                .flex()
                .flex_col()
                .gap(px(9.0))
                .child(demo_badge(colors, "只读引导演示"))
                .child(demo_version(colors, "演示版本 1.21.100", "UWP · 正式版", true))
                .child(demo_version(colors, "LeviLamina 1.21.93", "UWP · LeviLamina", false))
                .child(demo_version(colors, "演示 Preview", "GDK · Preview", false))
                .child(demo_version(colors, "旧版 1.20.80", "UWP · 正式版", false)),
        )
        .child(
            div()
                .absolute()
                .left(px(content_x))
                .top(px(page_y))
                .w(px(content_w))
                .h(px(full_h))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .border_1()
                .border_color(Hsla { a: 0.34, ..colors.border })
                .bg(Hsla { a: 0.98, ..colors.bg })
                .shadow_lg()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(16.0))
                        .pt(px(13.0))
                        .pb(px(8.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(div().text_size(px(17.0)).font_weight(FontWeight::BOLD).text_color(colors.text_primary).child("演示版本 1.21.100"))
                                .child(div().text_size(px(10.0)).text_color(colors.text_secondary).child("UWP · 正式版 · 隔离数据目录")),
                        )
                        .child(demo_badge(colors, "不会写入磁盘")),
                )
                .child(render_demo_tool_row(colors))
                .child(render_demo_tabs(colors, content_is_resource))
                .child(if content_is_resource {
                    render_demo_resource_list(colors).into_any_element()
                } else {
                    render_demo_statistics(colors).into_any_element()
                }),
        )
        .into_any_element()
}

fn demo_version(colors: &ThemeColors, name: &'static str, detail: &'static str, selected: bool) -> Div {
    div()
        .w_full()
        .p(px(10.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(if selected {
            Hsla { a: 0.45, ..colors.accent }
        } else {
            Hsla { a: 0.26, ..colors.border }
        })
        .bg(if selected {
            Hsla { a: 0.08, ..colors.accent }
        } else {
            Hsla { a: 0.62, ..colors.surface }
        })
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(div().text_size(px(12.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(name))
        .child(div().text_size(px(9.5)).text_color(colors.text_secondary).child(detail))
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
        .px(px(16.0))
        .py(px(6.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .children(tools.into_iter().map(|(icon, label)| {
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.65, ..colors.surface })
                .border_1()
                .border_color(Hsla { a: 0.24, ..colors.border })
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(svg().path(icon).size(px(12.0)).text_color(colors.text_secondary))
                .child(div().text_size(px(9.0)).text_color(colors.text_secondary).child(label))
        }))
}

fn render_demo_tabs(colors: &ThemeColors, resource_active: bool) -> Div {
    let tabs = ["统计", "模组", "资源包", "皮肤", "地图", "截图", "服务器"];
    div()
        .px(px(16.0))
        .pt(px(5.0))
        .pb(px(8.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .children(tabs.into_iter().enumerate().map(|(index, label)| {
            let active = if resource_active { index == 2 } else { index == 0 };
            div()
                .px(px(7.0))
                .py(px(5.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(if active {
                    Hsla { a: 0.12, ..colors.accent }
                } else {
                    Hsla { a: 0.55, ..colors.surface }
                })
                .text_size(px(9.0))
                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(if active { colors.accent } else { colors.text_secondary })
                .child(label)
        }))
}

fn render_demo_statistics(colors: &ThemeColors) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .px(px(16.0))
        .pb(px(14.0))
        .flex()
        .flex_wrap()
        .gap(px(9.0))
        .child(demo_stat(colors, "游戏版本", "1.21.100"))
        .child(demo_stat(colors, "平台", "UWP"))
        .child(demo_stat(colors, "世界", "3 个"))
        .child(demo_stat(colors, "模组", "2 个启用"))
        .child(demo_stat(colors, "资源包", "4 个"))
        .child(demo_stat(colors, "数据模式", "实例隔离"))
}

fn demo_stat(colors: &ThemeColors, label: &'static str, value: &'static str) -> Div {
    div()
        .w(px(185.0))
        .min_h(px(72.0))
        .p(px(11.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.24, ..colors.border })
        .bg(Hsla { a: 0.66, ..colors.surface })
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(div().text_size(px(9.5)).text_color(colors.text_muted).child(label))
        .child(div().text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(value))
}

fn render_demo_resource_list(colors: &ThemeColors) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .px(px(16.0))
        .pb(px(14.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(demo_asset_row(colors, "Faithful 32x Bedrock", "resource_pack · 已启用", "1.21.x"))
        .child(demo_asset_row(colors, "UI Tweaks", "resource_pack · 已启用", "1.21.x"))
        .child(demo_asset_row(colors, "Better Animations", "behavior_pack · 已禁用", "1.21.100"))
        .child(demo_asset_row(colors, "RTX Demo Pack", "resource_pack · 已禁用", "RTX"))
}

fn demo_asset_row(
    colors: &ThemeColors,
    name: &'static str,
    detail: &'static str,
    version: &'static str,
) -> Div {
    div()
        .w_full()
        .px(px(11.0))
        .py(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.22, ..colors.border })
        .bg(Hsla { a: 0.64, ..colors.surface })
        .flex()
        .items_center()
        .gap(px(9.0))
        .child(
            div()
                .size(px(34.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.10, ..colors.accent })
                .flex()
                .items_center()
                .justify_center()
                .child(svg().path(lucide_icons::icon_package()).size(px(16.0)).text_color(colors.accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(name))
                .child(div().text_size(px(9.0)).text_color(colors.text_secondary).child(detail)),
        )
        .child(demo_badge(colors, version))
}

fn demo_badge(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .px(px(7.0))
        .py(px(3.0))
        .rounded(px(crate::ui::theme::tokens::radius::FULL))
        .bg(Hsla { a: 0.11, ..colors.accent })
        .text_size(px(8.5))
        .font_weight(FontWeight::BOLD)
        .text_color(colors.accent)
        .child(label)
}

fn feature(colors: &ThemeColors, icon: &'static str, title: &'static str, detail: &'static str) -> Div {
    div()
        .w_full()
        .p(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.28, ..colors.border })
        .bg(Hsla { a: 0.58, ..colors.surface })
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(svg().path(icon).size(px(15.0)).text_color(colors.accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_size(px(11.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(title))
                .child(div().text_size(px(9.5)).line_height(px(15.0)).text_color(colors.text_secondary).child(detail)),
        )
}

fn step(colors: &ThemeColors, number: usize, title: &'static str, detail: &'static str) -> Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(
            div()
                .flex_none()
                .w(px(23.0))
                .h(px(23.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla { a: 0.12, ..colors.accent })
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.0))
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
                .gap(px(2.0))
                .child(div().text_size(px(10.5)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(title))
                .child(div().text_size(px(9.5)).line_height(px(15.0)).text_color(colors.text_secondary).child(detail)),
        )
}

fn format_card(colors: &ThemeColors, format: &'static str, detail: &'static str) -> Div {
    div()
        .w_full()
        .p(px(8.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla { a: 0.055, ..colors.accent })
        .flex()
        .items_center()
        .gap(px(9.0))
        .child(
            div()
                .min_w(px(62.0))
                .px(px(7.0))
                .py(px(4.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla { a: 0.12, ..colors.accent })
                .text_center()
                .text_size(px(9.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(format),
        )
        .child(div().flex_1().text_size(px(9.5)).text_color(colors.text_secondary).child(detail))
}

fn intro(colors: &ThemeColors, text: &'static str) -> Div {
    div()
        .w_full()
        .text_size(px(10.5))
        .line_height(px(16.5))
        .text_color(colors.text_secondary)
        .child(text)
}

fn route_badge(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .w_full()
        .px(px(9.0))
        .py(px(7.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla { a: 0.08, ..colors.accent })
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(svg().path(lucide_icons::icon_map_pin()).size(px(12.0)).text_color(colors.accent))
        .child(div().flex_1().text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD).text_color(colors.accent).child(label))
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
        .p(px(8.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla { a: 0.06, ..color })
        .flex()
        .items_start()
        .gap(px(7.0))
        .child(svg().path(icon).size(px(12.0)).text_color(color))
        .child(div().flex_1().min_w(px(0.0)).text_size(px(9.5)).line_height(px(15.0)).text_color(colors.text_secondary).child(text.to_string()))
}

fn platform_summary(
    colors: &ThemeColors,
    summary: &crate::ui::onboarding::state::OnboardingPlatformSummary,
) -> Div {
    let mut items = div().w_full().flex().flex_col().gap(px(6.0));
    for item in &summary.items {
        let color = if item.warning { colors.danger } else { colors.accent };
        let icon = if item.warning {
            lucide_icons::icon_triangle_alert()
        } else {
            lucide_icons::icon_circle_check()
        };
        items = items.child(
            div()
                .flex()
                .items_start()
                .gap(px(7.0))
                .child(svg().path(icon).size(px(12.0)).text_color(color))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(div().text_size(px(9.0)).font_weight(FontWeight::SEMIBOLD).text_color(colors.text_primary).child(item.label.clone()))
                        .child(div().text_size(px(8.5)).line_height(px(13.5)).text_color(colors.text_secondary).child(item.value.clone())),
                ),
        );
    }

    div()
        .w_full()
        .p(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.28, ..colors.border })
        .bg(Hsla { a: 0.62, ..colors.surface })
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(div().text_size(px(10.0)).font_weight(FontWeight::BOLD).text_color(colors.text_primary).child(summary.title.clone()))
        .child(div().text_size(px(8.5)).line_height(px(13.5)).text_color(colors.text_secondary).child(summary.detail.clone()))
        .child(items)
}

fn primary_button(colors: &ThemeColors, label: &'static str, enabled: bool) -> Stateful<Div> {
    let mut button = div()
        .id(SharedString::from(format!("onboarding-guided-primary-{label}")))
        .min_h(px(34.0))
        .px(px(13.0))
        .py(px(7.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.5))
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
        .min_h(px(34.0))
        .px(px(12.0))
        .py(px(7.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla { a: 0.40, ..colors.border })
        .bg(colors.surface)
        .text_color(colors.text_primary)
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}
