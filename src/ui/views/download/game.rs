use crate::tasks::task_manager::{self, TaskSnapshot};
use crate::ui::animation::repeating_linear_motion;
use crate::ui::components::icon::themed_icon;
use crate::ui::components::input::{Input, InputState};
use crate::ui::components::scroll::ScrollableElement;
use crate::ui::components::toast;
use crate::ui::components::virtual_list::compute_virtual_list_plan;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::download::state::{
    DownloadChannelFilter, DownloadPageState, GameDialogCdnResult, GameDialogKind, GameDialogState,
};
use crate::ui::views::download::{
    DownloadTab, GamePanelObserveSignature, build_game_panel_observe_signature,
};
use crate::utils::file_ops;
use gpui::AnimationExt;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use super::common::{status_card, wait_task_finished};

const GAME_ROW_PITCH_PX: f32 = 96.0;
const GAME_ROW_OVERSCAN: usize = 1;
const GAME_ROW_HEAVY_BUDGET: usize = 6;

#[derive(Clone, PartialEq, Eq)]
struct GamePageRowProps {
    version: SharedString,
    package_id: SharedString,
    version_type: i32,
    archival_status: Option<i32>,
    meta_present: bool,
    md5: Option<SharedString>,
    is_gdk: bool,
}

#[derive(Clone)]
struct GameVisibleRowProps {
    version: SharedString,
    package_id: SharedString,
    badge: &'static str,
    is_gdk: bool,
    action_label: &'static str,
    local_ready: bool,
    disabled: bool,
    md5: Option<SharedString>,
    local_path: Option<SharedString>,
    active_task_running: bool,
    active_snapshot: Option<Arc<TaskSnapshot>>,
}

type GamePanelRenderSignature = (
    usize,
    SharedString,
    SharedString,
    SharedString,
    DownloadChannelFilter,
    usize,
    usize,
);

pub(crate) struct DownloadGamePanelView {
    _subscriptions: Vec<Subscription>,
    last_observed_signature: GamePanelObserveSignature,
    last_observed_dialog_signature: GameDialogObserveSignature,
}

#[derive(Clone, PartialEq, Eq)]
struct GameDialogObserveSignature {
    dialog_kind: Option<GameDialogKind>,
    dialog_version: SharedString,
    dialog_package_id: SharedString,
    dialog_file_name: SharedString,
    dialog_local_path: SharedString,
    dialog_cdn_loading: bool,
    dialog_cdn_error: SharedString,
    dialog_cdn_result_count: usize,
    selected_cdn_base: SharedString,
}

impl DownloadGamePanelView {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let last_observed_signature = cx.read_global(|state: &DownloadPageState, _cx| {
            build_game_panel_observe_signature(state)
        });
        let last_observed_dialog_signature = cx.read_global(|state: &DownloadPageState, _cx| {
            build_game_dialog_observe_signature(state)
        });

        let mut subscriptions = vec![cx.observe_global::<DownloadPageState>(|this, cx| {
            let (game_signature, dialog_signature) =
                cx.read_global(|state: &DownloadPageState, _cx| {
                    (
                        build_game_panel_observe_signature(state),
                        build_game_dialog_observe_signature(state),
                    )
                });

            if this.last_observed_signature != game_signature {
                this.last_observed_signature = game_signature;
                if cx.global::<DownloadPageState>().tab == DownloadTab::Game {
                    cx.notify();
                }
                return;
            }

            if this.last_observed_dialog_signature != dialog_signature {
                this.last_observed_dialog_signature = dialog_signature;
                if cx.global::<DownloadPageState>().tab == DownloadTab::Game {
                    cx.notify();
                }
            }
        })];

        subscriptions.shrink_to_fit();

        Self {
            _subscriptions: subscriptions,
            last_observed_signature,
            last_observed_dialog_signature,
        }
    }
}

fn build_game_dialog_observe_signature(state: &DownloadPageState) -> GameDialogObserveSignature {
    let dialog = state.game_dialog.as_ref();
    GameDialogObserveSignature {
        dialog_kind: dialog.map(|dialog| dialog.kind),
        dialog_version: dialog
            .map(|dialog| dialog.version.clone())
            .unwrap_or_else(|| SharedString::from("")),
        dialog_package_id: dialog
            .map(|dialog| dialog.package_id.clone())
            .unwrap_or_else(|| SharedString::from("")),
        dialog_file_name: dialog
            .map(|dialog| dialog.file_name.clone())
            .unwrap_or_else(|| SharedString::from("")),
        dialog_local_path: dialog
            .and_then(|dialog| dialog.local_path.clone())
            .unwrap_or_else(|| SharedString::from("")),
        dialog_cdn_loading: state.game_dialog_cdn_loading,
        dialog_cdn_error: state
            .game_dialog_cdn_error
            .clone()
            .unwrap_or_else(|| SharedString::from("")),
        dialog_cdn_result_count: state.game_dialog_cdn_results.len(),
        selected_cdn_base: state
            .game_dialog_selected_cdn_base
            .clone()
            .unwrap_or_else(|| SharedString::from("")),
    }
}

impl Render for DownloadGamePanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let theme = cx.global::<crate::ui::state::theme::ThemeState>();
        let colors = crate::ui::theme::colors::lerp_theme_colors(
            &crate::ui::theme::colors::LightColors::colors(),
            &crate::ui::theme::colors::DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );

        render_game_panel(window, cx, &colors)
    }
}

#[derive(Default)]
struct GamePanelRenderCache {
    last_signature: Option<GamePanelRenderSignature>,
    filtered_total: usize,
    total_pages: usize,
    page_index: usize,
    page_rows: Vec<GamePageRowProps>,
}

fn build_game_panel_render_signature(state: &DownloadPageState) -> GamePanelRenderSignature {
    (
        state.versions.len(),
        state
            .versions
            .first()
            .map(|version| version.package_id.clone())
            .unwrap_or_else(|| SharedString::from("")),
        state
            .versions
            .last()
            .map(|version| version.package_id.clone())
            .unwrap_or_else(|| SharedString::from("")),
        state.search_query.clone(),
        state.channel_filter,
        state.page_index,
        state.page_size,
    )
}

fn rebuild_game_panel_render_cache(
    state: &DownloadPageState,
    signature: GamePanelRenderSignature,
) -> GamePanelRenderCache {
    let query = state.search_query.to_lowercase();
    let trimmed_query = query.trim().to_owned();
    let matches_filter = |version: &crate::ui::views::download::state::DownloadRemoteVersion| {
        let channel_matches = match state.channel_filter {
            DownloadChannelFilter::All => true,
            DownloadChannelFilter::Release => version.version_type == 0,
            DownloadChannelFilter::Beta => version.version_type == 1,
            DownloadChannelFilter::Preview => version.version_type >= 2,
        };
        if !channel_matches {
            return false;
        }
        if trimmed_query.is_empty() {
            return true;
        }
        version.version.to_lowercase().contains(&trimmed_query)
    };

    let filtered_total = state
        .versions
        .iter()
        .filter(|version| matches_filter(version))
        .count();
    let page_size = state.page_size.max(1);
    let total_pages = (filtered_total + page_size - 1) / page_size;
    let page_index = state.page_index.min(total_pages.saturating_sub(1));
    let start_index = page_index * page_size;
    let end_index = (start_index + page_size).min(filtered_total);

    let mut seen_index = 0usize;
    let mut page_rows = Vec::with_capacity(end_index.saturating_sub(start_index));
    for version in &state.versions {
        if !matches_filter(version) {
            continue;
        }
        if seen_index < start_index {
            seen_index += 1;
            continue;
        }
        if seen_index >= end_index {
            break;
        }
        seen_index += 1;
        page_rows.push(GamePageRowProps {
            version: version.version.clone(),
            package_id: version.package_id.clone(),
            version_type: version.version_type,
            archival_status: version.archival_status,
            meta_present: version.meta_present,
            md5: version.md5.clone(),
            is_gdk: version.is_gdk,
        });
    }

    GamePanelRenderCache {
        last_signature: Some(signature),
        filtered_total,
        total_pages,
        page_index,
        page_rows,
    }
}

fn render_game_loading_placeholder(colors: &ThemeColors, row_count: usize) -> Div {
    let skeleton_bar = |width: Pixels, height: Pixels| {
        div().w(width).h(height).rounded(px(999.)).bg(Hsla {
            a: 0.08,
            ..colors.text_secondary
        })
    };

    let skeleton_shimmer = || {
        div()
            .absolute()
            .top(px(0.))
            .bottom(px(0.))
            .w(px(120.))
            .bg(Hsla {
                a: 0.24,
                ..colors.surface
            })
            .with_animation(
                "game-skeleton-shimmer",
                repeating_linear_motion(Duration::from_millis(1400)),
                |this, t| this.left(px(-160.0 + t * 420.0)),
            )
            .into_any_element()
    };

    let skeleton_row = || {
        div()
            .w_full()
            .min_h(px(77.))
            .rounded(px(8.))
            .px(px(16.))
            .py(px(14.))
            .relative()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .flex_1()
                    .min_w(px(0.))
                    .child(skeleton_bar(px(240.), px(16.)))
                    .child(skeleton_bar(px(340.), px(10.)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(skeleton_bar(px(68.), px(10.)))
                            .child(skeleton_bar(px(96.), px(12.))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(skeleton_bar(px(78.), px(16.)))
                    .child(div().w(px(88.)).h(px(28.)).rounded(px(999.)).bg(Hsla {
                        a: 0.10,
                        ..colors.accent
                    })),
            )
            .child(skeleton_shimmer())
    };

    let row_count = row_count.clamp(4, 8);

    let mut placeholder = div()
        .size_full()
        .flex()
        .flex_col()
        .min_h(px(0.))
        .min_w(px(0.))
        .p(px(18.))
        .gap(px(12.));

    for _ in 0..row_count {
        placeholder = placeholder.child(skeleton_row());
    }

    placeholder
}

fn stable_game_row_element_id(package_id: &SharedString) -> u64 {
    render_fingerprint(package_id)
}

fn render_game_loading_placeholder_aligned(colors: &ThemeColors, state: &DownloadPageState) -> Div {
    let skeleton_bar = |width: Pixels, height: Pixels| {
        div().w(width).h(height).rounded(px(999.)).bg(Hsla {
            a: 0.10,
            ..colors.text_secondary
        })
    };

    let skeleton_shimmer = || {
        div()
            .absolute()
            .top(px(0.))
            .bottom(px(0.))
            .w(px(120.))
            .bg(Hsla {
                a: 0.24,
                ..colors.surface
            })
            .with_animation(
                "game-skeleton-shimmer-aligned",
                repeating_linear_motion(Duration::from_millis(1400)),
                |this, t| this.left(px(-160.0 + t * 420.0)),
            )
            .into_any_element()
    };

    let skeleton_row = || {
        div()
            .w_full()
            .h(px(76.))
            .px(px(24.))
            .py(px(12.))
            .relative()
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_between()
            .bg(Hsla {
                a: 0.0,
                ..colors.surface
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(0.))
                    .min_w(px(0.))
                    .flex_1()
                    .child(div().w(px(64.)).flex().items_center().child(
                        div().w(px(42.)).h(px(42.)).rounded(px(10.)).bg(Hsla {
                            a: 0.12,
                            ..colors.surface
                        }),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .pr(px(16.))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(skeleton_bar(px(240.), px(16.)))
                                    .child(div().w(px(74.)).h(px(20.)).rounded(px(6.)).bg(Hsla {
                                        a: 0.08,
                                        ..colors.text_secondary
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(skeleton_bar(px(68.), px(10.)))
                                    .child(skeleton_bar(px(96.), px(12.))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(skeleton_bar(px(78.), px(16.)))
                    .child(div().w(px(88.)).h(px(28.)).rounded(px(999.)).bg(Hsla {
                        a: 0.12,
                        ..colors.accent
                    })),
            )
            .child(skeleton_shimmer())
    };

    let row_count = state.page_size.max(1).min(8) as usize;

    let rows = (0..row_count)
        .map(|_| {
            div().child(skeleton_row()).child(div().h(px(1.)).bg(Hsla {
                a: 0.06,
                ..colors.border
            }))
        })
        .collect::<Vec<_>>();

    div()
        .size_full()
        .flex()
        .flex_col()
        .min_h(px(0.))
        .min_w(px(0.))
        .child(
            div()
                .id("download-game-rows-scroll")
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
                .overflow_y_scroll()
                .scrollbar_width(px(0.))
                .track_scroll(&state.game_rows_scroll)
                .child(div().flex().flex_col().min_w(px(0.)).children(rows)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(div().h(px(1.)).bg(Hsla {
                    a: 0.12,
                    ..colors.border
                }))
                .child(
                    div()
                        .px(px(14.))
                        .py(px(11.))
                        .bg(Hsla {
                            a: 0.30,
                            ..colors.surface
                        })
                        .child(div().w_full().h(px(32.)).rounded(px(8.)).bg(Hsla {
                            a: 0.08,
                            ..colors.text_secondary
                        })),
                ),
        )
}

pub(super) fn render_game_panel(window: &mut Window, cx: &mut App, colors: &ThemeColors) -> Div {
    let cache = window.use_keyed_state("download-game-panel-cache", cx, |_, _| {
        GamePanelRenderCache::default()
    });
    let render_signature =
        cx.read_global(|state: &DownloadPageState, _cx| build_game_panel_render_signature(state));
    let cache_needs_rebuild = cache.read(cx).last_signature.as_ref() != Some(&render_signature);
    if cache_needs_rebuild {
        let rebuilt_cache = cx.read_global(|state: &DownloadPageState, _cx| {
            rebuild_game_panel_render_cache(state, render_signature.clone())
        });
        cache.update(cx, |cached, _| {
            *cached = rebuilt_cache;
        });
    }

    let state = cx.global::<DownloadPageState>();
    let mut panel = div()
        .size_full()
        .flex()
        .flex_col()
        .min_h(px(0.))
        .min_w(px(0.));

    if (state.loading || state.force_refresh_next) && state.versions.is_empty() {
        return panel.child(render_game_loading_placeholder_aligned(colors, state));
    }

    if let Some(err) = state.error.clone() {
        return panel.child(div().p(px(18.)).child(status_card(
            colors,
            &format!("加载失败: {err}"),
            Some(colors.danger),
        )));
    }

    if state.versions.is_empty() {
        return panel.child(
            div()
                .p(px(18.))
                .child(status_card(colors, "暂无可用版本", None)),
        );
    }

    let (filtered_total, total_pages, page_index, page_rows) = {
        let cached = cache.read(cx);
        (
            cached.filtered_total,
            cached.total_pages,
            cached.page_index,
            cached.page_rows.clone(),
        )
    };

    let virtual_list_plan = compute_virtual_list_plan(
        page_rows.len(),
        GAME_ROW_PITCH_PX,
        state.game_rows_scroll.offset().y,
        state.game_rows_scroll.bounds().size.height,
        GAME_ROW_OVERSCAN,
        GAME_ROW_HEAVY_BUDGET,
    );

    let prepare_started_at = Instant::now();
    let mut rows_body = div().flex().flex_col().min_w(px(0.));
    if virtual_list_plan.render_slice.top_spacer > px(0.) {
        rows_body = rows_body.child(div().h(virtual_list_plan.render_slice.top_spacer));
    }

    let visible_start = virtual_list_plan.render_slice.start_index;
    let visible_end = virtual_list_plan
        .render_slice
        .end_index
        .min(page_rows.len());
    let visible_rows = page_rows[visible_start..visible_end]
        .iter()
        .enumerate()
        .map(|(row_offset, row)| {
            let virtual_index = visible_start + row_offset;
            let is_heavy_row = virtual_list_plan.heavy_slice.contains(virtual_index);
            let local_path = state
                .local_path_by_package
                .get(&row.package_id)
                .cloned()
                .or_else(|| {
                    let file_name = SharedString::from(format!(
                        "{}{}",
                        row.version,
                        if row.is_gdk { ".msixvc" } else { ".appx" }
                    ));
                    state
                        .local_files
                        .contains(&file_name)
                        .then(|| local_game_file_path(&file_name))
                });
            let local_ready = local_path.is_some();
            let disabled =
                !row.meta_present || matches!(row.archival_status, Some(0 | 1)) || state.loading;
            let badge = if row.version_type == 0 {
                "正式"
            } else {
                "预览"
            };
            let action_label = if local_ready { "安装" } else { "下载" };
            let (download_task, extract_task) = state
                .operations_by_package
                .get(&row.package_id)
                .map(|op| (op.download_task_id.clone(), op.extract_task_id.clone()))
                .unwrap_or((None, None));
            let active_task_running = download_task.is_some() || extract_task.is_some();
            let active_snapshot = if is_heavy_row {
                download_task
                    .as_ref()
                    .and_then(|id| task_manager::get_snapshot_arc(id.as_ref()))
                    .or_else(|| {
                        extract_task
                            .as_ref()
                            .and_then(|id| task_manager::get_snapshot_arc(id.as_ref()))
                    })
            } else {
                None
            };

            GameVisibleRowProps {
                version: row.version.clone(),
                package_id: row.package_id.clone(),
                badge,
                is_gdk: row.is_gdk,
                action_label,
                local_ready,
                disabled,
                md5: row.md5.clone(),
                local_path,
                active_task_running,
                active_snapshot,
            }
        })
        .collect::<Vec<_>>();

    let prepare_elapsed_ms = prepare_started_at.elapsed().as_secs_f64() * 1000.0;
    if prepare_elapsed_ms >= 4.0 {
        tracing::debug!(
            "game rows prepare slow: elapsed_ms={prepare_elapsed_ms:.3} total_rows={} render_start={} render_len={} visible_start={} visible_len={} heavy_start={} heavy_len={}",
            page_rows.len(),
            virtual_list_plan.render_slice.start_index,
            virtual_list_plan.render_slice.visible_len(),
            virtual_list_plan.visible_slice.start_index,
            virtual_list_plan.visible_slice.len(),
            virtual_list_plan.heavy_slice.start_index,
            virtual_list_plan.heavy_slice.len()
        );
    }

    let render_started_at = Instant::now();
    for row in &visible_rows {
        rows_body = rows_body
            .child(render_version_row(
                colors,
                row.version.clone(),
                row.badge,
                row.is_gdk,
                row.action_label,
                row.local_ready,
                row.disabled,
                row.package_id.clone(),
                row.md5.clone(),
                row.local_path.clone(),
                row.active_task_running,
                row.active_snapshot.clone(),
            ))
            .child(div().h(px(12.)));
    }

    let render_elapsed_ms = render_started_at.elapsed().as_secs_f64() * 1000.0;
    if render_elapsed_ms >= 6.0 {
        tracing::debug!(
            "game rows render slow: elapsed_ms={render_elapsed_ms:.3} total_rows={} render_start={} render_len={} visible_start={} visible_len={} heavy_start={} heavy_len={}",
            page_rows.len(),
            virtual_list_plan.render_slice.start_index,
            virtual_list_plan.render_slice.visible_len(),
            virtual_list_plan.visible_slice.start_index,
            virtual_list_plan.visible_slice.len(),
            virtual_list_plan.heavy_slice.start_index,
            virtual_list_plan.heavy_slice.len()
        );
    }

    if virtual_list_plan.render_slice.bottom_spacer > px(0.) {
        rows_body = rows_body.child(div().h(virtual_list_plan.render_slice.bottom_spacer));
    }

    let rows = div()
        .id("download-game-rows-scroll")
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .px(px(20.))
        .py(px(16.))
        .overflow_y_scroll()
        .scrollbar_width(px(0.))
        .track_scroll(&state.game_rows_scroll)
        .child(rows_body);

    let footer = div()
        .flex()
        .flex_col()
        .child(div().h(px(1.)).bg(Hsla {
            a: 0.12,
            ..colors.border
        }))
        .child(
            div()
                .px(px(14.))
                .py(px(11.))
                .bg(Hsla {
                    a: 0.30,
                    ..colors.surface
                })
                .child(render_pager(
                    window,
                    cx,
                    colors,
                    page_rows.len(),
                    filtered_total,
                )),
        );

    panel.child(rows).child(footer)
}

fn render_pager(
    window: &mut Window,
    cx: &mut App,
    colors: &ThemeColors,
    showing: usize,
    total: usize,
) -> Div {
    let state = cx.global::<DownloadPageState>();
    let page_index = state.page_index;
    let page_size = state.page_size;
    let total_pages = (total + page_size - 1) / page_size;
    let page_index = page_index.min(total_pages.saturating_sub(1));
    let page_jump_input = state.page_jump_input.clone();

    if total_pages <= 1 {
        return div().w_full().h(px(0.));
    }

    let total_pages = total_pages.max(1);
    if let Some(input) = &page_jump_input {
        let placeholder = format!("{}/{}", page_index + 1, total_pages);
        let _ = input.update(cx, |st, cx| {
            st.set_placeholder(SharedString::from(placeholder), window, cx);
        });
    }

    let prev_enabled = page_index > 0;
    let next_enabled = page_index + 1 < total_pages;

    let nav_btn = |id: &'static str,
                   icon: &'static str,
                   enabled: bool,
                   on_click: Box<dyn Fn(&mut DownloadPageState)>| {
        div()
            .id(id)
            .min_w(px(32.))
            .h(px(32.))
            .rounded(px(6.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .text_color(if enabled {
                colors.text_primary
            } else {
                colors.text_muted
            })
            .when(!enabled, |this| this.opacity(0.35))
            .child(themed_icon(
                icon,
                16.0,
                if enabled {
                    colors.text_primary
                } else {
                    colors.text_muted
                },
            ))
            .hover(|s| {
                s.bg(Hsla {
                    a: if colors.bg.l < 0.5 { 0.12 } else { 0.08 },
                    ..colors.text_primary
                })
            })
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                if !enabled {
                    return;
                }
                cx.update_global(|s: &mut DownloadPageState, cx| {
                    on_click(s);
                    s.game_rows_scroll.set_offset(point(px(0.), px(0.)));
                });
            })
    };

    let page_btn = |label: SharedString, active: bool, page: usize| {
        div()
            .id(("download-page-btn", page))
            .min_w(px(32.))
            .h(px(32.))
            .rounded(px(6.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if active {
                colors.accent
            } else {
                Hsla {
                    a: 0.0,
                    ..colors.surface
                }
            })
            .text_color(if active {
                colors.btn_primary_text
            } else {
                colors.text_secondary
            })
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
            )
            .hover(move |s| {
                if active {
                    s
                } else {
                    s.bg(Hsla {
                        a: if colors.bg.l < 0.5 { 0.12 } else { 0.08 },
                        ..colors.text_primary
                    })
                }
            })
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                cx.update_global(|s: &mut DownloadPageState, cx| {
                    s.page_index = page;
                    s.game_rows_scroll.set_offset(point(px(0.), px(0.)));
                });
            })
    };

    let mut pages: Vec<Option<usize>> = Vec::new();
    if total_pages <= 7 {
        for p in 0..total_pages {
            pages.push(Some(p));
        }
    } else {
        let last = total_pages - 1;
        pages.push(Some(0));
        if page_index.saturating_sub(1) > 1 {
            pages.push(None);
        }
        for p in page_index.saturating_sub(1)..=(page_index + 1).min(last) {
            if p != 0 && p != last {
                pages.push(Some(p));
            }
        }
        if page_index + 2 < last {
            pages.push(None);
        }
        pages.push(Some(last));
    }

    let mut page_row = div().flex().items_center().gap(px(8.));
    for p in pages {
        match p {
            Some(p) => {
                page_row = page_row.child(page_btn(
                    SharedString::from((p + 1).to_string()),
                    p == page_index,
                    p,
                ));
            }
            None => {
                page_row = page_row.child(
                    div()
                        .px(px(6.))
                        .text_size(px(12.))
                        .text_color(colors.text_muted)
                        .child("..."),
                );
            }
        }
    }

    let jump = page_jump_input.map(|input| {
        let input_entity = input.clone();
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .child("跳转至"),
            )
            .child(
                Input::new(&input_entity)
                    .w(px(56.))
                    .px(px(4.))
                    .with_size(crate::ui::components::input::InputSize::Small),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_muted)
                    .child("页"),
            )
    });

    div()
        .w_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .flex()
                .justify_start()
                .text_size(px(12.))
                .text_color(colors.text_muted)
                .child({
                    let start = if total == 0 {
                        0
                    } else {
                        page_index * page_size + 1
                    };
                    let end = (start + showing.saturating_sub(1)).min(total);
                    format!("结果: {start}-{end} / {total}")
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(nav_btn(
                    "download-nav-prev",
                    lucide_icons::icon_chevron_left(),
                    prev_enabled,
                    Box::new(|s| s.page_index = s.page_index.saturating_sub(1)),
                ))
                .child(page_row)
                .child(nav_btn(
                    "download-nav-next",
                    lucide_icons::icon_chevron_right(),
                    next_enabled,
                    Box::new(|s| s.page_index = s.page_index.saturating_add(1)),
                )),
        )
        .child(
            div().flex_1().flex().justify_end().child(
                jump.map(IntoElement::into_any_element)
                    .unwrap_or_else(|| div().into_any_element()),
            ),
        )
}

fn render_version_row(
    colors: &ThemeColors,
    version: SharedString,
    channel: &'static str,
    is_gdk: bool,
    action_label: &'static str,
    is_installed: bool,
    disabled: bool,
    package_id: SharedString,
    md5: Option<SharedString>,
    local_path: Option<SharedString>,
    active_task_running: bool,
    active_task: Option<Arc<TaskSnapshot>>,
) -> AnyElement {
    let channel_bg = if channel == "正式" {
        colors.badge_stable_bg
    } else {
        colors.badge_beta_bg
    };
    let channel_fg = if channel == "正式" {
        colors.badge_stable_text
    } else {
        colors.badge_beta_text
    };

    let action_bg = if is_installed {
        colors.accent_hover
    } else {
        colors.accent
    };

    let action_bg = if disabled {
        colors.surface_hover
    } else {
        action_bg
    };

    let file_name = SharedString::from(format!(
        "{}{}",
        version,
        if is_gdk { ".msixvc" } else { ".appx" }
    ));
    let row_element_id = stable_game_row_element_id(&package_id);

    let meta_tag = |label: &'static str, bg: Hsla, fg: Hsla| {
        div()
            .px(px(8.))
            .py(px(2.))
            .rounded(px(4.))
            .bg(bg)
            .text_size(px(12.))
            .text_color(fg)
            .child(label)
    };

    let icon_path = if channel == "预览" {
        "images/minecraft/Preview.png"
    } else {
        "images/minecraft/Release.png"
    };

    let dark_mode = colors.bg.l < 0.5;
    let card_bg = if dark_mode {
        Hsla {
            a: 0.80,
            ..colors.surface
        }
    } else {
        Hsla {
            a: 0.95,
            ..colors.surface
        }
    };

    let card_hover_bg = if dark_mode {
        Hsla {
            a: 0.95,
            ..colors.surface
        }
    } else {
        Hsla {
            a: 1.0,
            ..colors.surface
        }
    };

    let row = div()
        .id(("download-game-row", row_element_id))
        .w_full()
        .h(px(84.))
        .px(px(20.))
        .py(px(12.))
        .rounded(px(8.))
        .flex()
        .items_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(0.))
                .min_w(px(0.))
                .flex_1()
                .child(
                    div().w(px(64.)).flex().items_center().child(
                        img(icon_path)
                            .w(px(42.))
                            .h(px(42.))
                            .rounded(px(10.))
                            .p(px(4.))
                            .bg(Hsla {
                                a: 0.05,
                                ..colors.text_secondary
                            }),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .pr(px(16.))
                        .flex()
                        .flex_col()
                        .justify_center()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(
                                    div()
                                        .text_size(px(18.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text_primary)
                                        .child(version.clone()),
                                )
                                .child(
                                    div()
                                        .px(px(8.))
                                        .py(px(3.))
                                        .rounded(px(6.))
                                        .bg(channel_bg)
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(channel_fg)
                                        .child(channel),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .children(is_gdk.then(|| {
                                    meta_tag(
                                        "GDK",
                                        Hsla {
                                            a: 0.10,
                                            ..colors.accent
                                        },
                                        colors.accent,
                                    )
                                    .into_any_element()
                                }))
                                .child(meta_tag(
                                    "x64",
                                    Hsla {
                                        a: 0.10,
                                        ..colors.text_secondary
                                    },
                                    colors.text_secondary,
                                ))
                                .children((!is_gdk).then(|| {
                                    meta_tag(
                                        "UWP",
                                        Hsla {
                                            a: 0.10,
                                            ..colors.stat_blue_bg
                                        },
                                        colors.stat_blue_text,
                                    )
                                    .into_any_element()
                                })),
                        ),
                ),
        )
        .child({
            let package_id = package_id.clone();
            let file_name = file_name.clone();
            let version = version.clone();
            let md5_string_outer = md5.clone().map(|s| s.to_string());
            let active_task = active_task.clone();
            let active_task_running = active_task_running || active_task.is_some();
            let local_path = local_path.clone();

            let btn_label = active_task
                .as_ref()
                .map(|snapshot| SharedString::from(snapshot.stage.clone()))
                .unwrap_or_else(|| {
                    if active_task_running {
                        SharedString::from("处理中")
                    } else {
                        SharedString::from(action_label)
                    }
                });
            let local_ready = local_path.is_some();

            {
                let btn_fg = if disabled || active_task_running {
                    colors.text_secondary
                } else {
                    colors.btn_primary_text
                };
                let btn_bg = if disabled || active_task_running {
                    Hsla {
                        a: 0.10,
                        ..colors.text_secondary
                    }
                } else {
                    action_bg
                };

                {
                    let is_interactive = !disabled && !active_task_running;
                    div()
                        .id(("download-game-action-btn", row_element_id))
                        .w(px(140.))
                        .h(px(36.))
                        .rounded(px(10.))
                        .bg(btn_bg)
                        .cursor_pointer()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(btn_fg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(8.))
                        .relative()
                        .hover(move |s| if is_interactive { s.opacity(0.85) } else { s })
                        .active(move |s| if is_interactive { s.top(px(1.5)) } else { s })
                        .child(themed_icon(lucide_icons::icon_download(), 16.0, btn_fg))
                        .child(btn_label)
                }
            }
            .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
                if disabled || active_task_running {
                    return;
                }

                let local_path = local_path.clone().or_else(|| {
                    cx.read_global(|s: &DownloadPageState, _cx| {
                        s.local_path_by_package.get(&package_id).cloned()
                    })
                });

                let dialog_kind = if local_path.is_some() {
                    GameDialogKind::LocalActions
                } else {
                    GameDialogKind::ConfirmDownload
                };

                open_game_dialog(
                    window,
                    cx,
                    GameDialogState {
                        kind: dialog_kind,
                        version: version.clone(),
                        package_id: package_id.clone(),
                        version_type: if channel == "正式" { 0 } else { 2 },
                        md5: md5.clone(),
                        is_gdk,
                        file_name: file_name.clone(),
                        local_path: local_path.or_else(|| {
                            if local_ready {
                                Some(local_game_file_path(&file_name))
                            } else {
                                None
                            }
                        }),
                    },
                );
            })
        })
        .when_some(active_task, |this, _snap| this);
    row.into_any_element()
}

fn open_game_dialog(window: &mut Window, cx: &mut App, dialog: GameDialogState) {
    let is_confirm_download = matches!(dialog.kind, GameDialogKind::ConfirmDownload);
    let initial_version = dialog.version.clone();
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        let selected_cdn_base = if dialog.is_gdk {
            reqwest::Url::parse(dialog.package_id.as_ref())
                .ok()
                .and_then(|url| {
                    url.host_str()
                        .map(|host| SharedString::from(format!("{}://{}", url.scheme(), host)))
                })
        } else {
            None
        };
        state.game_dialog = Some(dialog.clone());
        state.game_dialog_input = None;
        state.game_dialog_folder_input = None;
        state.game_dialog_cdn_loading = false;
        state.game_dialog_cdn_error = None;
        state.game_dialog_cdn_results.clear();
        state.game_dialog_selected_cdn_base = selected_cdn_base;
    });
    if is_confirm_download {
        let folder_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder(SharedString::from("版本名称"), window, cx);
            input.set_value(initial_version.clone(), window, cx);
            input
        });
        cx.update_global(|state: &mut DownloadPageState, _cx| {
            if state.game_dialog_folder_input.is_none() {
                state.game_dialog_folder_input = Some(folder_input);
            }
        });
    }
    refresh_game_dialog_cdn(cx);
}

fn local_game_file_path(file_name: &SharedString) -> SharedString {
    let path = file_ops::downloads_dir().join(file_name.as_ref());
    SharedString::from(path.to_string_lossy().to_string())
}

fn close_game_dialog(cx: &mut App) {
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.game_dialog = None;
        state.game_dialog_input = None;
        state.game_dialog_folder_input = None;
        state.game_dialog_cdn_loading = false;
        state.game_dialog_cdn_error = None;
        state.game_dialog_cdn_results.clear();
        state.game_dialog_selected_cdn_base = None;
    });
}

fn reopen_game_dialog_for_redownload(window: &mut Window, cx: &mut App, dialog: GameDialogState) {
    let package_id = dialog.package_id.clone();
    let mut dialog = dialog;
    dialog.kind = GameDialogKind::ConfirmDownload;
    dialog.local_path = None;
    open_game_dialog(window, cx, dialog);
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state
            .force_download_by_package
            .insert(package_id.clone(), true);
    });
}

fn sanitize_game_file_name(raw_name: &str, fallback: &str, is_gdk: bool) -> SharedString {
    let mut value = raw_name
        .trim()
        .replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
    while value.ends_with('.') {
        value.pop();
    }
    if value.is_empty() {
        value = fallback.to_string();
    }

    let extension = if is_gdk { ".msixvc" } else { ".appx" };
    if !value.to_ascii_lowercase().ends_with(extension) {
        value.push_str(extension);
    }

    SharedString::from(value)
}

fn sanitize_install_folder_name(raw_name: &str, fallback_file_name: &str) -> SharedString {
    let mut value = raw_name
        .trim()
        .replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
    while value.ends_with('.') || value.ends_with(' ') {
        value.pop();
    }
    if value.is_empty() {
        let lower = fallback_file_name.to_ascii_lowercase();
        for suffix in [".msixvc", ".appx", ".zip"] {
            if lower.ends_with(suffix) {
                value = fallback_file_name[..fallback_file_name.len() - suffix.len()].to_string();
                break;
            }
        }
        if value.is_empty() {
            value = fallback_file_name.to_string();
        }
    }

    SharedString::from(value)
}

fn apply_cdn_base(original_url: &str, base: &str) -> Result<String, String> {
    let original = reqwest::Url::parse(original_url).map_err(|error| error.to_string())?;
    let mut base_url = reqwest::Url::parse(base).map_err(|error| error.to_string())?;
    base_url.set_path(original.path());
    base_url.set_query(original.query());
    base_url.set_fragment(None);
    Ok(base_url.to_string())
}

pub(super) fn refresh_game_dialog_cdn(cx: &mut App) {
    const GDK_CDN_BASES: [&str; 4] = [
        "http://assets1.xboxlive.cn",
        "http://assets2.xboxlive.cn",
        "http://assets1.xboxlive.com",
        "http://assets2.xboxlive.com",
    ];

    let package_id = cx.read_global(|state: &DownloadPageState, _cx| {
        state.game_dialog.as_ref().and_then(|dialog| {
            (dialog.is_gdk && matches!(dialog.kind, GameDialogKind::ConfirmDownload))
                .then(|| dialog.package_id.clone())
        })
    });
    let Some(package_id) = package_id else {
        return;
    };

    let request_started = cx.update_global(|state: &mut DownloadPageState, _cx| {
        if state.game_dialog.is_none() || state.game_dialog_cdn_loading {
            return false;
        }
        state.game_dialog_cdn_loading = true;
        state.game_dialog_cdn_error = None;
        state.game_dialog_cdn_results.clear();
        true
    });
    if !request_started {
        return;
    }

    cx.spawn(async move |cx| {
        let bases = GDK_CDN_BASES
            .into_iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>();
        let response = cx
            .background_spawn(async move {
                crate::utils::network::probe_gdk_asset_cdns_blocking(package_id.to_string(), bases)
            })
            .await;

        match response {
            Ok(result) => {
                let _ = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    if state.game_dialog.is_none() {
                        return;
                    }
                    state.game_dialog_cdn_loading = false;
                    state.game_dialog_cdn_error = None;
                    state.game_dialog_cdn_results = result
                        .results
                        .into_iter()
                        .map(|item| GameDialogCdnResult {
                            base: SharedString::from(item.base),
                            url: SharedString::from(item.url),
                            latency_ms: item.latency_ms,
                            error: item.error.map(SharedString::from),
                        })
                        .collect();
                    state.game_dialog_selected_cdn_base =
                        result.recommended_base.map(SharedString::from).or_else(|| {
                            state.game_dialog_cdn_results.iter().find_map(|item| {
                                item.latency_ms.is_some().then(|| item.base.clone())
                            })
                        });
                });
            }
            Err(error) => {
                let _ = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    if state.game_dialog.is_none() {
                        return;
                    }
                    state.game_dialog_cdn_loading = false;
                    state.game_dialog_cdn_error = Some(SharedString::from(error));
                });
            }
        }
    })
    .detach();
}

fn channel_label(version_type: i32) -> &'static str {
    match version_type {
        0 => "正式",
        1 => "Beta",
        _ => "预览",
    }
}

fn start_game_operation(
    cx: &mut App,
    dialog: GameDialogState,
    force_download: bool,
    install_folder_override: Option<SharedString>,
    selected_cdn_base: Option<SharedString>,
) {
    let package_id = dialog.package_id.clone();
    let version_label = dialog.version.clone();
    let file_name = sanitize_game_file_name(
        dialog.file_name.as_ref(),
        dialog.version.as_ref(),
        dialog.is_gdk,
    );
    let install_folder = sanitize_install_folder_name(
        install_folder_override
            .as_ref()
            .map(SharedString::as_ref)
            .unwrap_or(""),
        file_name.as_ref(),
    );
    let md5_string = dialog.md5.clone().map(|value| value.to_string());
    let is_gdk = dialog.is_gdk;
    let selected_cdn_base = selected_cdn_base.map(|value| value.to_string());

    close_game_dialog(cx);

    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.operations_by_package.insert(
            package_id.clone(),
            crate::ui::views::download::state::DownloadOperation {
                package_id: package_id.clone(),
                file_name: file_name.clone(),
                download_task_id: None,
                extract_task_id: None,
            },
        );
    });

    info!("game_op: start package_id={package_id} file_name={file_name} is_gdk={is_gdk} force_download={force_download}");
    cx.spawn(async move |cx| {
        let operation_package_id = package_id.clone();
        let operation_file_name = file_name.clone();
        let result = async {
            let local_path = if force_download {
                None
            } else {
                cx.background_spawn({
                    let file_name = file_name.to_string();
                    let md5_string = md5_string.clone();
                    async move {
                        crate::downloads::api::local_download_path(file_name, md5_string)
                            .await
                            .ok()
                            .flatten()
                    }
                })
                .await
            };

            let file_path = if let Some(path) = local_path {
                path
            } else {
                let task_id = if is_gdk {
                    let default_base = reqwest::Url::parse(package_id.as_ref())
                        .ok()
                        .map(|url| {
                            format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default())
                        })
                        .filter(|base| !base.ends_with("://"));
                    let base = selected_cdn_base
                        .clone()
                        .or(default_base)
                        .ok_or_else(|| "missing GDK CDN base for package url".to_string())?;
                    let url = apply_cdn_base(package_id.as_ref(), &base)?;
                    crate::downloads::api::download_resource(
                        url,
                        file_name.to_string(),
                        md5_string.clone(),
                        Some(force_download),
                        None,
                    )
                    .await
                    .map_err(|error| error.to_string())?
                } else {
                    let base = package_id.as_ref();
                    let full_id = if base.contains('_') {
                        base.to_string()
                    } else {
                        format!("{base}_1")
                    };

                    crate::downloads::api::download_appx(
                        full_id,
                        file_name.to_string(),
                        md5_string.clone(),
                        Some(force_download),
                        None,
                    )
                    .await
                    .map_err(|error| error.to_string())?
                };

                if let Err(err) = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    if let Some(operation) = state.operations_by_package.get_mut(&package_id) {
                        operation.download_task_id = Some(SharedString::from(task_id.clone()));
                    }
                }) {
                    warn!("update_global failed: {err:?}");
                }
                let _ = crate::tasks::task_manager::set_task_labels(
                    &task_id,
                    file_name.as_ref(),
                    Some(version_label.to_string()),
                );

                info!("game_op: waiting for download task_id={task_id}");
                let snapshot = wait_task_finished(&task_id).await?;
                info!("game_op: download completed task_id={task_id} status={}", snapshot.status);
                if snapshot.status.as_ref() != "completed" {
                    return Err(format!(
                        "download {} ({})",
                        snapshot.status,
                        snapshot.message.clone().unwrap_or_default()
                    ));
                }

                snapshot
                    .message
                    .clone()
                    .map(|message| message.to_string())
                    .ok_or_else(|| "download completed but no path returned".to_string())?
            };

            info!("game_op: starting extract file_path={file_path} install_folder={install_folder}");
            let extract_task_id = if is_gdk {
                crate::core::minecraft::gdk::unpack::start_unpack_gdk_task(
                    file_path.clone(),
                    install_folder.as_ref(),
                )?
            } else {
                crate::archive::api::extract_zip_appx(
                    format!("{}.appx", install_folder),
                    file_path.clone(),
                    true,
                    true,
                )
                .await?
            };

            if let Err(err) = cx.update_global(|state: &mut DownloadPageState, _cx| {
                if let Some(operation) = state.operations_by_package.get_mut(&package_id) {
                    operation.extract_task_id = Some(SharedString::from(extract_task_id.clone()));
                }
            }) {
                warn!("update_global failed: {err:?}");
            }

            info!("game_op: waiting for extract task_id={extract_task_id}");
            let extract_snapshot = wait_task_finished(&extract_task_id).await?;
            info!("game_op: extract completed task_id={extract_task_id} status={}", extract_snapshot.status);
            if extract_snapshot.status.as_ref() != "completed" {
                return Err(format!(
                    "extract {} ({})",
                    extract_snapshot.status,
                    extract_snapshot.message.clone().unwrap_or_default()
                ));
            }

            if let Err(err) = cx.update_global(|state: &mut DownloadPageState, _cx| {
                state.operations_by_package.remove(&package_id);
            }) {
                warn!("update_global failed: {err:?}");
            }

            let local_after = cx
                .background_spawn({
                    let file_name = file_name.to_string();
                    let md5_string = md5_string.clone();
                    async move {
                        crate::downloads::api::local_download_path(file_name, md5_string)
                            .await
                            .ok()
                            .flatten()
                    }
                })
                .await;

            if let Err(err) =
                cx.update_global(|state: &mut DownloadPageState, _cx| match local_after {
                    Some(path) => {
                        state
                            .local_path_by_package
                            .insert(package_id.clone(), SharedString::from(path));
                        state.local_files.insert(file_name.clone());
                    }
                    None => {
                        state.local_path_by_package.remove(&package_id);
                        state.local_files.remove(&file_name);
                    }
                })
            {
                warn!("update_global failed: {err:?}");
            }

            info!("game_op: triggering version refresh after successful download+extract");
            cx.update(|cx| {
                crate::ui::hooks::use_local_versions::ensure_local_versions_loaded(true, cx);
            })
            .map_err(|error| format!("请求刷新本地游戏版本失败: {error}"))?;

            info!("game_op: download+extract complete");
            Ok::<(), String>(())
        }
        .await;

        if let Err(error) = result {
            warn!("game_op: operation failed error={error}");
            let message = SharedString::from(format!(
                "{} 操作失败: {}",
                operation_file_name.as_ref(),
                error
            ));
            if let Err(err) = cx.update(|cx| {
                cx.update_global(|state: &mut DownloadPageState, cx| {
                    state.operations_by_package.remove(&operation_package_id);
                    toast::error(cx, message);
                });
            }) {
                warn!("failed to clear game operation after error: {err:?}");
            }

            let _ = cx.update(|cx| {
                crate::ui::hooks::use_local_versions::ensure_local_versions_loaded(true, cx);
            });
        }

        Ok::<(), anyhow::Error>(())
    })
    .detach_and_log_err(cx);
}

fn delete_game_local_file(cx: &mut App, dialog: GameDialogState) {
    close_game_dialog(cx);

    let package_id = dialog.package_id.clone();
    let file_name = dialog.file_name.clone();
    let md5_string = dialog.md5.clone().map(|value| value.to_string());

    cx.spawn(async move |cx| {
        crate::downloads::api::delete_local_download(file_name.to_string()).await?;

        let local_after =
            crate::downloads::api::local_download_path(file_name.to_string(), md5_string)
                .await
                .ok()
                .flatten();

        if let Err(err) = cx.update_global(|state: &mut DownloadPageState, _cx| match local_after {
            Some(path) => {
                state
                    .local_path_by_package
                    .insert(package_id.clone(), SharedString::from(path));
                state.local_files.insert(file_name.clone());
            }
            None => {
                state.local_path_by_package.remove(&package_id);
                state.local_files.remove(&file_name);
            }
        }) {
            warn!("update_global failed: {err:?}");
        }

        Ok::<(), String>(())
    })
    .detach();
}

fn render_local_action_card<F>(
    colors: &ThemeColors,
    icon: &'static str,
    accent: Hsla,
    title: &'static str,
    subtitle: &'static str,
    primary: bool,
    handler: F,
) -> Div
where
    F: Fn(&mut Window, &mut App) + 'static,
{
    div()
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: if primary { 0.18 } else { 0.16 },
            ..colors.border
        })
        .bg(if primary {
            Hsla {
                a: 0.10,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.92,
                ..colors.settings_field_bg
            }
        })
        .p(px(11.))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .hover(|style| {
            style
                .bg(if primary {
                    Hsla {
                        a: 0.14,
                        ..colors.accent
                    }
                } else {
                    Hsla {
                        a: 1.0,
                        ..colors.surface
                    }
                })
                .border_color(Hsla {
                    a: 0.24,
                    ..colors.border
                })
        })
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            handler(window, cx);
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(36.))
                        .h(px(36.))
                        .rounded(px(10.))
                        .bg(Hsla { a: 0.14, ..accent })
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .child(themed_icon(icon, 15.0, accent)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .min_w(px(0.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .line_height(relative(1.35))
                                .text_color(Hsla {
                                    a: 0.94,
                                    ..colors.text_secondary
                                })
                                .whitespace_normal()
                                .child(subtitle),
                        ),
                ),
        )
        .child(themed_icon(
            lucide_icons::icon_chevron_right(),
            14.0,
            colors.text_secondary,
        ))
}

fn render_local_actions_dialog(colors: &ThemeColors, dialog: GameDialogState) -> Div {
    let local_path = dialog
        .local_path
        .clone()
        .unwrap_or_else(|| local_game_file_path(&dialog.file_name));
    let local_file_name = std::path::Path::new(local_path.as_ref())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(dialog.file_name.as_ref())
        .to_string();
    let channel = channel_label(dialog.version_type);
    let platform_label = if dialog.is_gdk { "GDK" } else { "UWP" };
    let local_path_for_open = local_path.clone();
    let header = div()
        .relative()
        .px(px(24.))
        .pt(px(18.))
        .pb(px(14.))
        .flex()
        .items_start()
        .justify_between()
        .gap(px(14.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child("本地安装选项"),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.))
                        .line_height(relative(1.35))
                        .text_color(colors.text_secondary)
                        .whitespace_normal()
                        .child("检测到本地已下载的安装包。你可以直接本地安装，或重新下载，也可以删除本地包释放空间。"),
                ),
        )
        .child(
            div()
                .flex_none()
                .w(px(32.))
                .h(px(32.))
                .rounded(px(10.))
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.text_secondary)
                .child(themed_icon(
                    lucide_icons::icon_x(),
                    16.0,
                    colors.text_secondary,
                ))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    close_game_dialog(cx);
                }),
        )
        .child(div().absolute().left(px(0.)).right(px(0.)).bottom(px(0.)).h(px(1.)).bg(Hsla {
            a: 0.14,
            ..colors.border
        }));

    let package_card = div()
        .rounded(px(10.))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.96,
            ..colors.settings_card_bg
        })
        .p(px(12.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(36.))
                        .h(px(36.))
                        .rounded(px(10.))
                        .bg(Hsla {
                            a: 0.12,
                            ..colors.accent
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(themed_icon(
                            lucide_icons::icon_package(),
                            14.0,
                            colors.text_secondary,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .min_w(px(0.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(dialog.version.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.text_secondary)
                                .child(local_file_name),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .child(
                    div()
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.18,
                            ..colors.badge_stable_bg
                        })
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.badge_stable_text)
                        .child(channel),
                )
                .child(
                    div()
                        .px(px(7.))
                        .py(px(2.))
                        .rounded(px(999.))
                        .bg(Hsla {
                            a: 0.16,
                            ..colors.accent
                        })
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.accent)
                        .child(platform_label),
                )
                .child(
                    div()
                        .w(px(30.))
                        .h(px(30.))
                        .rounded(px(9.))
                        .border_1()
                        .border_color(Hsla {
                            a: 0.16,
                            ..colors.border
                        })
                        .bg(Hsla {
                            a: 0.92,
                            ..colors.settings_field_bg
                        })
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(colors.accent)
                        .child(themed_icon(
                            lucide_icons::icon_folder_open(),
                            14.0,
                            colors.accent,
                        ))
                        .hover(|style| {
                            style.bg(Hsla {
                                a: 1.0,
                                ..colors.surface
                            })
                        })
                        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                            let path = local_path_for_open.clone();
                            cx.spawn(async move |_cx| {
                                let open_target = std::path::Path::new(path.as_ref())
                                    .parent()
                                    .map(|value| value.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.to_string());
                                let _ = crate::utils::open_path::open_path(open_target).await;
                                Ok::<(), anyhow::Error>(())
                            })
                            .detach();
                        }),
                ),
        );

    let action_list = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(render_local_action_card(
            colors,
            lucide_icons::icon_download(),
            colors.accent,
            "本地安装",
            "继续进入安装确认并设置版本名称",
            true,
            {
                let dialog = dialog.clone();
                move |window, cx| {
                    open_game_dialog(
                        window,
                        cx,
                        GameDialogState {
                            kind: GameDialogKind::ConfirmDownload,
                            ..dialog.clone()
                        },
                    );
                }
            },
        ))
        .child(render_local_action_card(
            colors,
            lucide_icons::icon_rotate_cw(),
            colors.text_secondary,
            "重新下载",
            "忽略本地包并重新下载最新安装包",
            false,
            {
                let dialog = dialog.clone();
                move |window, cx| {
                    reopen_game_dialog_for_redownload(window, cx, dialog.clone());
                }
            },
        ))
        .child(render_local_action_card(
            colors,
            lucide_icons::icon_trash_2(),
            colors.danger,
            "删除本地安装包",
            "从下载目录删除该本地安装包",
            false,
            {
                let dialog = dialog.clone();
                move |window, cx| {
                    open_game_dialog(
                        window,
                        cx,
                        GameDialogState {
                            kind: GameDialogKind::ConfirmDelete,
                            ..dialog.clone()
                        },
                    )
                }
            },
        ));

    div()
        .w(px(560.))
        .max_w(px(560.))
        .max_h(px(880.))
        .rounded(px(12.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(colors.settings_panel_bg)
        .overflow_hidden()
        .flex()
        .flex_col()
        .relative()
        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
        .child(header)
        .child(
            div()
                .flex_none()
                .px(px(20.))
                .pb(px(34.))
                .pt(px(10.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(package_card)
                .child(action_list),
        )
}

pub(super) fn render_game_dialog(
    colors: &ThemeColors,
    dialog: GameDialogState,
    folder_name_input: Option<&Entity<InputState>>,
    cdn_loading: bool,
    cdn_error: Option<SharedString>,
    cdn_results: Vec<GameDialogCdnResult>,
    selected_cdn_base: Option<SharedString>,
) -> Div {
    if matches!(dialog.kind, GameDialogKind::LocalActions) {
        return render_local_actions_dialog(colors, dialog);
    }

    let is_local_install_confirm =
        matches!(dialog.kind, GameDialogKind::ConfirmDownload) && dialog.local_path.is_some();
    let title = match dialog.kind {
        GameDialogKind::ConfirmDownload if is_local_install_confirm => "本地安装确认",
        GameDialogKind::ConfirmDownload => "确认",
        GameDialogKind::ConfirmDelete => "删除本地包",
        GameDialogKind::LocalActions => unreachable!("local actions dialog is rendered separately"),
    };
    let subtitle = match dialog.kind {
        GameDialogKind::ConfirmDownload if is_local_install_confirm => "请确认版本名称",
        GameDialogKind::ConfirmDownload => "版本名称",
        GameDialogKind::ConfirmDelete => "删除本地包",
        GameDialogKind::LocalActions => unreachable!("local actions dialog is rendered separately"),
    };

    let folder_name_editor = matches!(dialog.kind, GameDialogKind::ConfirmDownload)
        .then(|| {
            folder_name_input.map(|input| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .child("版本名称"),
                    )
                    .child(
                        Input::new(input)
                            .w_full()
                            .h(px(40.))
                            .px(px(12.))
                            .rounded(px(10.)),
                    )
            })
        })
        .flatten();

    let delete_notice = matches!(dialog.kind, GameDialogKind::ConfirmDelete).then(|| {
        div()
            .rounded(px(10.))
            .border_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.danger
            })
            .bg(Hsla {
                a: 0.08,
                ..colors.danger
            })
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child("将从下载目录删除这个本地安装包。"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_secondary)
                    .child("这不会删除已安装的游戏目录，只会移除当前下载好的本地包。"),
            )
            .into_any_element()
    });

    let cdn_panel = (dialog.is_gdk
        && matches!(dialog.kind, GameDialogKind::ConfirmDownload)
        && dialog.local_path.is_none())
    .then(|| {
        const GDK_CDN_BASES: [&str; 4] = [
            "http://assets1.xboxlive.cn",
            "http://assets2.xboxlive.cn",
            "http://assets1.xboxlive.com",
            "http://assets2.xboxlive.com",
        ];

        let cdn_entries = if cdn_results.is_empty() {
            GDK_CDN_BASES
                .into_iter()
                .map(|base| GameDialogCdnResult {
                    base: SharedString::from(base),
                    url: SharedString::from(""),
                    latency_ms: None,
                    error: None,
                })
                .collect::<Vec<_>>()
        } else {
            cdn_results
        };
        let mut cdn_list = div().flex().flex_col().gap(px(10.));
        for pair in cdn_entries.chunks(2) {
            let mut row = div().flex().gap(px(10.)).w_full();
            for result in pair {
                let is_selected = selected_cdn_base.as_ref() == Some(&result.base);
                let badge_text =
                    if cdn_loading && result.latency_ms.is_none() && result.error.is_none() {
                        "测试中".to_string()
                    } else if let Some(latency_ms) = result.latency_ms {
                        format!("{latency_ms} ms")
                    } else {
                        "失败".to_string()
                    };
                let badge_color = if let Some(latency_ms) = result.latency_ms {
                    if latency_ms <= 80 {
                        colors.stat_green_text
                    } else if latency_ms <= 180 {
                        colors.accent
                    } else if latency_ms <= 500 {
                        colors.danger
                    } else {
                        Hsla {
                            a: 1.0,
                            ..colors.text_secondary
                        }
                    }
                } else {
                    colors.danger
                };

                row = row.child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(if is_selected {
                            Hsla {
                                a: 0.45,
                                ..colors.accent
                            }
                        } else {
                            Hsla {
                                a: 0.12,
                                ..colors.border
                            }
                        })
                        .bg(if is_selected {
                            Hsla {
                                a: 0.10,
                                ..colors.accent
                            }
                        } else {
                            Hsla {
                                a: 0.25,
                                ..colors.surface_hover
                            }
                        })
                        .p(px(12.))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, {
                            let base = result.base.clone();
                            move |_ev, _window, cx| {
                                cx.update_global(|state: &mut DownloadPageState, _cx| {
                                    state.game_dialog_selected_cdn_base = Some(base.clone());
                                });
                            }
                        })
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .justify_between()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.))
                                        .child(
                                            div()
                                                .text_size(px(13.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(colors.text_primary)
                                                .child(result.base.clone()),
                                        )
                                        .children(result.error.clone().map(|error| {
                                            div()
                                                .text_size(px(12.))
                                                .text_color(colors.text_secondary)
                                                .child(error)
                                                .into_any_element()
                                        })),
                                )
                                .child(
                                    div()
                                        .px(px(8.))
                                        .py(px(4.))
                                        .rounded(px(999.))
                                        .bg(Hsla {
                                            a: 0.12,
                                            ..badge_color
                                        })
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(badge_color)
                                        .child(badge_text),
                                ),
                        ),
                );
            }
            if pair.len() == 1 {
                row = row.child(div().flex_1());
            }
            cdn_list = cdn_list.child(row);
        }

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .child("GDK CDN 节点"),
                    )
                    .child(render_dialog_button(
                        colors,
                        "重新测试",
                        false,
                        |_window, cx| refresh_game_dialog_cdn(cx),
                    )),
            )
            .child(cdn_list)
            .children(cdn_error.map(|error| {
                div()
                    .text_size(px(12.))
                    .text_color(colors.danger)
                    .child(error)
                    .into_any_element()
            }))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_secondary)
                    .child("会优先选择延迟最低的可用 CDN，也可以手动改选。"),
            )
    });

    let body = div()
        .w(px(if dialog.is_gdk { 640. } else { 560. }))
        .max_w(px(640.))
        .max_h(px(760.))
        .rounded(px(12.))
        .border_1()
        .border_color(Hsla {
            a: 0.18,
            ..colors.border
        })
        .bg(colors.surface)
        .overflow_hidden()
        .flex()
        .flex_col()
        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
        .child(
            div()
                .px(px(22.))
                .pt(px(22.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .w(px(44.))
                                .h(px(44.))
                                .rounded(px(999.))
                                .bg(Hsla {
                                    a: 0.12,
                                    ..colors.accent
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(themed_icon(
                                    lucide_icons::icon_download(),
                                    18.0,
                                    colors.accent,
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_size(px(20.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text_primary)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(colors.text_secondary)
                                        .child(subtitle),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(5.))
                                .rounded(px(999.))
                                .border_1()
                                .border_color(Hsla {
                                    a: 0.10,
                                    ..colors.border
                                })
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(dialog.version.clone()),
                        )
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(5.))
                                .rounded(px(999.))
                                .border_1()
                                .border_color(Hsla {
                                    a: 0.10,
                                    ..colors.border
                                })
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(channel_label(dialog.version_type)),
                        )
                        .child(
                            div()
                                .px(px(10.))
                                .py(px(5.))
                                .rounded(px(999.))
                                .bg(Hsla {
                                    a: 0.10,
                                    ..colors.stat_green_text
                                })
                                .border_1()
                                .border_color(Hsla {
                                    a: 0.20,
                                    ..colors.stat_green_text
                                })
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.stat_green_text)
                                .child(if dialog.is_gdk { "GDK" } else { "UWP" }),
                        ),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .scrollbar_width(px(6.))
                .px(px(22.))
                .rounded(px(16.))
                .border_1()
                .border_color(Hsla {
                    a: 0.12,
                    ..colors.border
                })
                .bg(Hsla {
                    a: 0.35,
                    ..colors.surface_hover
                })
                .p(px(16.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .children(folder_name_editor.map(IntoElement::into_any_element))
                .children(delete_notice)
                .children(cdn_panel.map(IntoElement::into_any_element))
                .children(dialog.local_path.clone().map(|path| {
                    div()
                        .text_size(px(13.))
                        .text_color(colors.text_secondary)
                        .child(format!("本地文件: {path}"))
                        .into_any_element()
                })),
        );

    let footer = match dialog.kind {
        GameDialogKind::ConfirmDownload => div()
            .flex()
            .justify_end()
            .gap(px(10.))
            .child(render_dialog_button(
                colors,
                "取消",
                false,
                |_window, cx| close_game_dialog(cx),
            ))
            .child(render_dialog_button(
                colors,
                if is_local_install_confirm {
                    "开始安装"
                } else {
                    "开始下载"
                },
                true,
                {
                    let dialog = dialog.clone();
                    let folder_name_input = folder_name_input.cloned();
                    move |_window, cx| {
                        let folder_name_override = folder_name_input
                            .as_ref()
                            .map(|input| input.read(cx).value());
                        let selected_cdn_base = cx.read_global(|state: &DownloadPageState, _cx| {
                            state.game_dialog_selected_cdn_base.clone()
                        });
                        let force_download =
                            cx.update_global(|state: &mut DownloadPageState, _cx| {
                                state
                                    .force_download_by_package
                                    .remove(&dialog.package_id)
                                    .unwrap_or(false)
                            });
                        start_game_operation(
                            cx,
                            dialog.clone(),
                            force_download,
                            folder_name_override,
                            selected_cdn_base,
                        )
                    }
                },
            )),
        GameDialogKind::ConfirmDelete => div()
            .flex()
            .justify_end()
            .gap(px(10.))
            .child(render_dialog_button(colors, "取消", false, {
                let dialog = dialog.clone();
                move |window, cx| {
                    open_game_dialog(
                        window,
                        cx,
                        GameDialogState {
                            kind: GameDialogKind::LocalActions,
                            ..dialog.clone()
                        },
                    )
                }
            }))
            .child(render_dialog_button(colors, "确认删除", true, {
                let dialog = dialog.clone();
                move |_window, cx| delete_game_local_file(cx, dialog.clone())
            })),
        GameDialogKind::LocalActions => unreachable!("local actions dialog is rendered separately"),
    };

    body.child(div().px(px(22.)).pb(px(22.)).pt(px(16.)).child(footer))
}

fn render_dialog_button<F>(
    colors: &ThemeColors,
    label: &'static str,
    primary: bool,
    handler: F,
) -> Div
where
    F: Fn(&mut Window, &mut App) + 'static,
{
    let bg = if primary {
        colors.accent
    } else {
        Hsla {
            a: 0.06,
            ..colors.text_secondary
        }
    };
    let fg = if primary {
        colors.btn_primary_text
    } else {
        colors.text_secondary
    };

    div()
        .h(px(38.))
        .px(px(16.))
        .rounded(px(10.))
        .bg(bg)
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .child(label)
        .hover(|style| style.opacity(0.92))
        .on_mouse_down(MouseButton::Left, move |_ev, window, cx| {
            handler(window, cx);
        })
}
