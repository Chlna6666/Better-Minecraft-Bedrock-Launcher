use crate::tasks::task_manager::{self, TaskSnapshot};
use crate::ui::animation::repeating_linear_motion;
use crate::ui::components::adaptive::{
    AdaptiveModalSpec, AdaptiveSizeClass, WindowMetrics, adaptive_modal_size,
};
use crate::ui::components::markdown_renderer::{
    MarkdownDocument, MarkdownItem, render_markdown_item,
};
use crate::ui::components::modal;
use crate::ui::state::i18n::I18n;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::utils::format_bytes::{format_bytes, format_bytes_per_sec};
use crate::utils::updater::ReleaseSummary;
use gpui::list;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui::{AnimationExt, StatefulInteractiveElement as _};
use std::sync::Arc;
use std::time::{Duration, Instant};

const UPDATE_MODAL_MARGIN_PX: f32 = 40.0;
const UPDATE_MODAL_MIN_WIDTH_PX: f32 = 340.0;
const UPDATE_MODAL_MAX_WIDTH_PX: f32 = 520.0;
const UPDATE_MODAL_MIN_HEIGHT_PX: f32 = 300.0;
const UPDATE_INFO_MAX_HEIGHT_PX: f32 = 560.0;
const UPDATE_DOWNLOAD_MAX_HEIGHT_PX: f32 = 240.0;
const UPDATE_DOWNLOAD_ERROR_MAX_HEIGHT_PX: f32 = 280.0;
const UPDATE_INFO_FIXED_CHROME_PX: f32 = 180.0;
const UPDATE_CHANGELOG_MIN_SCROLL_PX: f32 = 120.0;
const UPDATE_CHANGELOG_MAX_SCROLL_PX: f32 = 340.0;

pub struct UpdateMarkdownView {
    release_tag: String,
    document: Arc<MarkdownDocument>,
    items: Arc<Vec<MarkdownItem>>,
    colors: ThemeColors,
    dark: bool,
    active: bool,
    list_state: ListState,
}

impl UpdateMarkdownView {
    pub fn new(
        release_tag: String,
        document: Arc<MarkdownDocument>,
        colors: ThemeColors,
        dark: bool,
    ) -> Self {
        let items = Arc::new(document.linearize());
        let list_state = ListState::new(items.len(), ListAlignment::Top, px(120.));
        Self {
            release_tag,
            document,
            items,
            colors,
            dark,
            active: true,
            list_state,
        }
    }

    pub fn matches(
        &self,
        release_tag: &str,
        document: &Arc<MarkdownDocument>,
        colors: &ThemeColors,
        dark: bool,
    ) -> bool {
        self.release_tag == release_tag
            && Arc::ptr_eq(&self.document, document)
            && self.dark == dark
            && self.colors == *colors
    }

    pub fn update(
        &mut self,
        release_tag: String,
        document: Arc<MarkdownDocument>,
        colors: ThemeColors,
        dark: bool,
    ) {
        let changed = self.release_tag != release_tag || !Arc::ptr_eq(&self.document, &document);
        self.release_tag = release_tag;
        self.document = document;
        self.colors = colors;
        self.dark = dark;
        self.active = true;
        if changed {
            let items = Arc::new(self.document.linearize());
            self.list_state.reset(items.len());
            self.items = items;
        }
    }

    pub fn set_active(&mut self, active: bool) -> bool {
        if self.active == active {
            return false;
        }

        self.active = active;
        true
    }
}

impl Render for UpdateMarkdownView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if self.items.is_empty() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(self.colors.text_muted)
                .text_size(px(13.))
                .child(t!("UpdateModal.empty_log"))
                .into_any_element();
        }

        let items = self.items.clone();
        let colors = self.colors;
        let is_dark = self.dark;

        list(self.list_state.clone(), move |index, _window, _cx| {
            if let Some(item) = items.get(index) {
                render_markdown_item(item, &colors, is_dark)
            } else {
                div().into_any_element()
            }
        })
        .size_full()
        .into_any_element()
    }
}

fn render_markdown_view(view: Entity<UpdateMarkdownView>) -> AnyElement {
    view.into_any_element()
}

fn format_date(iso: Option<String>, i18n: &I18n) -> SharedString {
    let Some(s) = iso else {
        return t!("UpdateModal.date.unknown");
    };
    let d = if s.len() >= 10 { &s[0..10] } else { s.as_str() };
    let mut it = d.split('-');
    let y = it.next().unwrap_or(d);
    let m = it.next().unwrap_or("");
    let dd = it.next().unwrap_or("");
    if !m.is_empty() && !dd.is_empty() {
        let month = m.trim_start_matches('0');
        let day = dd.trim_start_matches('0');
        t!("UpdateModal.date.full", month = y, day = month, year = day)
    } else {
        SharedString::from(d.to_string())
    }
}

fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla {
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

pub fn render_update_modal(
    release: ReleaseSummary,
    markdown_view: Option<Entity<UpdateMarkdownView>>,
    _changelog_scroll_handle: ScrollHandle,
    window_width: Pixels,
    window_height: Pixels,
    modal_visible: bool,
    downloading: bool,
    task_id: Option<String>,
    snapshot: Option<Arc<TaskSnapshot>>,
    download_error: Option<String>,
    theme_factor: f32,
    modal_factor: f32,
    accent_override: Option<Hsla>,
    i18n: &I18n,
) -> impl IntoElement + use<> {
    let tag = release.tag.clone();
    let title = release
        .name
        .clone()
        .unwrap_or_else(|| t!("UpdateModal.default_title", tag = &tag).to_string());
    let date = format_date(release.published_at.clone(), i18n);
    let size = release
        .asset_size
        .map(format_bytes)
        .unwrap_or_else(|| t!("UpdateModal.size.unknown").to_string());
    let colors = lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme_factor,
        accent_override,
    );
    let panel_edge_color = if theme_factor > 0.5 {
        Hsla {
            h: colors.bg.h,
            s: colors.bg.s,
            l: (colors.bg.l + 0.08).min(1.0),
            a: 0.75,
        }
    } else {
        colors.border
    };

    let (channel_label, channel_bg, channel_fg) = if release.prerelease {
        (
            t!("common.preview"),
            colors.badge_beta_bg,
            colors.badge_beta_text,
        )
    } else {
        (
            t!("common.release"),
            colors.badge_stable_bg,
            colors.badge_stable_text,
        )
    };

    let window_metrics = WindowMetrics::new(window_width, window_height);
    let max_height = if downloading {
        if download_error.is_some() {
            UPDATE_DOWNLOAD_ERROR_MAX_HEIGHT_PX
        } else {
            UPDATE_DOWNLOAD_MAX_HEIGHT_PX
        }
    } else {
        UPDATE_INFO_MAX_HEIGHT_PX
    };
    let modal_size = adaptive_modal_size(
        window_metrics,
        AdaptiveModalSpec {
            min_width: UPDATE_MODAL_MIN_WIDTH_PX,
            max_width: UPDATE_MODAL_MAX_WIDTH_PX,
            min_height: UPDATE_MODAL_MIN_HEIGHT_PX,
            max_height,
            margin: UPDATE_MODAL_MARGIN_PX,
        },
    );
    let card_w = modal_size.width;
    let available_card_h = modal_size.height;
    let changelog_scroll_height = if downloading {
        px(0.0)
    } else {
        let raw_height = ((available_card_h / px(1.0)) - UPDATE_INFO_FIXED_CHROME_PX).clamp(
            UPDATE_CHANGELOG_MIN_SCROLL_PX,
            UPDATE_CHANGELOG_MAX_SCROLL_PX,
        );
        px((raw_height / 24.0).floor() * 24.0)
    };
    let card_h = if downloading {
        let target_h = if download_error.is_some() {
            UPDATE_DOWNLOAD_ERROR_MAX_HEIGHT_PX
        } else {
            UPDATE_DOWNLOAD_MAX_HEIGHT_PX
        };
        px(target_h.min(available_card_h / px(1.0)))
    } else {
        px(
            (UPDATE_INFO_FIXED_CHROME_PX + changelog_scroll_height / px(1.0))
                .min(available_card_h / px(1.0))
                .max(UPDATE_MODAL_MIN_HEIGHT_PX.min(available_card_h / px(1.0))),
        )
    };
    let motion_k = modal_factor.clamp(0.0, 1.06);
    let k = motion_k.min(1.0);
    let k = if k > 0.996 { 1.0 } else { k };
    let smooth_k = (k * k * (3.0 - 2.0 * k)).clamp(0.0, 1.0);
    let card_offset_y = if modal_visible {
        px((1.0 - smooth_k) * 14.0 - (motion_k - 1.0).max(0.0) * 6.0)
    } else {
        px((1.0 - smooth_k) * 10.0)
    };
    let overlay_bg = hsla(0., 0., 0.08, 0.32);
    let asset_url = release.asset_url.clone();
    let asset_name = release.asset_name.clone();
    let card_bg = colors.bg;

    // ========== Header 区域 ==========
    let header = div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .px(px(7.))
                                .py(px(2.))
                                .rounded(px(crate::ui::theme::tokens::radius::FULL))
                                .bg(channel_bg)
                                .text_size(px(10.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(channel_fg)
                                .child(channel_label),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_muted)
                        .child(format!("{} · {}", date, size)),
                ),
        )
        .child(
            // 关闭按钮
            div()
                .w(px(28.))
                .h(px(28.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.text_muted)
                .when(downloading, |this| {
                    this.opacity(0.35).cursor(CursorStyle::OperationNotAllowed)
                })
                .when(!downloading, |this| {
                    this.cursor_pointer()
                        .hover(|s| s.bg(colors.surface).text_color(colors.text_primary))
                        .on_mouse_down(MouseButton::Left, |_, _window, cx| {
                            let now = Instant::now();
                            cx.update_global(|u: &mut UpdateState, cx| {
                                u.set_show_modal(false, now);
                            });
                        })
                })
                .child(
                    svg()
                        .path(lucide_gpui::icon!(x))
                        .w(px(15.))
                        .h(px(15.))
                        .text_color(colors.text_muted),
                ),
        );

    // ========== 下载状态 UI ==========
    let downloading_body = {
        let snap = snapshot.clone();
        let percent = snap
            .as_ref()
            .and_then(|s| s.percent)
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        let pct_label = format!("{:.0}%", percent);
        let is_extracting = snap
            .as_ref()
            .is_some_and(|snapshot| snapshot.stage.as_ref() == "extracting");
        let eta = snap
            .as_ref()
            .map(|s| s.eta.as_ref())
            .filter(|eta| !eta.eq_ignore_ascii_case("unknown"))
            .map(str::to_string)
            .unwrap_or_else(|| "--:--".to_string());
        let speed = snap.as_ref().map(|s| s.speed_bytes_per_sec).unwrap_or(0.0);
        let done = snap.as_ref().map(|s| s.done).unwrap_or(0);
        let snapshot_total = snap.as_ref().and_then(|s| s.total).unwrap_or(0);
        let display_total = if snapshot_total == 0 {
            release.asset_size.unwrap_or(0)
        } else {
            snapshot_total
        };
        let total_label = if display_total == 0 {
            t!("UpdateModal.no_file").to_string()
        } else {
            format_bytes(display_total)
        };
        let is_indeterminate = snapshot_total == 0;
        let progress_ratio = ((percent as f32) / 100.0).clamp(0.0, 1.0);
        let progress_width = if progress_ratio > 0.0 {
            progress_ratio.max(0.02)
        } else {
            0.0
        };
        let (stage_title, stage_detail) = if is_extracting {
            (
                t!("UpdateModal.summary.extracting_title"),
                t!("UpdateModal.summary.extracting_detail", tag = &tag),
            )
        } else if is_indeterminate {
            (
                t!("UpdateModal.summary.downloading_title"),
                t!("UpdateModal.progress.connecting_source"),
            )
        } else {
            (
                t!("UpdateModal.summary.downloading_title"),
                t!("UpdateModal.progress.install_after_download"),
            )
        };

        let cancel_btn = div()
            .id("update-cancel")
            .px(px(16.))
            .py(px(7.))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .cursor_pointer()
            .text_size(px(13.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(colors.text_secondary)
            .bg(colors.surface)
            .border_1()
            .border_color(panel_edge_color)
            .hover(|this| {
                this.bg(with_alpha(colors.danger, 0.10))
                    .text_color(colors.danger)
                    .border_color(with_alpha(colors.danger, 0.25))
            })
            .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE))
            .on_mouse_down(MouseButton::Left, {
                let id = task_id.clone();
                move |_, _window, cx| {
                    if let Some(id) = id.clone() {
                        if let Err(error) = crate::tasks::runtime::spawn_io(async move {
                            task_manager::cancel_task(&id);
                        }) {
                            tracing::error!(%error, "failed to schedule update cancellation");
                        }
                    }
                    let now = Instant::now();
                    cx.update_global(|u: &mut UpdateState, cx| {
                        u.cancel_download();
                        u.set_show_modal(false, now);
                    });
                }
            })
            .child(t!("UpdateModal.cancel_download"));

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(14.))
            .pt(px(4.))
            .child(
                // Phase row: icon + title + percent
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .size(px(28.))
                                    .rounded_full()
                                    .bg(with_alpha(colors.accent, 0.12))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        svg()
                                            .path(if is_extracting {
                                                lucide_gpui::icon!(package)
                                            } else {
                                                lucide_gpui::icon!(download)
                                            })
                                            .size(px(14.))
                                            .text_color(colors.accent),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.text_primary)
                                            .child(stage_title),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.5))
                                            .text_color(colors.text_muted)
                                            .child(stage_detail),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.accent)
                            .child(pct_label),
                    ),
            )
            .child(
                // Progress Bar
                div()
                    .h(px(6.))
                    .w_full()
                    .rounded_full()
                    .bg(with_alpha(colors.accent, 0.12))
                    .relative()
                    .overflow_hidden()
                    .child(if is_indeterminate {
                        div()
                            .absolute()
                            .top(px(0.))
                            .bottom(px(0.))
                            .w(relative(0.36))
                            .rounded_full()
                            .bg(colors.accent)
                            .with_animation(
                                "update-download-indeterminate",
                                repeating_linear_motion(Duration::from_millis(1200)),
                                |this, t| this.left(relative(-0.36 + t * 1.42)),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .relative()
                            .h_full()
                            .w(relative(progress_width))
                            .rounded_full()
                            .bg(colors.accent)
                            .into_any_element()
                    }),
            )
            .child(
                // Metrics row: Clean typographic metrics
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(12.))
                    .text_color(colors.text_secondary)
                    .child(
                        div().child(format!("{} / {}", format_bytes(done), total_label)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(
                                div()
                                    .text_color(colors.text_muted)
                                    .child(format_bytes_per_sec(speed)),
                            )
                            .child(
                                div()
                                    .text_color(colors.text_muted)
                                    .child(format!("{}: {}", t!("UpdateModal.progress.eta"), eta)),
                            ),
                    ),
            )
            .children(download_error.clone().map(|e| {
                div()
                    .flex_none()
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .border_1()
                    .border_color(with_alpha(colors.danger, 0.20))
                    .bg(with_alpha(colors.danger, 0.08))
                    .px(px(12.))
                    .py(px(8.))
                    .text_size(px(12.))
                    .line_height(px(17.))
                    .text_color(colors.danger)
                    .whitespace_normal()
                    .child(e)
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .pt(px(2.))
                    .child(cancel_btn),
            )
    };

    // ========== 更新日志 / 信息状态 UI ==========
    let external = {
        let mut el = div()
            .id("update-external")
            .flex()
            .items_center()
            .gap(px(5.))
            .cursor_pointer()
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(colors.accent)
            .hover(|this| this.text_color(colors.accent_hover));
        if let Some(url) = asset_url.clone() {
            el = el.on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                cx.open_url(&url);
            });
        } else {
            el = el.opacity(0.6);
        }
        el.child(
            svg()
                .path(lucide_gpui::icon!(external_link))
                .w(px(13.))
                .h(px(13.))
                .text_color(colors.accent),
        )
        .child(t!("UpdateModal.browser_download"))
    };

    let later = div()
        .id("update-later")
        .px(px(16.))
        .py(px(7.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .cursor_pointer()
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(colors.text_secondary)
        .hover(|this| this.bg(colors.surface).text_color(colors.text_primary))
        .on_mouse_down(MouseButton::Left, |_, _window, cx| {
            let now = Instant::now();
            cx.update_global(|u: &mut UpdateState, cx| {
                u.set_show_modal(false, now);
            });
        })
        .child(t!("UpdateModal.later"));

    let now_btn = {
        let mut el = div()
            .id("update-now")
            .px(px(18.))
            .py(px(7.))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .cursor_pointer()
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.btn_primary_text)
            .bg(colors.accent)
            .hover(|this| this.bg(colors.accent_hover))
            .active(|this| this.scale(crate::ui::theme::tokens::motion::PRESS_SCALE));
        if let Some(url) = asset_url.clone() {
            let filename_hint = asset_name.clone();
            el = el.on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                let url = url.clone();
                let filename_hint = filename_hint.clone();
                let task_id = format!("update-task-{}", uuid::Uuid::new_v4().to_string());
                cx.update_global(|u: &mut UpdateState, _cx| {
                    u.begin_download(task_id.clone(), task_manager::subscribe_task_updates());
                });
                cx.spawn(async move |cx| {
                    let args = crate::utils::updater::DownloadAndApplyArgs {
                        url,
                        filename_hint,
                        target_exe_path: None,
                        timeout_secs: Some(120),
                        auto_quit: Some(true),
                        task_id: Some(task_id),
                    };
                    let result = cx
                        .background_spawn(async move {
                            crate::utils::updater::download_and_apply_update_blocking(args)
                        })
                        .await;

                    match result {
                        Ok(value) => {
                            if value
                                .get("cancelled")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                            {
                                tracing::info!("download_and_apply_update cancelled by user");
                                let _ = cx.update_global(|u: &mut UpdateState, _cx| {
                                    u.cancel_download();
                                });
                            }
                        }
                        Err(err) => {
                            if crate::ui::state::update::is_cancelled_download_error(&err) {
                                tracing::info!("download_and_apply_update cancelled by user");
                                let _ = cx.update_global(|u: &mut UpdateState, _cx| {
                                    u.cancel_download();
                                });
                                return;
                            }

                            tracing::error!("download_and_apply_update error: {err}");
                            let _ = cx.update_global(|u: &mut UpdateState, _cx| {
                                u.fail_download(err.to_string());
                            });
                        }
                    }
                })
                .detach();
            });
        } else {
            el = el.opacity(0.6);
        }
        el.child(t!("UpdateModal.update_now"))
    };

    let actions = div()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(external)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(later)
                .child(now_btn),
        );

    let hint_row = div()
        .text_size(px(11.))
        .text_color(colors.text_muted)
        .child(t!("UpdateModal.hint_auto_check"));

    let changelog_container = div()
        .w_full()
        .flex_1()
        .min_h(changelog_scroll_height)
        .h(changelog_scroll_height)
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .bg(colors.surface)
        .border_1()
        .border_color(panel_edge_color)
        .overflow_hidden()
        .px(px(14.))
        .py(px(10.))
        .child(markdown_view.map_or_else(
            || {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(colors.text_muted)
                    .text_size(px(13.))
                    .child(t!("UpdateModal.preparing_changelog"))
                    .into_any_element()
            },
            render_markdown_view,
        ));

    let info_body = div()
        .flex_1()
        .min_h(px(0.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(changelog_container)
        .child(hint_row)
        .children(download_error.clone().map(|e| {
            div()
                .text_size(px(11.5))
                .text_color(colors.danger)
                .whitespace_normal()
                .child(e)
        }))
        .child(actions);

    // ========== 弹窗卡片主体 ==========
    let card = div()
        .w(card_w)
        .h(card_h)
        .flex()
        .flex_col()
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .overflow_hidden()
        .occlude()
        .bg(card_bg)
        .shadow(vec![BoxShadow {
            color: Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.16,
            },
            blur_radius: px(24.),
            spread_radius: px(0.),
            offset: point(px(0.), px(12.)),
        }])
        .border_1()
        .border_color(panel_edge_color)
        .child(
            div()
                .p(px(20.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .size_full()
                .child(header)
                .child(if downloading {
                    downloading_body.into_any_element()
                } else {
                    info_body.into_any_element()
                }),
        );

    let card_shell = div()
        .w(card_w)
        .h(card_h)
        .flex()
        .items_center()
        .justify_center()
        .child(card);

    modal::animated_modal_layer_with_content_offset(card_shell, overlay_bg, smooth_k, card_offset_y)
}
