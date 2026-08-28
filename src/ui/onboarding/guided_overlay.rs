use gpui::*;
use lucide_gpui::icons as lucide_icons;

use super::state::{OnboardingAnchor, OnboardingScene, OnboardingTourState};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::i18n::I18n;
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
    let i18n = cx.global::<I18n>();
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
        root = root.child(render_tasks_demo_layer(width, height, &colors, i18n));
    }
    if show_manage_demo {
        root = root.child(render_manage_demo_layer(
            state.scene,
            width,
            height,
            &colors,
            i18n,
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
            .child(render_guide_panel(state, &colors, i18n, geometry.class)),
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
    i18n: &I18n,
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
        .child(render_header(state, colors, i18n, class))
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .px(px(inner_px))
                .py(px(11.0))
                .child(render_scene_body(state, colors, i18n)),
        )
        .child(render_footer(state, colors, i18n))
}

fn render_header(
    state: &OnboardingTourState,
    colors: &ThemeColors,
    i18n: &I18n,
    class: ViewportClass,
) -> Div {
    let (icon, title, subtitle) = scene_header(state.scene, i18n);
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

fn render_scene_body(state: &OnboardingTourState, colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    match state.scene {
        OnboardingScene::Welcome => render_welcome(colors, i18n),
        OnboardingScene::DownloadNavigation => render_download_navigation(colors, i18n),
        OnboardingScene::GameDownload => render_game_download(colors, i18n),
        OnboardingScene::ResourcePackDownload => render_resource_download(colors, i18n),
        OnboardingScene::ModDownload => render_mod_download(colors, i18n),
        OnboardingScene::ImportPackage => render_import(colors, i18n),
        OnboardingScene::TasksOverview => render_tasks_overview(colors, i18n),
        OnboardingScene::ManageOverview => render_manage_overview(colors, i18n),
        OnboardingScene::ManageContent => render_manage_content(colors, i18n),
        OnboardingScene::SettingsOverview => render_settings_overview(colors, i18n),
        OnboardingScene::ToolsOverview => render_tools_overview(colors, i18n),
        OnboardingScene::PlatformSetup => render_platform(state, colors, i18n),
        OnboardingScene::Finish => render_finish(colors, i18n),
    }
}

fn render_welcome(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(intro(colors, t!("Onboarding.welcome.intro")))
        .child(feature(
            colors,
            lucide_icons::icon_download(),
            t!("Onboarding.welcome.get_game"),
            t!("Onboarding.welcome.get_game_detail"),
        ))
        .child(feature(
            colors,
            lucide_icons::icon_activity(),
            t!("Onboarding.welcome.tasks"),
            t!("Onboarding.welcome.tasks_detail"),
        ))
        .child(feature(
            colors,
            lucide_icons::icon_settings_2(),
            t!("Onboarding.welcome.manage"),
            t!("Onboarding.welcome.manage_detail"),
        ))
        .child(tip(colors, t!("Onboarding.welcome.demo_hint")))
        .into_any_element()
}

fn render_download_navigation(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(
            colors,
            t!("Onboarding.download.highlight_types"),
        ))
        .child(feature(
            colors,
            lucide_icons::icon_box(),
            t!("Onboarding.common.game"),
            t!("Onboarding.download.game_detail"),
        ))
        .child(feature(
            colors,
            lucide_icons::icon_package(),
            t!("Onboarding.common.resource_pack"),
            t!("Onboarding.download.resource_detail"),
        ))
        .child(feature(
            colors,
            lucide_icons::icon_layers(),
            t!("Onboarding.common.mods"),
            t!("Onboarding.download.mod_detail"),
        ))
        .into_any_element()
}

fn render_game_download(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.game.highlight_search")))
        .child(step(
            colors,
            1,
            t!("Onboarding.game.search"),
            t!("Onboarding.game.search_detail"),
        ))
        .child(step(
            colors,
            2,
            t!("Onboarding.game.loader"),
            t!("Onboarding.game.loader_detail"),
        ))
        .child(step(
            colors,
            3,
            t!("Onboarding.game.action"),
            t!("Onboarding.game.action_detail"),
        ))
        .child(tip(colors, t!("Onboarding.game.first_tip")))
        .into_any_element()
}

fn render_resource_download(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.resource.highlight")))
        .child(step(
            colors,
            1,
            t!("Onboarding.resource.find"),
            t!("Onboarding.resource.find_detail"),
        ))
        .child(step(
            colors,
            2,
            t!("Onboarding.resource.version"),
            t!("Onboarding.resource.version_detail"),
        ))
        .child(step(
            colors,
            3,
            t!("Onboarding.resource.target"),
            t!("Onboarding.resource.target_detail"),
        ))
        .into_any_element()
}

fn render_mod_download(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.mod.highlight")))
        .child(step(
            colors,
            1,
            t!("Onboarding.mod.loader"),
            t!("Onboarding.mod.loader_detail"),
        ))
        .child(step(
            colors,
            2,
            t!("Onboarding.mod.compatibility"),
            t!("Onboarding.mod.compatibility_detail"),
        ))
        .child(step(
            colors,
            3,
            t!("Onboarding.mod.target"),
            t!("Onboarding.mod.target_detail"),
        ))
        .into_any_element()
}

fn render_import(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.import.highlight")))
        .child(format_card(colors, "APPX", t!("Onboarding.import.appx")))
        .child(format_card(colors, "ZIP", t!("Onboarding.import.zip")))
        .child(format_card(
            colors,
            "MSIXVC",
            t!("Onboarding.import.msixvc"),
        ))
        .child(tip(colors, t!("Onboarding.import.tip")))
        .into_any_element()
}

fn render_tasks_overview(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.tasks.highlight")))
        .child(step(
            colors,
            1,
            t!("Onboarding.tasks.progress"),
            t!("Onboarding.tasks.progress_detail"),
        ))
        .child(step(
            colors,
            2,
            t!("Onboarding.tasks.controls"),
            t!("Onboarding.tasks.controls_detail"),
        ))
        .child(step(
            colors,
            3,
            t!("Onboarding.tasks.errors"),
            t!("Onboarding.tasks.errors_detail"),
        ))
        .child(tip(colors, t!("Onboarding.tasks.demo_hint")))
        .into_any_element()
}

fn render_manage_overview(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.manage.highlight")))
        .child(step(
            colors,
            1,
            t!("Onboarding.manage.select"),
            t!("Onboarding.manage.select_detail"),
        ))
        .child(step(
            colors,
            2,
            t!("Onboarding.manage.actions"),
            t!("Onboarding.manage.actions_detail"),
        ))
        .child(tip(colors, t!("Onboarding.manage.demo_hint")))
        .into_any_element()
}

fn render_manage_content(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.content.highlight")))
        .child(step(
            colors,
            1,
            t!("Onboarding.content.tabs"),
            t!("Onboarding.content.tabs_detail"),
        ))
        .child(step(
            colors,
            2,
            t!("Onboarding.content.scope"),
            t!("Onboarding.content.scope_detail"),
        ))
        .child(step(
            colors,
            3,
            t!("Onboarding.content.backup"),
            t!("Onboarding.content.backup_detail"),
        ))
        .into_any_element()
}

fn render_settings_overview(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.settings.highlight")))
        .child(step(
            colors,
            1,
            t!("Onboarding.settings.defaults"),
            t!("Onboarding.settings.defaults_detail"),
        ))
        .child(step(
            colors,
            2,
            t!("Onboarding.settings.network"),
            t!("Onboarding.settings.network_detail"),
        ))
        .child(step(
            colors,
            3,
            t!("Onboarding.settings.other"),
            t!("Onboarding.settings.other_detail"),
        ))
        .into_any_element()
}

fn render_tools_overview(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(route_badge(colors, t!("Onboarding.tools.highlight")))
        .child(step(
            colors,
            1,
            t!("Onboarding.tools.optional"),
            t!("Onboarding.tools.optional_detail"),
        ))
        .child(step(
            colors,
            2,
            t!("Onboarding.tools.online"),
            t!("Onboarding.tools.online_detail"),
        ))
        .child(step(
            colors,
            3,
            t!("Onboarding.tools.advanced"),
            t!("Onboarding.tools.advanced_detail"),
        ))
        .into_any_element()
}

fn render_platform(state: &OnboardingTourState, colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(8.0));

    #[cfg(target_os = "windows")]
    {
        body = body
            .child(route_badge(colors, t!("Onboarding.platform.windows")))
            .child(step(
                colors,
                1,
                t!("Onboarding.platform.windows_step"),
                t!("Onboarding.platform.windows_step_detail"),
            ))
            .child(step(
                colors,
                2,
                t!("Onboarding.platform.store_step"),
                t!("Onboarding.platform.store_step_detail"),
            ));
    }

    #[cfg(target_os = "linux")]
    {
        body = body
            .child(route_badge(colors, t!("Onboarding.platform.linux")))
            .child(step(
                colors,
                1,
                t!("Onboarding.platform.linux_step"),
                t!("Onboarding.platform.linux_step_detail"),
            ))
            .child(step(
                colors,
                2,
                t!("Onboarding.platform.runtime_step"),
                t!("Onboarding.platform.runtime_step_detail"),
            ));
    }

    if state.platform_scanning {
        body = body.child(status(
            colors,
            lucide_icons::icon_loader_circle(),
            t!("Onboarding.platform.scanning"),
            false,
        ));
    } else if let Some(error) = &state.error {
        let error = i18n.resolve(error);
        body = body.child(dynamic_status(
            colors,
            lucide_icons::icon_triangle_alert(),
            error,
            true,
        ));
    } else if let Some(summary) = &state.platform_summary {
        body = body.child(platform_summary(colors, i18n, summary));
    } else {
        body = body.child(tip(colors, t!("Onboarding.platform.waiting")));
    }

    body.into_any_element()
}

fn render_finish(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
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
                        .child(t!("Onboarding.finish.next")),
                ),
        )
        .child(feature(
            colors,
            lucide_icons::icon_download(),
            t!("Onboarding.finish.no_game"),
            t!("Onboarding.finish.no_game_detail"),
        ))
        .child(feature(
            colors,
            lucide_icons::icon_settings_2(),
            t!("Onboarding.finish.has_version"),
            t!("Onboarding.finish.has_version_detail"),
        ))
        .child(tip(colors, t!("Onboarding.finish.reopen_hint")))
        .into_any_element()
}

fn render_footer(state: &OnboardingTourState, colors: &ThemeColors, i18n: &I18n) -> Div {
    let scene = state.scene;
    let left_label = if scene == OnboardingScene::Welcome {
        t!("Onboarding.skip")
    } else {
        t!("common.back")
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
        t!("Onboarding.finish_button")
    } else {
        t!("Onboarding.next")
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

fn scene_header(scene: OnboardingScene, i18n: &I18n) -> (&'static str, SharedString, SharedString) {
    match scene {
        OnboardingScene::Welcome => (
            lucide_icons::icon_route(),
            t!("Onboarding.header.welcome"),
            t!("Onboarding.header.welcome_detail"),
        ),
        OnboardingScene::DownloadNavigation => (
            lucide_icons::icon_download(),
            t!("Onboarding.header.download"),
            t!("Onboarding.header.download_detail"),
        ),
        OnboardingScene::GameDownload => (
            lucide_icons::icon_box(),
            t!("Onboarding.header.game"),
            t!("Onboarding.header.game_detail"),
        ),
        OnboardingScene::ResourcePackDownload => (
            lucide_icons::icon_package(),
            t!("Onboarding.header.resource"),
            t!("Onboarding.header.resource_detail"),
        ),
        OnboardingScene::ModDownload => (
            lucide_icons::icon_layers(),
            t!("Onboarding.header.mod"),
            t!("Onboarding.header.mod_detail"),
        ),
        OnboardingScene::ImportPackage => (
            lucide_icons::icon_upload(),
            t!("Onboarding.header.import"),
            t!("Onboarding.header.import_detail"),
        ),
        OnboardingScene::TasksOverview => (
            lucide_icons::icon_activity(),
            t!("Onboarding.header.tasks"),
            t!("Onboarding.header.tasks_detail"),
        ),
        OnboardingScene::ManageOverview => (
            lucide_icons::icon_settings_2(),
            t!("Onboarding.header.manage"),
            t!("Onboarding.header.manage_detail"),
        ),
        OnboardingScene::ManageContent => (
            lucide_icons::icon_package(),
            t!("Onboarding.header.content"),
            t!("Onboarding.header.content_detail"),
        ),
        OnboardingScene::SettingsOverview => (
            lucide_icons::icon_settings(),
            t!("Onboarding.header.settings"),
            t!("Onboarding.header.settings_detail"),
        ),
        OnboardingScene::ToolsOverview => (
            lucide_icons::icon_wrench(),
            t!("Onboarding.header.tools"),
            t!("Onboarding.header.tools_detail"),
        ),
        OnboardingScene::PlatformSetup => {
            #[cfg(target_os = "windows")]
            {
                (
                    lucide_icons::icon_shield_check(),
                    t!("Onboarding.header.windows"),
                    t!("Onboarding.header.windows_detail"),
                )
            }
            #[cfg(target_os = "linux")]
            {
                (
                    lucide_icons::icon_box(),
                    t!("Onboarding.header.linux"),
                    t!("Onboarding.header.linux_detail"),
                )
            }
        }
        OnboardingScene::Finish => (
            lucide_icons::icon_circle_check(),
            t!("Onboarding.header.finish"),
            t!("Onboarding.header.finish_detail"),
        ),
    }
}

fn render_tasks_demo_layer(
    width: f32,
    height: f32,
    colors: &ThemeColors,
    i18n: &I18n,
) -> AnyElement {
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
                        .child(t!("Onboarding.tasks.title")),
                )
                .child(demo_badge(colors, t!("Onboarding.demo.read_only"))),
        )
        .child(demo_task_card(
            colors,
            "Minecraft 1.21.100",
            t!("Tasks.status.running"),
            0.68,
            t!("Onboarding.demo.download_detail"),
            false,
        ))
        .child(demo_task_card(
            colors,
            "Faithful 32x",
            t!("CurseForgeInstall.installing"),
            0.88,
            t!("Onboarding.demo.install_detail"),
            false,
        ))
        .child(demo_task_card(
            colors,
            t!("Onboarding.demo.mod_dependency"),
            t!("Tasks.status.completed"),
            1.0,
            t!("Onboarding.demo.installed_detail"),
            false,
        ))
        .child(demo_task_card(
            colors,
            t!("Onboarding.demo.legacy_import"),
            t!("Tasks.status.error"),
            0.37,
            t!("Onboarding.demo.failure_detail"),
            true,
        ))
        .into_any_element()
}

fn demo_task_card(
    colors: &ThemeColors,
    title: impl Into<SharedString>,
    status: impl Into<SharedString>,
    progress: f32,
    detail: impl Into<SharedString>,
    danger: bool,
) -> Div {
    let title = title.into();
    let status = status.into();
    let detail = detail.into();
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
    i18n: &I18n,
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
                .child(demo_badge(colors, t!("Onboarding.demo.empty_page")))
                .child(demo_version(
                    colors,
                    "Minecraft 1.21.100",
                    t!(
                        "Onboarding.demo.platform_edition",
                        platform = t!("common.uwp"),
                        edition = t!("common.release")
                    ),
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
                        .child(demo_badge(colors, t!("Onboarding.demo.read_only_short"))),
                )
                .child(render_demo_tabs(colors, i18n, content_is_resource))
                .child(if content_is_resource {
                    render_demo_resource_list(colors, i18n).into_any_element()
                } else {
                    render_demo_statistics(colors, i18n).into_any_element()
                }),
        )
        .into_any_element()
}

fn demo_version(
    colors: &ThemeColors,
    name: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    selected: bool,
) -> Div {
    let name = name.into();
    let detail = detail.into();
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

fn render_demo_tabs(colors: &ThemeColors, i18n: &I18n, resource_active: bool) -> Div {
    let tabs = [
        t!("Onboarding.demo.tab_stats"),
        t!("Onboarding.common.mods"),
        t!("Onboarding.common.resource_pack"),
        t!("Onboarding.demo.tab_skins"),
        t!("Onboarding.demo.tab_maps"),
        t!("Onboarding.demo.tab_screenshots"),
        t!("Onboarding.demo.tab_servers"),
    ];
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

fn render_demo_statistics(colors: &ThemeColors, i18n: &I18n) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_wrap()
        .gap(px(8.0))
        .child(demo_stat(
            colors,
            t!("Onboarding.demo.stat_game"),
            "1.21.100",
        ))
        .child(demo_stat(
            colors,
            t!("Onboarding.demo.stat_platform"),
            "UWP",
        ))
        .child(demo_stat(
            colors,
            t!("Onboarding.demo.stat_world"),
            t!("Onboarding.demo.world_count"),
        ))
        .child(demo_stat(
            colors,
            t!("Onboarding.common.mods"),
            t!("Onboarding.demo.mods_enabled"),
        ))
        .child(demo_stat(
            colors,
            t!("Onboarding.common.resource_pack"),
            t!("Onboarding.demo.resource_count"),
        ))
        .child(demo_stat(
            colors,
            t!("Onboarding.demo.stat_mode"),
            t!("Onboarding.demo.isolated"),
        ))
}

fn demo_stat(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Div {
    let label = label.into();
    let value = value.into();
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

fn render_demo_resource_list(colors: &ThemeColors, i18n: &I18n) -> Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(demo_asset_row(
            colors,
            "Faithful 32x Bedrock",
            t!("Onboarding.demo.resource_enabled"),
            "1.21.x",
        ))
        .child(demo_asset_row(
            colors,
            "UI Tweaks",
            t!("Onboarding.demo.resource_enabled"),
            "1.21.x",
        ))
        .child(demo_asset_row(
            colors,
            "Better Animations",
            t!("Onboarding.demo.behavior_disabled"),
            "1.21.100",
        ))
}

fn demo_asset_row(
    colors: &ThemeColors,
    name: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    version: impl Into<SharedString>,
) -> Div {
    let name = name.into();
    let detail = detail.into();
    let version = version.into();
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

fn demo_badge(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    let label = label.into();
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
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    let title = title.into();
    let detail = detail.into();
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

fn step(
    colors: &ThemeColors,
    number: usize,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    let title = title.into();
    let detail = detail.into();
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

fn format_card(
    colors: &ThemeColors,
    format: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    let format = format.into();
    let detail = detail.into();
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

fn intro(colors: &ThemeColors, text: impl Into<SharedString>) -> Div {
    let text = text.into();
    div()
        .w_full()
        .text_size(px(10.0))
        .line_height(px(15.5))
        .text_color(colors.text_secondary)
        .child(text)
}

fn route_badge(colors: &ThemeColors, label: impl Into<SharedString>) -> Div {
    let label = label.into();
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

fn tip(colors: &ThemeColors, text: impl Into<SharedString>) -> Div {
    status(colors, lucide_icons::icon_info(), text, false)
}

fn status(
    colors: &ThemeColors,
    icon: &'static str,
    text: impl Into<SharedString>,
    danger: bool,
) -> Div {
    dynamic_status(colors, icon, text, danger)
}

fn dynamic_status(
    colors: &ThemeColors,
    icon: &'static str,
    text: impl Into<SharedString>,
    danger: bool,
) -> Div {
    let text = text.into();
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
                .child(text),
        )
}

fn platform_summary(
    colors: &ThemeColors,
    i18n: &I18n,
    summary: &crate::ui::onboarding::state::OnboardingPlatformSummary,
) -> Div {
    let title = if summary.ready {
        t!("Onboarding.platform.ready_title")
    } else {
        t!("Onboarding.platform.needs_runtime")
    };
    let detail = if summary.ready {
        t!("Onboarding.platform.ready_detail")
    } else if let Some(reason) = &summary.missing_reason {
        let reason = reason.clone();
        SharedString::from(format!(
            "{}: {reason}",
            t!("Onboarding.platform.missing_reason_label")
        ))
    } else {
        t!("Onboarding.platform.missing_fallback")
    };
    let runtime_value = summary.runner.as_deref().map_or_else(
        || t!("Onboarding.platform.runner_missing"),
        |runner| SharedString::from(runner.to_owned()),
    );
    let local_versions = t!(
        "Onboarding.platform.local_versions_value",
        count = summary.local_versions
    );
    let mut items = div().w_full().flex().flex_col().gap(px(5.0));
    for item in &summary.items {
        let value = match item.label.as_str() {
            "Onboarding.platform.runtime" => runtime_value.clone(),
            "Onboarding.platform.local_versions" => local_versions.clone(),
            _ => SharedString::from(summary.distribution_name.clone()),
        };
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
                                .child(i18n.t_key(item.label)),
                        )
                        .child(
                            div()
                                .text_size(px(8.2))
                                .line_height(px(13.0))
                                .text_color(colors.text_secondary)
                                .child(value),
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
                .child(title),
        )
        .child(
            div()
                .text_size(px(8.2))
                .line_height(px(13.0))
                .text_color(colors.text_secondary)
                .child(detail),
        )
        .child(items)
}

fn primary_button(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let label = label.into();
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

fn secondary_button(colors: &ThemeColors, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
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
