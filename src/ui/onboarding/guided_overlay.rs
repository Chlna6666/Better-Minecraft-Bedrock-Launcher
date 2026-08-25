use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::state::{OnboardingAnchor, OnboardingScene, OnboardingTourState};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};

const VIEWPORT_MARGIN: f32 = 14.0;
const PANEL_GAP: f32 = 14.0;
const FOCUS_GAP: f32 = 10.0;
const PAGE_TOP: f32 = 72.0;

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

    fn center_x(self) -> f32 {
        self.x + self.w * 0.5
    }

    fn center_y(self) -> f32 {
        self.y + self.h * 0.5
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

    fn intersects_with_gap(self, other: Self, gap: f32) -> bool {
        self.intersects(other.padded(gap))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewportClass {
    Wide,
    Regular,
    Tight,
}

impl ViewportClass {
    fn for_size(width: f32, height: f32) -> Self {
        if width >= 1180.0 && height >= 680.0 {
            Self::Wide
        } else if width >= 760.0 && height >= 520.0 {
            Self::Regular
        } else {
            Self::Tight
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SceneGeometry {
    panel: RectF,
    focus: Option<RectF>,
    class: ViewportClass,
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
    let geometry = scene_geometry(state, width, height);

    let show_tasks_demo = state.scene == OnboardingScene::TasksOverview
        && crate::tasks::task_manager::try_snapshots_sorted().is_some_and(|tasks| tasks.is_empty());
    let show_manage_demo = matches!(
        state.scene,
        OnboardingScene::ManageOverview | OnboardingScene::ManageContent
    ) && cx
        .global::<crate::ui::views::manage::state::ManagePageState>()
        .versions
        .is_empty();

    let mut root = div().absolute().inset_0();

    if show_tasks_demo {
        root = root.child(render_tasks_demo_layer(width, height, &colors));
    }
    if show_manage_demo {
        root = root.child(render_manage_demo_layer(
            state.scene,
            width,
            height,
            &colors,
        ));
    }

    root = root.child(render_dim_layer(geometry.focus, geometry.class));

    if let Some(focus) = geometry.focus {
        root = root.child(render_spotlight(focus, &colors));
    }

    root.child(
        div()
            .absolute()
            .left(px(geometry.panel.x))
            .top(px(geometry.panel.y))
            .w(px(geometry.panel.w))
            .h(px(geometry.panel.h))
            .child(render_guide_panel(state, &colors, geometry.class)),
    )
    .into_any_element()
}

fn scene_geometry(state: &OnboardingTourState, width: f32, height: f32) -> SceneGeometry {
    let class = ViewportClass::for_size(width, height);
    let focus =
        observed_focus(state, width, height).or_else(|| fallback_focus(state.scene, width, height));
    let (panel_w, panel_h) = adaptive_panel_size(state.scene, width, height, class);
    let panel = place_panel(focus, panel_w, panel_h, width, height, class);

    SceneGeometry {
        panel,
        focus,
        class,
    }
}

fn adaptive_panel_size(
    scene: OnboardingScene,
    width: f32,
    height: f32,
    class: ViewportClass,
) -> (f32, f32) {
    let ideal_w: f32 = match class {
        ViewportClass::Wide => 344.0,
        ViewportClass::Regular => 320.0,
        ViewportClass::Tight => 304.0,
    };
    let ideal_h: f32 = match scene {
        OnboardingScene::Welcome => 334.0,
        OnboardingScene::PlatformSetup => 342.0,
        OnboardingScene::Finish => 300.0,
        OnboardingScene::TasksOverview
        | OnboardingScene::ManageOverview
        | OnboardingScene::ManageContent => 292.0,
        _ => 274.0,
    };

    let max_w = (width - VIEWPORT_MARGIN * 2.0).max(260.0);
    let max_h = (height - PAGE_TOP - VIEWPORT_MARGIN).max(230.0);
    let w = ideal_w.min(max_w);
    let h = ideal_h.min(max_h);
    (w, h)
}

fn place_panel(
    focus: Option<RectF>,
    panel_w: f32,
    panel_h: f32,
    width: f32,
    height: f32,
    class: ViewportClass,
) -> RectF {
    let centered = RectF {
        x: (width - panel_w) * 0.5,
        y: (height - panel_h) * 0.5,
        w: panel_w,
        h: panel_h,
    }
    .clamp(width, height, VIEWPORT_MARGIN);

    let Some(focus) = focus else {
        return centered;
    };

    if class == ViewportClass::Tight {
        let y = if focus.center_y() < height * 0.5 {
            height - VIEWPORT_MARGIN - panel_h
        } else {
            VIEWPORT_MARGIN.max(PAGE_TOP + 4.0)
        };
        return RectF {
            x: (width - panel_w) * 0.5,
            y,
            w: panel_w,
            h: panel_h,
        }
        .clamp(width, height, VIEWPORT_MARGIN);
    }

    let candidates = [
        RectF {
            x: focus.right() + PANEL_GAP,
            y: focus.center_y() - panel_h * 0.5,
            w: panel_w,
            h: panel_h,
        },
        RectF {
            x: focus.x - PANEL_GAP - panel_w,
            y: focus.center_y() - panel_h * 0.5,
            w: panel_w,
            h: panel_h,
        },
        RectF {
            x: focus.center_x() - panel_w * 0.5,
            y: focus.bottom() + PANEL_GAP,
            w: panel_w,
            h: panel_h,
        },
        RectF {
            x: focus.center_x() - panel_w * 0.5,
            y: focus.y - PANEL_GAP - panel_h,
            w: panel_w,
            h: panel_h,
        },
        RectF {
            x: VIEWPORT_MARGIN,
            y: height - VIEWPORT_MARGIN - panel_h,
            w: panel_w,
            h: panel_h,
        },
        RectF {
            x: width - VIEWPORT_MARGIN - panel_w,
            y: height - VIEWPORT_MARGIN - panel_h,
            w: panel_w,
            h: panel_h,
        },
    ];

    for candidate in candidates {
        let placed = candidate.clamp(width, height, VIEWPORT_MARGIN);
        if !placed.intersects_with_gap(focus, FOCUS_GAP) {
            return placed;
        }
    }

    let left_distance = focus.center_x();
    let right_distance = width - focus.center_x();
    RectF {
        x: if left_distance > right_distance {
            VIEWPORT_MARGIN
        } else {
            width - VIEWPORT_MARGIN - panel_w
        },
        y: height - VIEWPORT_MARGIN - panel_h,
        w: panel_w,
        h: panel_h,
    }
    .clamp(width, height, VIEWPORT_MARGIN)
}

fn observed_focus(state: &OnboardingTourState, width: f32, height: f32) -> Option<RectF> {
    let (anchor, padding) = match state.scene {
        OnboardingScene::DownloadNavigation => (OnboardingAnchor::DownloadTabs, 5.0),
        OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => (OnboardingAnchor::DownloadToolbar, 4.0),
        OnboardingScene::ImportPackage => (OnboardingAnchor::DownloadImport, 6.0),
        OnboardingScene::TasksOverview => (OnboardingAnchor::TasksPage, 4.0),
        OnboardingScene::SettingsOverview | OnboardingScene::PlatformSetup => {
            (OnboardingAnchor::SettingsTabs, 5.0)
        }
        OnboardingScene::ToolsOverview => (OnboardingAnchor::ToolsSidebar, 5.0),
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
    let page_w = (width - page_x * 2.0).max(240.0);
    let page_h = (height - page_y - page_bottom).max(220.0);

    match scene {
        OnboardingScene::DownloadNavigation => Some(RectF {
            x: page_x + 20.0,
            y: page_y + 14.0,
            w: 320.0,
            h: 40.0,
        }),
        OnboardingScene::GameDownload
        | OnboardingScene::ResourcePackDownload
        | OnboardingScene::ModDownload => Some(RectF {
            x: page_x,
            y: page_y,
            w: page_w,
            h: 68.0,
        }),
        OnboardingScene::ImportPackage => Some(
            RectF {
                x: width - page_x - 96.0,
                y: page_y + 14.0,
                w: 32.0,
                h: 32.0,
            }
            .padded(6.0),
        ),
        OnboardingScene::TasksOverview => Some(RectF {
            x: page_x + 12.0,
            y: page_y + 12.0,
            w: (page_w - 24.0).max(220.0),
            h: (page_h - 24.0).max(180.0),
        }),
        OnboardingScene::ManageOverview => Some(RectF {
            x: page_x,
            y: page_y,
            w: sidebar_w,
            h: page_h,
        }),
        OnboardingScene::ManageContent => Some(RectF {
            x: page_x + sidebar_w + 12.0,
            y: page_y,
            w: (page_w - sidebar_w - 12.0).max(240.0),
            h: page_h,
        }),
        OnboardingScene::SettingsOverview | OnboardingScene::PlatformSetup => Some(RectF {
            x: page_x + 12.0,
            y: page_y + 12.0,
            w: (page_w - 24.0).max(220.0),
            h: 54.0,
        }),
        OnboardingScene::ToolsOverview => Some(RectF {
            x: page_x,
            y: page_y,
            w: sidebar_w,
            h: page_h,
        }),
        OnboardingScene::Welcome | OnboardingScene::Finish => None,
    }
    .map(|bounds| bounds.clamp(width, height, 6.0))
}

fn render_dim_layer(focus: Option<RectF>, class: ViewportClass) -> AnyElement {
    let alpha = match class {
        ViewportClass::Wide => 0.16,
        ViewportClass::Regular => 0.14,
        ViewportClass::Tight => 0.11,
    };

    let Some(focus) = focus else {
        return div()
            .absolute()
            .inset_0()
            .bg(Hsla {
                a: alpha,
                ..black()
            })
            .occlude()
            .into_any_element();
    };

    rounded_cutout(
        Bounds::new(
            point(px(focus.x), px(focus.y)),
            size(px(focus.w), px(focus.h)),
        ),
        px(crate::ui::theme::tokens::radius::MD),
        Hsla {
            a: alpha,
            ..black()
        },
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
        .border_color(Hsla {
            a: 0.94,
            ..colors.accent
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.22,
                ..colors.accent
            },
            blur_radius: px(18.0),
            spread_radius: px(2.0),
            offset: point(px(0.0), px(0.0)),
        }])
        .bg(Hsla {
            a: 0.012,
            ..colors.accent
        })
}

fn render_guide_panel(
    state: &OnboardingTourState,
    colors: &ThemeColors,
    class: ViewportClass,
) -> Div {
    let inner_px = if class == ViewportClass::Tight {
        13.0
    } else {
        15.0
    };
    div()
        .size_full()
        .min_h(px(0.0))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.965,
            ..colors.bg
        })
        .shadow(vec![BoxShadow {
            color: Hsla { a: 0.20, ..black() },
            blur_radius: px(30.0),
            spread_radius: px(-6.0),
            offset: point(px(0.0), px(12.0)),
        }])
        .occlude()
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(render_header(state, colors, class))
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .px(px(inner_px))
                .py(px(11.0))
                .child(render_scene_body(state, colors)),
        )
        .child(render_footer(state, colors))
}

fn render_header(state: &OnboardingTourState, colors: &ThemeColors, class: ViewportClass) -> Div {
    let (icon, title, subtitle) = scene_header(state.scene);
    let icon_size = if class == ViewportClass::Tight {
        34.0
    } else {
        38.0
    };
    div()
        .px(px(15.0))
        .pt(px(13.0))
        .pb(px(10.0))
        .flex()
        .items_start()
        .gap(px(9.0))
        .child(
            div()
                .flex_none()
                .w(px(icon_size))
                .h(px(icon_size))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.12,
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
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .line_height(px(15.0))
                        .text_color(colors.text_secondary)
                        .child(subtitle),
                ),
        )
        .child(
            div()
                .flex_none()
                .px(px(7.0))
                .py(px(3.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.10,
                    ..colors.accent
                })
                .text_size(px(9.0))
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
        .gap(px(8.0))
        .child(intro(
            colors,
            "不用先记所有功能。导览会直接切到真实页面，只解释第一次真正会用到的入口。",
        ))
        .child(feature(
            colors,
            lucide_icons::icon_download(),
            "获得游戏",
            "在线下载，或导入已有 APPX / ZIP / MSIXVC。",
        ))
        .child(feature(
            colors,
            lucide_icons::icon_activity(),
            "看任务",
            "下载、安装、导入和失败原因都集中在任务页。",
        ))
        .child(feature(
            colors,
            lucide_icons::icon_settings_2(),
            "管理版本",
            "启动、模组、资源包、地图等操作都围绕具体实例。",
        ))
        .child(tip(colors, "演示数据只在真实页面为空时出现，不写入磁盘。"))
        .into_any_element()
}

fn render_download_navigation(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：下载类型标签"))
        .child(feature(
            colors,
            lucide_icons::icon_box(),
            "游戏",
            "Minecraft Bedrock 本体和历史版本。",
        ))
        .child(feature(
            colors,
            lucide_icons::icon_package(),
            "资源包",
            "CurseForge 内容，安装到指定实例。",
        ))
        .child(feature(
            colors,
            lucide_icons::icon_layers(),
            "模组",
            "LeviLamina / LeviLauncher 客户端模组。",
        ))
        .into_any_element()
}

fn render_game_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：搜索与版本筛选区"))
        .child(step(
            colors,
            1,
            "搜索或筛选",
            "知道版本号就直接搜；日常使用优先正式版。",
        ))
        .child(step(
            colors,
            2,
            "选择加载器",
            "不需要模组时保持原版最简单。",
        ))
        .child(step(
            colors,
            3,
            "点列表右侧按钮",
            "下载或安装完成后会进入本地版本列表。",
        ))
        .child(tip(colors, "第一次不知道怎么选：正式版 + 原版。"))
        .into_any_element()
}

fn render_resource_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：CurseForge 搜索与筛选"))
        .child(step(colors, 1, "找项目", "按分类或关键字找到资源包。"))
        .child(step(
            colors,
            2,
            "确认游戏版本",
            "版本不匹配时不要强行安装。",
        ))
        .child(step(
            colors,
            3,
            "选择目标实例",
            "资源写入指定实例，不会替换 Minecraft 本体。",
        ))
        .into_any_element()
}

fn render_mod_download(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：模组加载器筛选"))
        .child(step(
            colors,
            1,
            "先看加载器",
            "选择 LeviLamina 等加载器类型和版本。",
        ))
        .child(step(
            colors,
            2,
            "再看兼容性",
            "游戏、加载器、模组三者版本需要匹配。",
        ))
        .child(step(
            colors,
            3,
            "最后选实例",
            "安装前确认目标版本，避免放错目录。",
        ))
        .into_any_element()
}

fn render_import(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：右上角上传按钮"))
        .child(format_card(colors, "APPX", "常见 UWP 安装包。"))
        .child(format_card(colors, "ZIP", "BMCBL 支持的版本压缩包。"))
        .child(format_card(
            colors,
            "MSIXVC",
            "部分 GDK 版本使用的容器格式。",
        ))
        .child(tip(colors, "选完文件后去任务页看解析、解包和导入进度。"))
        .into_any_element()
}

fn render_tasks_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "任务页：下载以后先看这里"))
        .child(step(
            colors,
            1,
            "进度",
            "百分比、速度、线程和 ETA 会集中显示。",
        ))
        .child(step(colors, 2, "控制", "支持的任务可以暂停、继续或取消。"))
        .child(step(
            colors,
            3,
            "错误",
            "失败原因也会留在任务卡里，方便排查。",
        ))
        .child(tip(colors, "没有真实任务时才会显示只读演示任务。"))
        .into_any_element()
}

fn render_manage_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：版本列表"))
        .child(step(colors, 1, "先选实例", "真实版本来自 BMCBL/versions。"))
        .child(step(
            colors,
            2,
            "再做实例级操作",
            "打开目录、设置、删除、启动只作用于当前版本。",
        ))
        .child(tip(
            colors,
            "没有真实版本时才投影只读示例，不创建任何目录。",
        ))
        .into_any_element()
}

fn render_manage_content(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：当前实例的内容区域"))
        .child(step(
            colors,
            1,
            "按标签管理",
            "统计、Mod、资源包、皮肤、地图、截图、服务器各自独立。",
        ))
        .child(step(
            colors,
            2,
            "所有操作都有实例边界",
            "启用、禁用、导入和删除都只针对当前版本。",
        ))
        .child(step(
            colors,
            3,
            "存档操作要留备份",
            "地图和 level.dat 会修改真实世界数据。",
        ))
        .into_any_element()
}

fn render_settings_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：设置分类"))
        .child(step(
            colors,
            1,
            "保持默认也能用",
            "第一次不需要把所有设置检查一遍。",
        ))
        .child(step(
            colors,
            2,
            "下载或网络有问题",
            "回启动器设置检查下载线程、代理和 API。",
        ))
        .child(step(
            colors,
            3,
            "外观 / 插件 / 关于",
            "主题、WASM 插件和重新打开导览都在这里。",
        ))
        .into_any_element()
}

fn render_tools_overview(colors: &ThemeColors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, "当前高光：工具列表"))
        .child(step(
            colors,
            1,
            "按需使用",
            "工具不是启动 Minecraft 的必经步骤。",
        ))
        .child(step(
            colors,
            2,
            "当前主要是联机大厅",
            "通过 EasyTier 创建或加入跨网络房间。",
        ))
        .child(step(
            colors,
            3,
            "联机异常再看高级项",
            "NAT、节点、P2P 和 bootstrap 用于排障。",
        ))
        .into_any_element()
}

fn render_platform(state: &OnboardingTourState, colors: &ThemeColors) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(8.0));

    #[cfg(target_os = "windows")]
    {
        body = body
            .child(route_badge(colors, "Windows：UWP 注册与数据保护"))
            .child(step(
                colors,
                1,
                "BMCBL 散装 UWP",
                "切换版本时重新指向目标版本目录。",
            ))
            .child(step(
                colors,
                2,
                "Store / 外部注册",
                "存在世界数据时先备份并校验，失败就停止替换。",
            ));
    }

    #[cfg(target_os = "linux")]
    {
        body = body
            .child(route_badge(colors, "Linux：Proton-GDK / UMU"))
            .child(step(
                colors,
                1,
                "Linux 不执行 UWP 检查",
                "不会处理 Microsoft Store 注册或 Windows LocalState。",
            ))
            .child(step(
                colors,
                2,
                "检查兼容运行环境",
                "重点确认 Proton-GDK / UMU runner 和系统依赖。",
            ));
    }

    if state.platform_scanning {
        body = body.child(status(
            colors,
            lucide_icons::icon_loader_circle(),
            "正在检测当前电脑…",
            false,
        ));
    } else if let Some(error) = state.error.as_deref() {
        body = body.child(dynamic_status(
            colors,
            lucide_icons::icon_triangle_alert(),
            error,
            true,
        ));
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
        .gap(px(9.0))
        .child(
            div()
                .py(px(7.0))
                .flex()
                .items_center()
                .gap(px(9.0))
                .child(
                    div()
                        .size(px(42.0))
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
                                .size(px(21.0))
                                .text_color(colors.accent),
                        ),
                )
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("现在知道下一步去哪了"),
                ),
        )
        .child(feature(
            colors,
            lucide_icons::icon_download(),
            "没有游戏",
            "去下载页选正式版，或导入已有安装包。",
        ))
        .child(feature(
            colors,
            lucide_icons::icon_settings_2(),
            "已经有版本",
            "去管理页选择实例并启动或管理内容。",
        ))
        .child(tip(colors, "以后可在“设置 → 关于”重新打开完整导览。"))
        .into_any_element()
}

fn render_footer(state: &OnboardingTourState, colors: &ThemeColors) -> Div {
    let scene = state.scene;
    let left_label = if scene == OnboardingScene::Welcome {
        "跳过"
    } else {
        "上一步"
    };
    let left =
        secondary_button(colors, left_label).on_mouse_down(MouseButton::Left, move |_, _, cx| {
            if scene == OnboardingScene::Welcome {
                crate::ui::onboarding::skip(cx);
            } else {
                crate::ui::onboarding::back(cx);
            }
        });

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
        .px(px(13.0))
        .py(px(9.0))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(9.0))
        .child(left)
        .child(next)
}

fn scene_header(scene: OnboardingScene) -> (&'static str, &'static str, &'static str) {
    match scene {
        OnboardingScene::Welcome => (
            lucide_icons::icon_route(),
            "欢迎使用 BMCBL",
            "跟着真实页面走一遍，不用先学所有设置。",
        ),
        OnboardingScene::DownloadNavigation => (
            lucide_icons::icon_download(),
            "先认识下载页",
            "高光会直接指向当前要看的区域。",
        ),
        OnboardingScene::GameDownload => (
            lucide_icons::icon_box(),
            "下载 Minecraft",
            "搜索、筛选，然后从列表开始下载。",
        ),
        OnboardingScene::ResourcePackDownload => (
            lucide_icons::icon_package(),
            "CurseForge 资源包",
            "找项目、确认版本、选择安装目标。",
        ),
        OnboardingScene::ModDownload => (
            lucide_icons::icon_layers(),
            "客户端模组",
            "先确认加载器与兼容关系。",
        ),
        OnboardingScene::ImportPackage => (
            lucide_icons::icon_upload(),
            "导入已有安装包",
            "已有文件就不必重新下载。",
        ),
        OnboardingScene::TasksOverview => (
            lucide_icons::icon_activity(),
            "任务去哪里看？",
            "下载、安装、导入和错误都集中在这里。",
        ),
        OnboardingScene::ManageOverview => (
            lucide_icons::icon_settings_2(),
            "管理一个版本",
            "先选实例，再做启动与实例级操作。",
        ),
        OnboardingScene::ManageContent => (
            lucide_icons::icon_package(),
            "管理实例内容",
            "Mod、资源包、地图等都属于当前版本。",
        ),
        OnboardingScene::SettingsOverview => (
            lucide_icons::icon_settings(),
            "设置在哪里？",
            "大多数配置保持默认即可。",
        ),
        OnboardingScene::ToolsOverview => (
            lucide_icons::icon_wrench(),
            "工具在哪里？",
            "高级能力按需使用。",
        ),
        OnboardingScene::PlatformSetup => {
            #[cfg(target_os = "windows")]
            {
                (
                    lucide_icons::icon_shield_check(),
                    "Windows 数据安全",
                    "替换 UWP 注册前先保护已有世界数据。",
                )
            }
            #[cfg(target_os = "linux")]
            {
                (
                    lucide_icons::icon_box(),
                    "Linux 运行环境",
                    "确认 Proton-GDK / UMU，不做 Windows UWP。",
                )
            }
        }
        OnboardingScene::Finish => (
            lucide_icons::icon_circle_check(),
            "导览完成",
            "现在可以按自己的情况开始。",
        ),
    }
}

fn render_tasks_demo_layer(width: f32, height: f32, colors: &ThemeColors) -> AnyElement {
    let page_x = crate::ui::components::page_shell::PAGE_INSET_X / px(1.0);
    let page_y = crate::ui::components::page_shell::PAGE_INSET_TOP / px(1.0);
    let page_bottom = crate::ui::components::page_shell::PAGE_INSET_BOTTOM / px(1.0);
    let page_w = (width - page_x * 2.0).max(420.0);
    let page_h = (height - page_y - page_bottom).max(320.0);

    div()
        .absolute()
        .left(px(page_x))
        .top(px(page_y))
        .w(px(page_w))
        .h(px(page_h))
        .occlude()
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: 0.96,
            ..colors.bg
        })
        .p(px(14.0))
        .flex()
        .flex_col()
        .gap(px(9.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("任务"),
                )
                .child(demo_badge(colors, "只读演示 · 无网络请求")),
        )
        .child(demo_task_card(
            colors,
            "Minecraft 1.21.100",
            "下载中",
            0.68,
            "18.4 MB/s · 12 线程 · ETA 00:42",
            false,
        ))
        .child(demo_task_card(
            colors,
            "Faithful 32x",
            "安装中",
            0.88,
            "正在写入目标实例",
            false,
        ))
        .child(demo_task_card(
            colors,
            "LeviLamina 模组依赖",
            "完成",
            1.0,
            "已安装到演示版本",
            false,
        ))
        .child(demo_task_card(
            colors,
            "旧版 APPX 导入",
            "失败",
            0.37,
            "示例：安装包不完整",
            true,
        ))
        .into_any_element()
}

fn demo_task_card(
    colors: &ThemeColors,
    title: &'static str,
    status: &'static str,
    progress: f32,
    detail: &'static str,
    danger: bool,
) -> Div {
    let accent = if danger { colors.danger } else { colors.accent };
    div()
        .w_full()
        .px(px(12.0))
        .py(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.64,
            ..colors.surface
        })
        .flex()
        .items_center()
        .gap(px(10.0))
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
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(9.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(accent)
                                .child(status),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(5.0))
                        .rounded(px(crate::ui::theme::tokens::radius::FULL))
                        .bg(Hsla {
                            a: 0.12,
                            ..colors.text_secondary
                        })
                        .child(
                            div()
                                .w(relative(progress.clamp(0.0, 1.0)))
                                .h_full()
                                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                                .bg(accent),
                        ),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .text_color(if danger {
                            colors.danger
                        } else {
                            colors.text_muted
                        })
                        .child(detail),
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
    let full_h = (height - page_y - page_bottom).max(320.0);
    let content_x = page_x + sidebar_w + 12.0;
    let content_w = (width - content_x - page_x).max(320.0);
    let content_is_resource = scene == OnboardingScene::ManageContent;

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
                .occlude()
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .bg(Hsla {
                    a: 0.95,
                    ..colors.bg
                })
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(demo_badge(colors, "空页面示例"))
                .child(demo_version(
                    colors,
                    "Minecraft 1.21.100",
                    "UWP · 正式版",
                    true,
                ))
                .child(demo_version(
                    colors,
                    "LeviLamina 1.21.93",
                    "UWP · LeviLamina",
                    false,
                ))
                .child(demo_version(colors, "Preview", "GDK · Preview", false)),
        )
        .child(
            div()
                .absolute()
                .left(px(content_x))
                .top(px(page_y))
                .w(px(content_w))
                .h(px(full_h))
                .occlude()
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .bg(Hsla {
                    a: 0.95,
                    ..colors.bg
                })
                .p(px(12.0))
                .flex()
                .flex_col()
                .gap(px(9.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child("Minecraft 1.21.100"),
                        )
                        .child(demo_badge(colors, "只读示例")),
                )
                .child(render_demo_tabs(colors, content_is_resource))
                .child(if content_is_resource {
                    render_demo_resource_list(colors).into_any_element()
                } else {
                    render_demo_statistics(colors).into_any_element()
                }),
        )
        .into_any_element()
}

fn demo_version(
    colors: &ThemeColors,
    name: &'static str,
    detail: &'static str,
    selected: bool,
) -> Div {
    div()
        .w_full()
        .p(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(if selected {
            Hsla {
                a: 0.10,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.60,
                ..colors.surface
            }
        })
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(name),
        )
        .child(
            div()
                .text_size(px(9.0))
                .text_color(colors.text_secondary)
                .child(detail),
        )
}

fn render_demo_tabs(colors: &ThemeColors, resource_active: bool) -> Div {
    let tabs = ["统计", "Mod", "资源包", "皮肤", "地图", "截图", "服务器"];
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .flex_wrap()
        .children(tabs.into_iter().enumerate().map(|(index, label)| {
            let active = if resource_active {
                index == 2
            } else {
                index == 0
            };
            div()
                .px(px(7.0))
                .py(px(5.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(if active {
                    Hsla {
                        a: 0.12,
                        ..colors.accent
                    }
                } else {
                    Hsla {
                        a: 0.55,
                        ..colors.surface
                    }
                })
                .text_size(px(9.0))
                .font_weight(if active {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if active {
                    colors.accent
                } else {
                    colors.text_secondary
                })
                .child(label)
        }))
}

fn render_demo_statistics(colors: &ThemeColors) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_wrap()
        .gap(px(8.0))
        .child(demo_stat(colors, "游戏版本", "1.21.100"))
        .child(demo_stat(colors, "平台", "UWP"))
        .child(demo_stat(colors, "世界", "3 个"))
        .child(demo_stat(colors, "Mod", "2 个启用"))
        .child(demo_stat(colors, "资源包", "4 个"))
        .child(demo_stat(colors, "数据模式", "实例隔离"))
}

fn demo_stat(colors: &ThemeColors, label: &'static str, value: &'static str) -> Div {
    div()
        .w(px(154.0))
        .min_h(px(62.0))
        .p(px(9.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.62,
            ..colors.surface
        })
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .text_size(px(9.0))
                .text_color(colors.text_muted)
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(value),
        )
}

fn render_demo_resource_list(colors: &ThemeColors) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(demo_asset_row(
            colors,
            "Faithful 32x Bedrock",
            "资源包 · 已启用",
            "1.21.x",
        ))
        .child(demo_asset_row(
            colors,
            "UI Tweaks",
            "资源包 · 已启用",
            "1.21.x",
        ))
        .child(demo_asset_row(
            colors,
            "Better Animations",
            "行为包 · 已禁用",
            "1.21.100",
        ))
}

fn demo_asset_row(
    colors: &ThemeColors,
    name: &'static str,
    detail: &'static str,
    version: &'static str,
) -> Div {
    div()
        .w_full()
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.62,
            ..colors.surface
        })
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            svg()
                .path(lucide_icons::icon_package())
                .size(px(15.0))
                .text_color(colors.accent),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(8.5))
                        .text_color(colors.text_secondary)
                        .child(detail),
                ),
        )
        .child(demo_badge(colors, version))
}

fn demo_badge(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .px(px(7.0))
        .py(px(3.0))
        .rounded(px(crate::ui::theme::tokens::radius::FULL))
        .bg(Hsla {
            a: 0.10,
            ..colors.accent
        })
        .text_size(px(8.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.accent)
        .child(label)
}

fn feature(
    colors: &ThemeColors,
    icon: &'static str,
    title: &'static str,
    detail: &'static str,
) -> Div {
    div()
        .w_full()
        .px(px(8.0))
        .py(px(7.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.055,
            ..colors.accent
        })
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(svg().path(icon).size(px(14.0)).text_color(colors.accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .line_height(px(14.0))
                        .text_color(colors.text_secondary)
                        .child(detail),
                ),
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
                .w(px(22.0))
                .h(px(22.0))
                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                .bg(Hsla {
                    a: 0.11,
                    ..colors.accent
                })
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
                .gap(px(1.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(9.0))
                        .line_height(px(14.0))
                        .text_color(colors.text_secondary)
                        .child(detail),
                ),
        )
}

fn format_card(colors: &ThemeColors, format: &'static str, detail: &'static str) -> Div {
    div()
        .w_full()
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.055,
            ..colors.accent
        })
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .min_w(px(60.0))
                .px(px(7.0))
                .py(px(4.0))
                .rounded(px(crate::ui::theme::tokens::radius::SM))
                .bg(Hsla {
                    a: 0.11,
                    ..colors.accent
                })
                .text_center()
                .text_size(px(9.0))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.accent)
                .child(format),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(9.0))
                .text_color(colors.text_secondary)
                .child(detail),
        )
}

fn intro(colors: &ThemeColors, text: &'static str) -> Div {
    div()
        .w_full()
        .text_size(px(10.0))
        .line_height(px(15.5))
        .text_color(colors.text_secondary)
        .child(text)
}

fn route_badge(colors: &ThemeColors, label: &'static str) -> Div {
    div()
        .w_full()
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.075,
            ..colors.accent
        })
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            svg()
                .path(lucide_icons::icon_map_pin())
                .size(px(12.0))
                .text_color(colors.accent),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(9.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.accent)
                .child(label),
        )
}

fn tip(colors: &ThemeColors, text: &'static str) -> Div {
    status(colors, lucide_icons::icon_info(), text, false)
}

fn status(colors: &ThemeColors, icon: &'static str, text: &'static str, danger: bool) -> Div {
    dynamic_status(colors, icon, text, danger)
}

fn dynamic_status(colors: &ThemeColors, icon: &'static str, text: &str, danger: bool) -> Div {
    let color = if danger {
        colors.danger
    } else {
        colors.text_secondary
    };
    div()
        .w_full()
        .px(px(8.0))
        .py(px(7.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla { a: 0.055, ..color })
        .flex()
        .items_start()
        .gap(px(7.0))
        .child(svg().path(icon).size(px(12.0)).text_color(color))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(9.0))
                .line_height(px(14.0))
                .text_color(colors.text_secondary)
                .child(text.to_string()),
        )
}

fn platform_summary(
    colors: &ThemeColors,
    summary: &crate::ui::onboarding::state::OnboardingPlatformSummary,
) -> Div {
    let mut items = div().w_full().flex().flex_col().gap(px(5.0));
    for item in &summary.items {
        let color = if item.warning {
            colors.danger
        } else {
            colors.accent
        };
        let icon = if item.warning {
            lucide_icons::icon_triangle_alert()
        } else {
            lucide_icons::icon_circle_check()
        };
        items = items.child(
            div()
                .flex()
                .items_start()
                .gap(px(6.0))
                .child(svg().path(icon).size(px(11.0)).text_color(color))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_size(px(8.8))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(8.2))
                                .line_height(px(13.0))
                                .text_color(colors.text_secondary)
                                .child(item.value.clone()),
                        ),
                ),
        );
    }

    div()
        .w_full()
        .px(px(8.0))
        .py(px(7.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.58,
            ..colors.surface
        })
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(
            div()
                .text_size(px(9.5))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_primary)
                .child(summary.title.clone()),
        )
        .child(
            div()
                .text_size(px(8.2))
                .line_height(px(13.0))
                .text_color(colors.text_secondary)
                .child(summary.detail.clone()),
        )
        .child(items)
}

fn primary_button(colors: &ThemeColors, label: &'static str, enabled: bool) -> Stateful<Div> {
    let mut button = div()
        .id(SharedString::from(format!(
            "onboarding-guided-primary-{label}"
        )))
        .min_h(px(32.0))
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.0))
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
        .id(SharedString::from(format!(
            "onboarding-guided-secondary-{label}"
        )))
        .min_h(px(32.0))
        .px(px(11.0))
        .py(px(6.0))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(Hsla {
            a: 0.68,
            ..colors.surface
        })
        .text_color(colors.text_primary)
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}
