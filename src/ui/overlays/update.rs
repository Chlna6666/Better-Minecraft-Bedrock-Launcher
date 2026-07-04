use crate::tasks::task_manager::{self, TaskSnapshot};
use crate::ui::animation::repeating_linear_motion;
use crate::ui::components::adaptive::{
    AdaptiveModalSpec, AdaptiveSizeClass, WindowMetrics, adaptive_modal_size,
};
use crate::ui::components::markdown_renderer::{
    MarkdownDocument, render_markdown_document_limited,
};
use crate::ui::components::modal;
use crate::ui::components::scroll::ScrollableElement as _;
use crate::ui::state::i18n::I18n;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::utils::format_bytes::{format_bytes, format_bytes_per_sec};
use crate::utils::updater::ReleaseSummary;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui::{AnimationExt, StatefulInteractiveElement as _};
use lucide_gpui::icons as lucide_icons;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const UPDATE_MODAL_MARGIN_PX: f32 = 40.0;
const UPDATE_MODAL_MIN_WIDTH_PX: f32 = 320.0;
const UPDATE_MODAL_MAX_WIDTH_PX: f32 = 520.0;
const UPDATE_MODAL_MIN_HEIGHT_PX: f32 = 360.0;
const UPDATE_INFO_MAX_HEIGHT_PX: f32 = 640.0;
const UPDATE_DOWNLOAD_MAX_HEIGHT_PX: f32 = 420.0;
const UPDATE_DOWNLOAD_ERROR_MAX_HEIGHT_PX: f32 = 470.0;
const UPDATE_INFO_FIXED_CHROME_PX: f32 = 304.0;
const UPDATE_CHANGELOG_MIN_SCROLL_PX: f32 = 48.0;
const UPDATE_CHANGELOG_MAX_SCROLL_PX: f32 = 336.0;

pub struct UpdateMarkdownView {
    release_tag: String,
    document: Arc<MarkdownDocument>,
    colors: ThemeColors,
    dark: bool,
    active: bool,
}

impl UpdateMarkdownView {
    pub fn new(
        release_tag: String,
        document: Arc<MarkdownDocument>,
        colors: ThemeColors,
        dark: bool,
    ) -> Self {
        Self {
            release_tag,
            document,
            colors,
            dark,
            active: true,
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
        self.release_tag = release_tag;
        self.document = document;
        self.colors = colors;
        self.dark = dark;
        self.active = true;
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
        render_markdown_document_limited(
            self.document.as_ref(),
            &self.colors,
            self.dark,
            self.document.blocks.len(),
        )
        .w_full()
    }
}

fn render_markdown_view(view: Entity<UpdateMarkdownView>) -> AnyElement {
    view.into_any_element()
}

fn format_date(iso: Option<String>, i18n: &I18n) -> SharedString {
    let Some(s) = iso else {
        return i18n.t("UpdateModal.date.unknown");
    };
    let d = if s.len() >= 10 { &s[0..10] } else { s.as_str() };
    let mut it = d.split('-');
    let y = it.next().unwrap_or(d);
    let m = it.next().unwrap_or("");
    let dd = it.next().unwrap_or("");
    if !m.is_empty() && !dd.is_empty() {
        let month = m.trim_start_matches('0');
        let day = dd.trim_start_matches('0');
        i18n.t_args(
            "UpdateModal.date.full",
            crate::i18n_args![("year", y), ("month", month), ("day", day)],
        )
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

fn stat_metric(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    icon_path: &'static str,
    icon_color: Hsla,
    colors: &ThemeColors,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let value: SharedString = value.into();
    let icon_bg = Hsla {
        h: icon_color.h,
        s: icon_color.s * 0.35,
        l: icon_color.l,
        a: 0.13,
    };

    div()
        .flex_1()
        .flex_basis(px(0.))
        .min_w(px(0.))
        .min_h(px(50.))
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(28.))
                        .h(px(28.))
                        .rounded(px(9.))
                        .bg(icon_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(svg().path(icon_path).size(px(14.)).text_color(icon_color)),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_muted)
                        .whitespace_nowrap()
                        .child(label),
                ),
        )
        .child(
            div()
                .min_w(px(0.))
                .text_size(px(13.))
                .line_height(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .whitespace_normal()
                .child(value),
        )
}

fn accent_tint(color: Hsla, alpha: f32) -> Hsla {
    Hsla {
        s: (color.s * 0.75).min(1.0),
        l: if color.l > 0.58 {
            (color.l + 0.08).min(1.0)
        } else {
            (color.l + 0.34).min(1.0)
        },
        a: alpha.clamp(0.0, 1.0),
        ..color
    }
}

pub fn render_update_modal(
    release: ReleaseSummary,
    markdown_view: Option<Entity<UpdateMarkdownView>>,
    changelog_scroll_handle: ScrollHandle,
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
) -> impl IntoElement {
    let tag = release.tag.clone();
    let title = release.name.clone().unwrap_or_else(|| {
        i18n.t_args(
            "UpdateModal.default_title",
            crate::i18n_args![("tag", &tag)],
        )
        .to_string()
    });
    let date = format_date(release.published_at.clone(), i18n);
    let size = release
        .asset_size
        .map(format_bytes)
        .unwrap_or_else(|| i18n.t("UpdateModal.size.unknown").to_string());
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
            i18n.t("common.preview"),
            colors.badge_beta_bg,
            colors.badge_beta_text,
        )
    } else {
        (
            i18n.t("common.release"),
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
    let changelog_container_height = changelog_scroll_height + px(59.0);
    let card_h = if downloading {
        available_card_h
    } else {
        px(
            (UPDATE_INFO_FIXED_CHROME_PX + changelog_scroll_height / px(1.0))
                .min(available_card_h / px(1.0))
                .max(UPDATE_MODAL_MIN_HEIGHT_PX.min(available_card_h / px(1.0))),
        )
    };
    let compact_modal =
        window_metrics.width_class == AdaptiveSizeClass::Compact || card_w < px(420.0);
    let motion_k = modal_factor.clamp(0.0, 1.06);
    let k = motion_k.min(1.0);
    // Snap near-complete animation to exact 1.0 to avoid subpixel alpha fringes on rounded edges.
    let k = if k > 0.996 { 1.0 } else { k };
    let smooth_k = (k * k * (3.0 - 2.0 * k)).clamp(0.0, 1.0);
    let card_offset_y = if modal_visible {
        px((1.0 - smooth_k) * 14.0 - (motion_k - 1.0).max(0.0) * 6.0)
    } else {
        px((1.0 - smooth_k) * 10.0)
    };
    // Keep backdrop readable while preserving visible blur.
    // Keep backdrop dark but avoid modal::frosted_backdrop_base white highlight branch
    // which can produce bright fringes around rounded modal edges in dark themes.
    let overlay_bg = hsla(0., 0., 0.08, 0.32);
    let asset_url = release.asset_url.clone();
    let asset_name = release.asset_name.clone();
    let card_bg = colors.bg;

    // ========== Header 区域 ==========
    let header = div()
        .flex()
        .items_start()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .flex_1()
                .min_w(px(0.))
                .child(
                    // 徽章组
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            // 类型徽章
                            div()
                                .px(px(8.))
                                .py(px(3.))
                                .rounded(px(99.))
                                .bg(channel_bg)
                                .text_size(px(11.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(channel_fg)
                                .child(channel_label),
                        )
                        .child(
                            // 版本徽章
                            div()
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .bg(colors.surface)
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.text_secondary)
                                .child(tag.clone()),
                        ),
                )
                .child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                ),
        )
        .child(
            // 关闭按钮
            div()
                .w(px(32.))
                .h(px(32.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.text_muted)
                .when(downloading, |this| {
                    this.opacity(0.38).cursor(CursorStyle::OperationNotAllowed)
                })
                .when(!downloading, |this| {
                    this.cursor_pointer()
                        .hover(|s| s.bg(colors.surface))
                        .on_mouse_down(MouseButton::Left, |_, _window, cx| {
                            let now = Instant::now();
                            cx.update_global(|u: &mut UpdateState, cx| {
                                u.set_show_modal(false, now);
                            });
                        })
                })
                .child(
                    svg()
                        .path(lucide_icons::icon_x())
                        .w(px(16.))
                        .h(px(16.))
                        .text_color(colors.text_muted),
                ),
        );

    // ========== 进度条组件 - 匹配 Web 版本 ==========
    let progress_panel = {
        let snap = snapshot.clone();
        let percent = snap
            .as_ref()
            .and_then(|s| s.percent)
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        let pct_label = format!("{:.0}", percent);
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
            i18n.t("UpdateModal.no_file").to_string()
        } else {
            format_bytes(display_total)
        };
        let is_indeterminate = snapshot_total == 0;
        let progress_ratio = ((percent as f32) / 100.0).clamp(0.0, 1.0);
        let progress_width = if progress_ratio > 0.0 {
            progress_ratio.max(0.035)
        } else {
            0.0
        };
        let (stage_label, stage_detail) = if is_extracting {
            (
                i18n.t("UpdateModal.progress.extracting"),
                i18n.t("UpdateModal.progress.organizing_files"),
            )
        } else if is_indeterminate {
            (
                i18n.t("UpdateModal.progress.downloading"),
                i18n.t("UpdateModal.progress.connecting_source"),
            )
        } else {
            (
                i18n.t("UpdateModal.progress.downloading"),
                i18n.t("UpdateModal.progress.install_after_download"),
            )
        };

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        div().w(px(7.)).h(px(7.)).rounded_full().bg(colors.accent),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(colors.text_primary)
                                            .child(stage_label),
                                    ),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .line_height(px(17.))
                                    .text_size(px(11.))
                                    .text_color(colors.text_muted)
                                    .child(stage_detail),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_baseline()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(28.))
                                    .line_height(px(28.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.accent)
                                    .child(pct_label),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text_secondary)
                                    .child("%"),
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(11.))
                    .w_full()
                    .rounded(px(99.))
                    .bg(with_alpha(colors.accent, 0.10))
                    .border_1()
                    .border_color(with_alpha(colors.accent, 0.16))
                    .relative()
                    .overflow_hidden()
                    .child(if is_indeterminate {
                        div()
                            .absolute()
                            .top(px(2.))
                            .bottom(px(2.))
                            .w(relative(0.36))
                            .rounded(px(99.))
                            .bg(linear_gradient(
                                90.0,
                                linear_color_stop(with_alpha(colors.progress_fill, 0.22), 0.0),
                                linear_color_stop(with_alpha(colors.accent_hover, 0.86), 1.0),
                            ))
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
                            .rounded(px(99.))
                            .bg(linear_gradient(
                                90.0,
                                linear_color_stop(colors.progress_fill, 0.0),
                                linear_color_stop(colors.accent_hover, 1.0),
                            ))
                            .shadow(vec![BoxShadow {
                                color: with_alpha(colors.accent_glow, 0.24),
                                blur_radius: px(16.0),
                                spread_radius: px(-3.0),
                                offset: point(px(0.0), px(0.0)),
                            }])
                            .child(
                                div()
                                    .absolute()
                                    .top(px(1.))
                                    .bottom(px(1.))
                                    .right(px(4.))
                                    .w(px(36.))
                                    .rounded(px(99.))
                                    .bg(with_alpha(colors.btn_primary_text, 0.20)),
                            )
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap(px(18.))
                    .border_t_1()
                    .border_color(panel_edge_color)
                    .pt(px(12.))
                    .child(stat_metric(
                        i18n.t("UpdateModal.progress.speed"),
                        format_bytes_per_sec(speed),
                        lucide_icons::icon_activity(),
                        colors.accent,
                        &colors,
                    ))
                    .child(stat_metric(
                        i18n.t("UpdateModal.progress.eta"),
                        eta,
                        lucide_icons::icon_clock(),
                        colors.stat_orange_text,
                        &colors,
                    ))
                    .child(stat_metric(
                        i18n.t("UpdateModal.progress.downloaded"),
                        format!("{} / {}", format_bytes(done), total_label),
                        lucide_icons::icon_database(),
                        colors.stat_green_text,
                        &colors,
                    )),
            )
    };

    // ========== 弹窗卡片主体 ==========
    let card = div()
        .w(card_w)
        .h(card_h)
        .flex()
        .flex_col()
        .rounded(px(16.))
        .overflow_hidden()
        .occlude()
        .bg(card_bg)
        .shadow(vec![BoxShadow {
            color: Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.30,
            },
            blur_radius: px(40.),
            spread_radius: px(0.),
            offset: point(px(0.), px(16.)),
        }])
        .border_0()
        .child(div().px(px(24.)).pt(px(24.)).pb(px(0.)).child(header))
        .child(
            div()
                .px(px(24.))
                .pt(px(12.))
                .pb(px(20.))
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(if downloading {
                    // 下载状态 UI
                    let cancel = div()
                        .id("update-cancel")
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_h(px(46.))
                        .px(px(18.))
                        .py(px(10.))
                        .rounded(px(10.))
                        .cursor_pointer()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.danger)
                        .bg(with_alpha(colors.danger, 0.08))
                        .border_1()
                        .border_color(with_alpha(colors.danger, 0.18))
                        .hover(|this| this.bg(with_alpha(colors.danger, 0.13)))
                        .on_mouse_down(MouseButton::Left, {
                            let id = task_id.clone();
                            move |_, _window, cx| {
                                if let Some(id) = id.clone() {
                                    if let Err(err) = thread::Builder::new()
                                        .name("update-cancel".to_string())
                                        .spawn(move || {
                                            task_manager::cancel_task(&id);
                                        })
                                    {
                                        eprintln!("failed to spawn update cancel thread: {err}");
                                    }
                                }
                                let now = Instant::now();
                                cx.update_global(|u: &mut UpdateState, cx| {
                                    u.cancel_download();
                                    u.set_show_modal(false, now);
                                });
                            }
                        })
                        .child(
                            svg()
                                .path(lucide_icons::icon_x())
                                .size(px(14.))
                                .text_color(colors.danger),
                        )
                        .child(i18n.t("UpdateModal.cancel_download"));

                    let is_extracting = snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.stage.as_ref() == "extracting");
                    let download_icon_path = if is_extracting {
                        lucide_icons::icon_package()
                    } else {
                        lucide_icons::icon_download()
                    };
                    let download_title = if is_extracting {
                        i18n.t("UpdateModal.summary.extracting_title")
                    } else {
                        i18n.t("UpdateModal.summary.downloading_title")
                    };
                    let download_detail = if is_extracting {
                        i18n.t_args(
                            "UpdateModal.summary.extracting_detail",
                            crate::i18n_args![("tag", &tag)],
                        )
                    } else {
                        i18n.t_args(
                            "UpdateModal.summary.file_detail",
                            crate::i18n_args![("tag", &tag), ("size", &size)],
                        )
                    };

                    let download_icon = div()
                        .relative()
                        .w(px(54.))
                        .h(px(54.))
                        .flex_none()
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded_full()
                                .bg(with_alpha(colors.accent, 0.16)),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset(px(3.))
                                .rounded_full()
                                .bg(with_alpha(colors.bg, 0.50)),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset(px(8.))
                                .rounded_full()
                                .bg(with_alpha(colors.bg, 0.88))
                                .border_1()
                                .border_color(with_alpha(colors.accent, 0.20)),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    svg()
                                        .path(download_icon_path)
                                        .size(px(20.))
                                        .text_color(colors.accent),
                                ),
                        );
                    let download_summary = div()
                        .w_full()
                        .rounded(px(14.))
                        .bg(accent_tint(colors.accent, 0.12))
                        .border_l_2()
                        .border_color(with_alpha(colors.accent, 0.70))
                        .px(px(14.))
                        .py(px(14.))
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .child(download_icon)
                        .child(
                            div()
                                .min_w(px(0.))
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(7.))
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(16.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(colors.text_primary)
                                        .child(download_title),
                                )
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .line_height(px(17.))
                                        .text_size(px(12.))
                                        .text_color(colors.text_secondary)
                                        .child(download_detail),
                                ),
                        );

                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(download_summary)
                        .child(progress_panel)
                        .children(download_error.clone().map(|e| {
                            div()
                                .flex_none()
                                .rounded(px(10.))
                                .border_1()
                                .border_color(with_alpha(colors.danger, 0.18))
                                .bg(with_alpha(colors.danger, 0.07))
                                .px(px(12.))
                                .py(px(9.))
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(colors.danger)
                                .whitespace_normal()
                                .child(e)
                        }))
                        .child(div().flex_none().flex().justify_end().pt(px(2.)).child(cancel))
                        .into_any_element()
                } else {
                    // 信息/更新日志 UI (参考 UpdateModal.tsx)
                    let hint_row = div()
                        .text_size(px(11.))
                        .text_color(colors.text_muted)
                        .child(i18n.t("UpdateModal.hint_auto_check"));

                    let external = {
                        let mut el = div()
                            .id("update-external")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .text_size(px(13.))
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
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .child(
                                    svg()
                                        .path(lucide_icons::icon_external_link())
                                        .w(px(14.))
                                        .h(px(14.))
                                        .text_color(colors.accent),
                                )
                                .child(i18n.t("UpdateModal.browser_download")),
                        )
                    };

                    let later = div()
                        .id("update-later")
                        .px(px(20.))
                        .py(px(10.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_secondary)
                        .hover(|this| this.bg(colors.surface))
                        .on_mouse_down(MouseButton::Left, |_, _window, cx| {
                            let now = Instant::now();
                            cx.update_global(|u: &mut UpdateState, cx| {
                                u.set_show_modal(false, now);
                            });
                        })
                        .child(i18n.t("UpdateModal.later"));

                    let now_btn = {
                        let mut el = div()
                            .id("update-now")
                            .px(px(20.))
                            .py(px(10.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(0., 0., 1., 1.0))
                            .bg(colors.accent)
                            .hover(|this| this.bg(colors.accent_hover));
                        if let Some(url) = asset_url.clone() {
                            let filename_hint = asset_name.clone();
                            el = el.on_mouse_down(MouseButton::Left, move |_, _window, cx| {
                                let url = url.clone();
                                let filename_hint = filename_hint.clone();
                                let task_id =
                                    format!("update-task-{}", uuid::Uuid::new_v4().to_string());
                                cx.update_global(|u: &mut UpdateState, cx| {
                                    u.begin_download(
                                        task_id.clone(),
                                        task_manager::subscribe_task_updates(),
                                    );
                                });
                                // 使用 cx.spawn 在后台执行下载（使用阻塞版本，内部会创建 Tokio runtime）
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
                                                tracing::info!(
                                                    "download_and_apply_update cancelled by user"
                                                );
                                                let _ = cx.update_global(
                                                    |u: &mut UpdateState, _cx| {
                                                        u.cancel_download();
                                                    },
                                                );
                                            }
                                        }
                                        Err(err) => {
                                            if crate::ui::state::update::is_cancelled_download_error(
                                                &err,
                                            ) {
                                                tracing::info!(
                                                    "download_and_apply_update cancelled by user"
                                                );
                                                let _ = cx.update_global(
                                                    |u: &mut UpdateState, _cx| {
                                                        u.cancel_download();
                                                    },
                                                );
                                                return;
                                            }

                                            tracing::error!(
                                                "download_and_apply_update error: {err}"
                                            );
                                            let _ = cx.update_global(|u: &mut UpdateState, _cx| {
                                                u.fail_download(err.to_string());
                                            });
                                        }
                                    }
                                })
                                .detach();
                            })
                        } else {
                            el = el.opacity(0.6);
                        }
                        el.child(i18n.t("UpdateModal.update_now"))
                    };

                    let actions = div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .when(!compact_modal, |this| this.justify_between())
                        .when(compact_modal, |this| this.flex_col().items_start())
                        .child(external)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .flex_wrap()
                                .when(compact_modal, |this| this.w_full().justify_end())
                                .gap(px(12.))
                                .child(later)
                                .child(now_btn),
                        );

                    let meta_grid = div()
                        .flex()
                        .flex_wrap()
                        .gap(px(12.))
                        .text_size(px(13.))
                        .text_color(colors.text_secondary)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .bg(colors.surface)
                                .border_1()
                                .border_color(panel_edge_color)
                                .rounded(px(8.))
                                .px(px(10.))
                                .py(px(6.))
                                .child(
                                    svg()
                                        .path(lucide_icons::icon_clock())
                                        .w(px(14.))
                                        .h(px(14.))
                                        .text_color(colors.text_secondary),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_secondary)
                                        .child(date),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .bg(colors.surface)
                                .border_1()
                                .border_color(panel_edge_color)
                                .rounded(px(8.))
                                .px(px(10.))
                                .py(px(6.))
                                .child(
                                    svg()
                                        .path(lucide_icons::icon_database())
                                        .w(px(14.))
                                        .h(px(14.))
                                        .text_color(colors.text_secondary),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.text_secondary)
                                        .child(size),
                                ),
                        );

                    let changelog_container = div()
                        .w_full()
                        .flex_none()
                        .h(changelog_container_height)
                        .min_h(changelog_container_height)
                        .rounded(px(12.))
                        .bg(colors.surface)
                        .border_1()
                        .border_color(panel_edge_color)
                        .overflow_hidden()
                        .p(px(14.))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    svg()
                                        .path(lucide_icons::icon_tag())
                                        .w(px(14.))
                                        .h(px(14.))
                                        .text_color(colors.text_muted),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(colors.text_muted)
                                        .child(i18n.t("UpdateModal.changelog")),
                                ),
                        )
                        .child(div().h(px(1.)).bg(panel_edge_color))
                        .child(
                            div()
                                .flex_none()
                                .h(changelog_scroll_height)
                                .min_h(changelog_scroll_height)
                                .relative()
                                .child(
                                    div()
                                        .id("update-changelog-scroll")
                                        .size_full()
                                        .overflow_y_scrollbar()
                                        .scrollbar_width(px(1.))
                                        .track_scroll(&changelog_scroll_handle)
                                        .child(
                                            div()
                                                .text_size(px(14.))
                                                .line_height(px(22.))
                                                .text_color(colors.text_secondary)
                                                .pr(px(8.))
                                                .pb(px(24.))
                                                .child(markdown_view.map_or_else(
                                                    || {
                                                        div()
                                                            .h_full()
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .text_color(colors.text_muted)
                                                            .child(i18n.t("UpdateModal.preparing_changelog"))
                                                            .into_any_element()
                                                    },
                                                    render_markdown_view,
                                                )),
                                        ),
                                ),
                        );

                    let info_body = div()
                        .w_full()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .child(meta_grid)
                        .child(changelog_container)
                        .child(hint_row)
                        .children(download_error.clone().map(|e| {
                            div()
                                .text_size(px(11.))
                                .text_color(colors.text_secondary)
                                .whitespace_normal()
                                .child(e)
                        }));

                    div()
                        .flex_none()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(16.))
                        .child(info_body)
                        .child(actions)
                        .into_any_element()
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
