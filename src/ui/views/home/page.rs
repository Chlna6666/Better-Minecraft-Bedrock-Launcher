use crate::core::version::launch_versions::{
    LaunchVersionEntry, sort_launch_versions, sort_versions_by_launch_counts,
};
use crate::plugins::events::{
    CompactBehavior, InjectionLayout, InjectionSlot, PluginInjectionRegistration,
};
use crate::ui::animation::{
    ease_in_cubic, ease_out_back, ease_out_cubic, ease_out_elastic, raw_progress,
    request_animation_frame_if,
};
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::hooks::use_launcher::{LaunchVersionDescriptor, start_launcher};
use crate::ui::hooks::use_local_versions::{
    LocalVersionsSnapshot, launch_version_icon_path, read_local_versions_snapshot,
    use_local_versions,
};
use crate::ui::navigation::{AppRoute, set_route};
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::{DarkColors, LightColors, lerp_theme_colors};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_hooks::{hook_element, hook_render};
use lucide_gpui::icons as lucide_icons;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

const DROPDOWN_ANIMATION_DURATION: Duration = Duration::from_millis(300);
const TITLEBAR_TOP_OFFSET_PX: f32 = 0.0;
const TITLEBAR_HEIGHT_PX: f32 = 60.0;
const TITLEBAR_CLEARANCE_PX: f32 = 12.0;
const HOME_SIDEBAR_DEFAULT_WIDTH_PX: f32 = 304.0;
const HOME_SIDEBAR_MIN_WIDTH_PX: f32 = 248.0;
const HOME_SIDEBAR_MAX_WIDTH_PX: f32 = 328.0;
const HOME_SIDEBAR_DEFAULT_MAX_HEIGHT_PX: f32 = 320.0;

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_color(a: impl Into<Hsla>, b: impl Into<Hsla>, t: f32) -> Hsla {
    let a: Hsla = a.into();
    let b: Hsla = b.into();
    Hsla {
        h: lerp_f32(a.h, b.h, t),
        s: lerp_f32(a.s, b.s, t),
        l: lerp_f32(a.l, b.l, t),
        a: lerp_f32(a.a, b.a, t),
    }
}

fn icon_path(path: &'static str) -> Svg {
    svg().path(path)
}

fn kind_label(i18n: &I18n, kind: &str) -> SharedString {
    match kind.to_ascii_uppercase().as_str() {
        "GDK" => i18n.t("common.gdk"),
        "UWP" => i18n.t("common.uwp"),
        other => SharedString::from(other.to_string()),
    }
}

#[hook_element]
pub(crate) struct HomePageView {
    created_at: Instant,
    versions_started: bool,
    versions_loading: bool,
    versions_error: Option<SharedString>,
    versions: Vec<LaunchVersionEntry>,
    launch_counts: HashMap<SharedString, u32>,
    selected_folder: Option<SharedString>,
    dropdown_open: bool,
    dropdown_anim_at: Option<Instant>,
    dropdown_anim_from_open: bool,
    dropdown_animating: bool,
    active: bool,
    active_at: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl HomePageView {
    fn apply_local_versions_snapshot(&mut self, snapshot: &LocalVersionsSnapshot) {
        self.versions_loading = snapshot.loading;
        self.versions_error = snapshot.error.clone();
        self.versions = snapshot.versions.iter().cloned().collect();
        sort_versions_by_launch_counts(&mut self.versions, |folder| {
            self.launch_counts.get(folder).copied().unwrap_or(0)
        });

        if let Some(selected) = self.selected_folder.clone() {
            let exists = self
                .versions
                .iter()
                .any(|version| version.folder.as_ref() == selected.as_ref());
            if !exists {
                self.selected_folder = self
                    .versions
                    .first()
                    .map(|version| SharedString::from(version.folder.clone()));
            }
        } else {
            self.selected_folder = self
                .versions
                .first()
                .map(|version| SharedString::from(version.folder.clone()));
        }

        if self.versions.is_empty() && !self.versions_loading {
            self.dropdown_open = false;
        }
    }

    pub(crate) fn new(
        prefetched_versions: Option<Vec<LaunchVersionEntry>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let prefetched_seed = prefetched_versions.clone();
        let (versions, selected_folder, versions_started, versions_loading, versions_error) =
            if let Some(mut prefetched_versions) = prefetched_versions {
                sort_launch_versions(&mut prefetched_versions);
                let first = prefetched_versions
                    .first()
                    .map(|version| SharedString::from(version.folder.clone()));
                (prefetched_versions, first, true, false, None)
            } else {
                (Vec::new(), None, false, false, None)
            };

        let subscriptions = vec![
            cx.observe_global::<ThemeState>(|_, cx| cx.notify()),
            cx.observe_global::<I18n>(|_, cx| cx.notify()),
        ];

        let mut this = Self {
            created_at: Instant::now(),
            versions_started,
            versions_loading,
            versions_error,
            versions,
            launch_counts: HashMap::new(),
            selected_folder,
            dropdown_open: false,
            dropdown_anim_at: None,
            dropdown_anim_from_open: false,
            dropdown_animating: false,
            active: false,
            active_at: None,
            _subscriptions: subscriptions,
            __gpui_hooks: RefCell::new(Vec::new()),
            __gpui_hook_index: Cell::new(0),
            __gpui_hook_count: Cell::new(0),
        };

        if let Some(versions) = prefetched_seed.as_deref() {
            crate::ui::hooks::use_local_versions::seed_local_versions(versions, cx);
        }

        let snapshot = read_local_versions_snapshot(cx);
        this.apply_local_versions_snapshot(&snapshot);
        this
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }

        self.active = active;
        if !active {
            self.dropdown_open = false;
            self.dropdown_anim_at = None;
            self.dropdown_animating = false;
            self.active_at = None;
            return;
        }

        self.active_at = Some(Instant::now());
        self.ensure_versions_loaded(true, cx);
        cx.notify();
    }

    fn ensure_versions_loaded(&mut self, force_refresh: bool, cx: &mut Context<Self>) {
        self.versions_started = true;
        crate::ui::hooks::use_local_versions::ensure_local_versions_loaded(force_refresh, cx);
    }

    fn dropdown_factor(&self, now: Instant) -> f32 {
        let Some(started_at) = self.dropdown_anim_at else {
            return if self.dropdown_open { 1.0 } else { 0.0 };
        };

        let progress = raw_progress(now, started_at, DROPDOWN_ANIMATION_DURATION);
        if self.dropdown_anim_from_open {
            1.0 - ease_in_cubic(progress)
        } else {
            ease_out_back(progress, 0.5)
        }
    }

    fn sync_dropdown_animation(&mut self, now: Instant) -> f32 {
        let dropdown_factor = self.dropdown_factor(now).clamp(0.0, 1.0);
        let dropdown_animating = self.dropdown_anim_at.is_some_and(|started_at| {
            now.saturating_duration_since(started_at) < DROPDOWN_ANIMATION_DURATION
        });

        self.dropdown_animating = dropdown_animating;
        if !dropdown_animating {
            self.dropdown_anim_at = None;
        }

        dropdown_factor
    }

    fn begin_dropdown_transition(&mut self, open: bool) {
        if self.dropdown_open == open && !self.dropdown_animating {
            return;
        }

        self.dropdown_anim_from_open = self.dropdown_open;
        self.dropdown_open = open;
        self.dropdown_anim_at = Some(Instant::now());
        self.dropdown_animating = true;
    }

    fn render_dropdown(
        &self,
        kind_labels: &[SharedString],
        theme_colors: &crate::ui::theme::colors::ThemeColors,
        accent: Hsla,
        list_bg: Hsla,
        list_border: Hsla,
        desired_list_h_px: f32,
        dropdown_factor: f32,
        item_height_px: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if dropdown_factor <= 0.001 {
            return div().into_any_element();
        }

        div()
            .w_full()
            .h(px(desired_list_h_px * dropdown_factor))
            .relative()
            .top(px(10.0 * (1.0 - dropdown_factor)))
            .rounded(px(20.0))
            .overflow_hidden()
            .bg(list_bg)
            .border_1()
            .border_color(list_border)
            .shadow(vec![BoxShadow {
                color: Hsla {
                    h: 0.0,
                    s: 0.0,
                    l: 0.0,
                    a: 0.10 * dropdown_factor,
                },
                blur_radius: px(40.0),
                spread_radius: px(-5.0),
                offset: point(px(0.0), px(20.0)),
            }])
            .opacity(dropdown_factor)
            .child(
                div()
                    .id("home-version-list-scroll")
                    .overflow_y_scroll()
                    .scrollbar_width(px(0.0))
                    .h_full()
                    .p(px(6.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .children(self.versions.iter().enumerate().map(|(index, version)| {
                                self.render_dropdown_item(
                                    index,
                                    version,
                                    kind_labels[index].clone(),
                                    theme_colors,
                                    accent,
                                    dropdown_factor,
                                    item_height_px,
                                    cx,
                                )
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_dropdown_item(
        &self,
        index: usize,
        version: &LaunchVersionEntry,
        kind_label_text: SharedString,
        theme_colors: &crate::ui::theme::colors::ThemeColors,
        accent: Hsla,
        dropdown_factor: f32,
        item_height_px: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let delay = (index as f32 * 0.035).min(0.18);
        let progress = ((dropdown_factor - delay) / (1.0 - delay).max(0.001)).clamp(0.0, 1.0);
        let item_factor = if self.dropdown_open {
            ease_out_back(progress, 0.35)
        } else {
            progress
        };
        let selected = self
            .selected_folder
            .as_ref()
            .is_some_and(|folder| folder.as_ref() == version.folder.as_ref());
        let folder = version.folder.clone();
        let item_bg = if selected {
            Hsla { a: 0.10, ..accent }
        } else {
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 0.0,
            }
        };
        let hover_bg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.5,
            a: 0.12,
        };
        let mut icon_bg: Hsla = rgb(0x808080).into();
        icon_bg.a = 0.10;
        let (kind_bg, kind_fg): (Hsla, Hsla) = if version.kind.eq_ignore_ascii_case("UWP") {
            (
                Hsla {
                    a: 0.15,
                    ..rgb(0x06b6d4).into()
                },
                rgb(0x0891b2).into(),
            )
        } else if version.kind.eq_ignore_ascii_case("GDK") {
            (
                Hsla {
                    a: 0.15,
                    ..rgb(0x8b5cf6).into()
                },
                rgb(0x7c3aed).into(),
            )
        } else {
            (
                Hsla {
                    a: 0.15,
                    ..rgb(0x94a3b8).into()
                },
                rgb(0x64748b).into(),
            )
        };
        let icon = launch_version_icon_path(version.name.as_ref());

        div()
            .relative()
            .top(px(10.0 * (1.0 - item_factor)))
            .opacity(item_factor)
            .w_full()
            .h(px(item_height_px))
            .px(px(12.0))
            .py(px(10.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .rounded(px(12.0))
            .bg(item_bg)
            .hover(move |style| if selected { style } else { style.bg(hover_bg) })
            .cursor_pointer()
            .child(
                div()
                    .w(px(40.0))
                    .h(px(40.0))
                    .rounded(px(10.0))
                    .overflow_hidden()
                    .bg(icon_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        img(icon)
                            .size_full()
                            .rounded(px(10.0))
                            .object_fit(ObjectFit::Cover),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(15.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme_colors.text_primary)
                            .child(SharedString::from(version.folder.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(theme_colors.text_secondary)
                                    .child(SharedString::from(version.version.clone())),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(kind_bg)
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(kind_fg)
                                    .child(kind_label_text),
                            ),
                    ),
            )
            .when(selected, |style| {
                style.child(
                    svg()
                        .path(lucide_icons::icon_circle_check())
                        .size(px(16.0))
                        .text_color(Hsla { a: 0.9, ..accent }),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.selected_folder = Some(folder.clone().into());
                    this.begin_dropdown_transition(false);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn render_launch_primary(
        &self,
        launch_label: SharedString,
        launch_sub: SharedString,
        selected_version: Option<LaunchVersionDescriptor>,
        loading: bool,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id("launch-primary")
            .flex_1()
            .h_full()
            .px(px(24.0))
            .relative()
            .flex()
            .items_center()
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .when(loading, |style| {
                                let angle =
                                    now.saturating_duration_since(self.created_at).as_secs_f32()
                                        * 2.0
                                        * std::f32::consts::PI
                                        * 1.5;
                                style.child(
                                    icon_path(lucide_icons::icon_loader_circle())
                                        .size(px(18.0))
                                        .text_color(rgb(0xffffff))
                                        .with_transformation(Transformation::rotate(radians(
                                            angle,
                                        ))),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xffffff))
                                    .child(launch_label),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(Hsla {
                                h: 0.0,
                                s: 0.0,
                                l: 1.0,
                                a: 0.85,
                            })
                            .child(launch_sub),
                    ),
            )
            .child({
                let icon = if loading {
                    let angle = now.saturating_duration_since(self.created_at).as_secs_f32()
                        * 2.0
                        * std::f32::consts::PI
                        * 1.0;
                    icon_path(lucide_icons::icon_loader_circle())
                        .size(px(40.0))
                        .text_color(rgb(0xffffff))
                        .with_transformation(Transformation::rotate(radians(angle)))
                } else {
                    icon_path(lucide_icons::icon_play())
                        .size(px(48.0))
                        .text_color(rgb(0xffffff))
                };
                let opacity = if loading { 0.20 } else { 0.10 };
                div()
                    .absolute()
                    .right(px(10.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .flex()
                    .items_center()
                    .opacity(opacity)
                    .child(icon)
            })
            .hover(|style| {
                style.bg(Hsla {
                    h: 0.0,
                    s: 0.0,
                    l: 1.0,
                    a: 0.08,
                })
            })
            .active(|style| style.opacity(0.75))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    if let Some(version) = selected_version.clone() {
                        let _ = start_launcher(version, cx);
                    } else {
                        set_route(cx, AppRoute::Download);
                    }

                    if let Some(selected) = this.selected_folder.clone() {
                        let next = this.launch_counts.get(&selected).copied().unwrap_or(0) + 1;
                        this.launch_counts.insert(selected, next);
                        sort_versions_by_launch_counts(&mut this.versions, |folder| {
                            this.launch_counts.get(folder).copied().unwrap_or(0)
                        });
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn render_launch_secondary(
        &self,
        is_empty: bool,
        dropdown_factor: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chevron = if is_empty {
            icon_path(lucide_icons::icon_download())
                .size(px(20.0))
                .text_color(rgb(0xffffff))
        } else {
            icon_path(lucide_icons::icon_chevron_down())
                .size(px(16.0))
                .text_color(rgb(0xffffff))
                .with_transformation(Transformation::rotate(radians(
                    dropdown_factor * std::f32::consts::PI,
                )))
        };

        div()
            .id("launch-secondary")
            .w(px(60.0))
            .h_full()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| {
                style.bg(Hsla {
                    h: 0.0,
                    s: 0.0,
                    l: 1.0,
                    a: 0.12,
                })
            })
            .active(|style| style.opacity(0.75))
            .when(!is_empty, |style| {
                style.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.begin_dropdown_transition(!this.dropdown_open);
                        cx.notify();
                    }),
                )
            })
            .child(chevron)
            .into_any_element()
    }

    fn render_home_sidebar(
        &self,
        layout: InjectionLayout,
        theme_colors: &crate::ui::theme::colors::ThemeColors,
        theme_dark: bool,
        entrance_eased: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let launch_bar_height_px = 72.0;
        let launcher_width_px = 288.0;
        let side_margin_px = 40.0;
        let window_width_px = window.bounds().size.width / px(1.0);
        let window_height_px = window.bounds().size.height / px(1.0);
        let compact = window_width_px < 820.0;
        let requested_width_px = f32::from(
            layout
                .preferred_width
                .unwrap_or(HOME_SIDEBAR_DEFAULT_WIDTH_PX as u16),
        );
        let min_width_px = f32::from(layout.min_width.unwrap_or(HOME_SIDEBAR_MIN_WIDTH_PX as u16))
            .clamp(HOME_SIDEBAR_MIN_WIDTH_PX, HOME_SIDEBAR_MAX_WIDTH_PX);
        let max_width_px = f32::from(layout.max_width.unwrap_or(HOME_SIDEBAR_MAX_WIDTH_PX as u16))
            .clamp(min_width_px, HOME_SIDEBAR_MAX_WIDTH_PX);
        let available_width_px = window_width_px - launcher_width_px - side_margin_px * 3.0;
        let width_px = requested_width_px
            .clamp(min_width_px, max_width_px)
            .min(available_width_px.max(min_width_px));
        let requested_max_height_px = f32::from(
            layout
                .max_height
                .unwrap_or(HOME_SIDEBAR_DEFAULT_MAX_HEIGHT_PX as u16),
        )
        .clamp(180.0, 460.0);
        let top_px = TITLEBAR_TOP_OFFSET_PX
            + TITLEBAR_HEIGHT_PX
            + TITLEBAR_CLEARANCE_PX
            + if compact { 12.0 } else { 18.0 };
        let launch_area_bottom_px =
            side_margin_px + launch_bar_height_px + if compact { 34.0 } else { 28.0 };
        let available_height_px = (window_height_px - top_px - launch_area_bottom_px).max(160.0);
        let max_height_px = requested_max_height_px.min(available_height_px);

        let left_px = if compact {
            side_margin_px.min(24.0)
        } else {
            side_margin_px
        };

        let left_offset_px = -40.0 * (1.0 - entrance_eased);
        let panel_left = left_px + left_offset_px;

        let mut panel_bg = theme_colors.settings_panel_bg;
        panel_bg.a = if theme_dark { 0.70 } else { 0.76 };
        let mut panel_border = theme_colors.border;
        panel_border.a = if theme_dark { 0.28 } else { 0.34 };

        let mut panel = div()
            .id("home-plugin-sidebar")
            .absolute()
            .left(px(panel_left))
            .top(px(top_px))
            .w(px(width_px))
            .max_h(px(max_height_px))
            .opacity(entrance_eased.clamp(0.0, 1.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(8.0))
            .rounded(px(8.0))
            .overflow_hidden()
            .border_1()
            .border_color(panel_border)
            .bg(panel_bg)
            .shadow(vec![
                BoxShadow {
                    color: Hsla {
                        h: 0.0,
                        s: 0.0,
                        l: 0.0,
                        a: if theme_dark { 0.20 } else { 0.10 },
                    },
                    blur_radius: px(24.0),
                    spread_radius: px(-12.0),
                    offset: point(px(0.0), px(12.0)),
                },
                BoxShadow {
                    color: Hsla {
                        a: 0.10,
                        ..theme_colors.accent
                    },
                    blur_radius: px(20.0),
                    spread_radius: px(-18.0),
                    offset: point(px(0.0), px(6.0)),
                },
            ]);

        if matches!(layout.compact_behavior, CompactBehavior::Scroll) {
            panel = panel.overflow_y_scrollbar().scrollbar_width(px(0.0));
        }

        let injections =
            crate::plugins::runtime::render_injections(cx, InjectionSlot::HomeSidebar, Some("/"));
        for injection in injections {
            panel = panel.child(crate::plugins::ui_dsl::render_validated_view_tree(
                &injection.tree,
                &injection.plugin_id,
                Some("/"),
                window,
                cx,
            ));
        }

        panel.into_any_element()
    }
}

fn merged_home_sidebar_layout(registrations: &[PluginInjectionRegistration]) -> InjectionLayout {
    let mut result = InjectionLayout::default();
    let mut found = false;
    for registration in registrations {
        let layout = registration.layout.unwrap_or_default();
        if !found || layout.priority >= result.priority {
            result = layout;
            found = true;
        }
    }
    result
}

#[hook_render]
impl Render for HomePageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let local_versions = use_local_versions(self, cx);
        self.apply_local_versions_snapshot(&local_versions);

        if !self.active {
            return div().into_any_element();
        }

        let now = Instant::now();
        let theme = cx.global::<ThemeState>();
        let theme_k = theme.factor(now);
        let theme_dark = theme.target_dark;
        let theme_colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme_k,
            theme.accent,
        );
        let dropdown_factor = self.sync_dropdown_animation(now);
        let dropdown_visible = self.dropdown_open || self.dropdown_animating;
        let i18n = cx.global::<I18n>();

        let entrance_factor = self
            .active_at
            .map(|at| {
                let elapsed = now.saturating_duration_since(at);
                let duration = Duration::from_millis(600);
                if elapsed < duration {
                    raw_progress(now, at, duration)
                } else {
                    1.0
                }
            })
            .unwrap_or(1.0);
        let entrance_eased = ease_out_elastic(entrance_factor);

        let is_empty = self.versions.is_empty() && !self.versions_loading;
        let selected_version = self.selected_folder.as_ref().and_then(|folder| {
            self.versions
                .iter()
                .find(|version| version.folder.as_ref() == folder.as_ref())
        });
        let selected_launch_version = selected_version.map(|version| LaunchVersionDescriptor {
            folder: version.folder.clone().into(),
            name: version.name.clone().into(),
            version: version.version.clone().into(),
            kind: version.kind.clone().into(),
            path: version.path.clone().into(),
            launch_args: None,
        });
        let launch_label = if self.versions_loading {
            SharedString::from("加载中")
        } else if is_empty {
            i18n.t("common.not_installed")
        } else {
            i18n.t("Sidebar.launch")
        };
        let launch_sub = if self.versions_loading {
            SharedString::from("请稍候...")
        } else if is_empty {
            i18n.t("common.go_download")
        } else if let Some(version) = selected_version {
            version.version.clone().into()
        } else {
            i18n.t("common.all_versions")
        };
        let kind_labels = dropdown_visible.then(|| {
            self.versions
                .iter()
                .map(|version| kind_label(i18n, version.kind.as_ref()))
                .collect::<Vec<_>>()
        });

        let accent = theme_colors.accent;
        let launch_bg = Hsla { a: 1.0, ..accent };
        let divider = Hsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 0.15,
        };
        let mut glass_bg_light: Hsla = rgb(0xffffff).into();
        glass_bg_light.a = 0.85;
        let mut glass_bg_dark: Hsla = rgb(0x0f172a).into();
        glass_bg_dark.a = 0.85;
        let list_bg = lerp_color(glass_bg_light, glass_bg_dark, theme_k);
        let mut list_border: Hsla = rgb(0xffffff).into();
        list_border.a = 0.10;
        let launch_bar_height_px = 72.0;
        let launch_bar_gap_px = 10.0;
        let launch_bar_bottom_margin_px = 40.0;
        let launcher_width_px = 288.0;
        let window_height_px = window.bounds().size.height / px(1.0);
        let titlebar_reserved_height_px =
            TITLEBAR_TOP_OFFSET_PX + TITLEBAR_HEIGHT_PX + TITLEBAR_CLEARANCE_PX;
        let available_list_h_px = (window_height_px
            - titlebar_reserved_height_px
            - launch_bar_bottom_margin_px
            - launch_bar_height_px
            - launch_bar_gap_px)
            .max(0.0);
        let item_height_px = 60.0;
        let list_padding_px = 12.0;
        let desired_list_h_px = (self.versions.len() as f32 * item_height_px + list_padding_px)
            .min(available_list_h_px);

        let mut launcher_root = div()
            .absolute()
            .right(px(40.0))
            .bottom(px(40.0 - 20.0 * (1.0 - entrance_eased)))
            .opacity(entrance_eased)
            .w(px(launcher_width_px))
            .flex()
            .flex_col()
            .items_end()
            .gap(px(10.0));

        if !self.versions_loading
            && !self.dropdown_open
            && let Some(message) = self.versions_error.clone()
        {
            launcher_root = launcher_root.child(
                div()
                    .w_full()
                    .rounded_xl()
                    .bg(lerp_color(
                        Hsla {
                            a: 0.12,
                            ..theme_colors.danger
                        },
                        Hsla {
                            a: 0.18,
                            ..theme_colors.danger
                        },
                        theme_k,
                    ))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.45,
                        ..theme_colors.danger
                    })
                    .p(px(10.0))
                    .text_size(px(11.0))
                    .text_color(theme_colors.text_primary)
                    .whitespace_normal()
                    .child(message),
            );
        }

        let loading_pulse = if self.versions_loading {
            let pulse_progress = (now
                .duration_since(self.active_at.unwrap_or(now))
                .as_secs_f32()
                * std::f32::consts::PI
                * 1.5)
                .sin();
            0.6 + 0.4 * (pulse_progress * 0.5 + 0.5)
        } else {
            1.0
        };

        let launch_bar = div()
            .w_full()
            .h(px(72.0))
            .rounded(px(24.0))
            .overflow_hidden()
            .bg(launch_bg)
            .opacity(loading_pulse)
            .child(div().absolute().inset_0())
            .shadow(vec![BoxShadow {
                color: Hsla {
                    a: 0.12,
                    ..launch_bg
                },
                blur_radius: px(16.0),
                spread_radius: px(-6.0),
                offset: point(px(0.0), px(4.0)),
            }])
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .child(self.render_launch_primary(
                        launch_label,
                        launch_sub.clone(),
                        selected_launch_version,
                        self.versions_loading,
                        now,
                        cx,
                    ))
                    .child(div().w(px(1.0)).h_full().bg(divider))
                    .child(self.render_launch_secondary(is_empty, dropdown_factor, cx)),
            );

        if !is_empty && dropdown_visible {
            launcher_root = launcher_root.child(self.render_dropdown(
                kind_labels.as_deref().unwrap_or(&[]),
                &theme_colors,
                accent,
                list_bg,
                list_border,
                desired_list_h_px,
                dropdown_factor,
                item_height_px,
                cx,
            ));
        }

        launcher_root = launcher_root.child(launch_bar);

        let mut overlay = div().absolute().inset_0().child(launcher_root);
        let sidebar_registrations = crate::plugins::runtime::injection_registrations(
            cx,
            InjectionSlot::HomeSidebar,
            Some("/"),
        );
        if !sidebar_registrations.is_empty() {
            overlay = overlay.child(self.render_home_sidebar(
                merged_home_sidebar_layout(&sidebar_registrations),
                &theme_colors,
                theme_dark,
                entrance_eased,
                window,
                cx,
            ));
        }

        request_animation_frame_if(
            window,
            self.dropdown_animating || entrance_factor < 1.0 || self.versions_loading,
        );

        overlay.into_any_element()
    }
}
