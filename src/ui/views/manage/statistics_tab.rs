use super::*;
use crate::ui::animation::{stat_chart_bar_cap_motion, stat_chart_bar_fill_motion};
use chrono::{Days, Utc};
use std::time::{Duration, Instant};

const STAT_NUMBER_DURATION: Duration = Duration::from_millis(420);

fn count_at_progress(target: u64, progress: f32) -> u64 {
    if progress >= 1.0 {
        target
    } else {
        ((target as f64) * f64::from(progress.clamp(0.0, 1.0))).round() as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatMetricKind {
    Duration,
    Count,
}

#[derive(IntoElement)]
struct AnimatedStatValue {
    id: SharedString,
    sequence: u64,
    target: u64,
    kind: StatMetricKind,
    colors: ThemeColors,
    animate: bool,
}

impl AnimatedStatValue {
    fn new(
        id: impl Into<SharedString>,
        sequence: u64,
        target: u64,
        kind: StatMetricKind,
        colors: &ThemeColors,
        animate: bool,
    ) -> Self {
        Self {
            id: id.into(),
            sequence,
            target,
            kind,
            colors: *colors,
            animate,
        }
    }
}

impl RenderOnce for AnimatedStatValue {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let i18n = cx.global::<I18n>().clone();
        let value = div()
            .w_full()
            .text_size(px(20.))
            .font_weight(FontWeight::BOLD)
            .text_color(self.colors.text_primary);

        // Numeric content must be sampled on the UI lane; opacity alone leaves the
        // displayed number at its final value for the entire entrance animation.
        if self.animate && self.target > 0 {
            let target = self.target;
            let kind = self.kind;
            value
                .with_animation(
                    SharedString::from(format!("{}-count-{}", self.id.as_ref(), self.sequence)),
                    Animation::from_spec(
                        AnimationSpec::new(STAT_NUMBER_DURATION)
                            .fill_mode(FillMode::Both)
                            .ease(Easing::OutCubic),
                    ),
                    move |element, progress| {
                        let current = count_at_progress(target, progress);
                        let label = match kind {
                            StatMetricKind::Duration => format_duration(&i18n, current),
                            StatMetricKind::Count => t!("ManagePage.stats_count", count = current),
                        };
                        element.child(label)
                    },
                )
                .into_any_element()
        } else {
            let label = match self.kind {
                StatMetricKind::Duration => format_duration(&i18n, self.target),
                StatMetricKind::Count => t!("ManagePage.stats_count", count = self.target),
            };
            value.child(label).into_any_element()
        }
    }
}

pub(super) fn render_statistics_tab(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
    scroll_handle: &ScrollHandle,
    now: Instant,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    let i18n = cx.global::<I18n>();
    let info = &version.game_info;
    let days = recent_days(info, 14);
    let max_sessions = days
        .iter()
        .map(|day| day.sessions)
        .max()
        .unwrap_or(0)
        .max(1);
    let max_play_time = days
        .iter()
        .map(|day| day.play_time)
        .max()
        .unwrap_or(0)
        .max(1);
    let animate = state.tab_animation_active(now) && !crate::core::ui_prefs::reduced_motion();
    let play_time_id = SharedString::from(format!(
        "manage-stat-total-play-time-{}",
        version.folder.as_ref()
    ));
    let launch_count_id = SharedString::from(format!(
        "manage-stat-launch-count-{}",
        version.folder.as_ref()
    ));

    div()
        .id("manage-statistics-scroll")
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .overflow_y_scrollbar()
        .track_scroll(scroll_handle)
        .scrollbar_width(px(6.))
        .child(
            div()
                .w_full()
                .min_w(px(0.))
                .px(px(5.))
                .pt(px(8.))
                .pb(px(22.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .grid()
                        .grid_cols(3)
                        .flex_none()
                        .gap(px(10.))
                        .child(stat_card(
                            colors,
                            t!("ManagePage.stats_total_play_time"),
                            AnimatedStatValue::new(
                                play_time_id,
                                state.tab_anim_seq,
                                info.total_play_time,
                                StatMetricKind::Duration,
                                colors,
                                animate,
                            ),
                        ))
                        .child(stat_card(
                            colors,
                            t!("ManagePage.stats_launch_count"),
                            AnimatedStatValue::new(
                                launch_count_id,
                                state.tab_anim_seq,
                                info.total_sessions,
                                StatMetricKind::Count,
                                colors,
                                animate,
                            ),
                        ))
                        .child(stat_card(
                            colors,
                            t!("ManagePage.stats_last_launch"),
                            stat_text(
                                colors,
                                info.last_play_time.map_or_else(
                                    || t!("ManagePage.stats_never_launched"),
                                    |time| {
                                        SharedString::from(
                                            time.format("%Y-%m-%d\n%H:%M").to_string(),
                                        )
                                    },
                                ),
                            ),
                        )),
                )
                .child(chart_card(
                    colors,
                    t!("ManagePage.stats_daily_launches"),
                    t!("ManagePage.stats_last_14_days"),
                    "launches",
                    state.tab_anim_seq,
                    animate,
                    &days,
                    max_sessions,
                    |day| day.sessions,
                    |value| t!("ManagePage.stats_count", count = value),
                    colors.accent,
                ))
                .child(chart_card(
                    colors,
                    t!("ManagePage.stats_daily_play_time"),
                    t!("ManagePage.stats_last_14_days"),
                    "play-time",
                    state.tab_anim_seq,
                    animate,
                    &days,
                    max_play_time,
                    |day| day.play_time,
                    |value| format_duration(i18n, value),
                    colors.stat_green_text,
                ))
        )
        .into_any_element()
}

#[derive(Clone, Copy)]
struct DailyPoint {
    date: chrono::NaiveDate,
    sessions: u64,
    play_time: u64,
}

fn recent_days(info: &crate::core::version::game_info::GameInfo, count: u64) -> Vec<DailyPoint> {
    let today = Utc::now().date_naive();
    (0..count)
        .rev()
        .filter_map(|offset| today.checked_sub_days(Days::new(offset)))
        .map(|date| {
            let daily = info.daily.get(&date).cloned().unwrap_or_default();
            DailyPoint {
                date,
                sessions: daily.sessions,
                play_time: daily.play_time,
            }
        })
        .collect()
}

fn stat_card(colors: &ThemeColors, label: SharedString, value: impl IntoElement) -> Div {
    div()
        .min_h(px(102.))
        .min_w(px(0.))
        .p(px(16.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(colors.border)
        .bg(Hsla {
            a: 0.62,
            ..colors.surface
        })
        .flex()
        .flex_col()
        .justify_between()
        .child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child(label),
        )
        .child(value)
}

fn stat_text(colors: &ThemeColors, value: SharedString) -> Div {
    div()
        .w_full()
        .text_size(px(18.))
        .font_weight(FontWeight::BOLD)
        .text_color(colors.text_primary)
        .child(value)
}

#[allow(clippy::too_many_arguments)]
fn chart_card(
    colors: &ThemeColors,
    title: SharedString,
    subtitle: SharedString,
    animation_scope: &'static str,
    animation_sequence: u64,
    animate: bool,
    days: &[DailyPoint],
    maximum: u64,
    value: impl Fn(&DailyPoint) -> u64,
    value_label: impl Fn(u64) -> SharedString,
    color: Hsla,
) -> Div {
    let total = days
        .iter()
        .fold(0u64, |sum, day| sum.saturating_add(value(day)));

    div()
        .flex_none()
        .min_w(px(0.))
        .p(px(16.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(colors.border)
        .bg(Hsla {
            a: 0.58,
            ..colors.surface
        })
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.text_secondary)
                                .child(subtitle),
                        )
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color)
                                .child(value_label(total)),
                        ),
                ),
        )
        .child(div().h(px(178.)).w_full().flex().items_end().gap(px(6.)).children(
            days.iter().enumerate().map(|(index, day)| {
                let current = value(day);
                let height = if current == 0 {
                    2.0
                } else {
                    10.0 + 130.0 * current as f32 / maximum as f32
                };
                // Keep the column width fixed. Animate a rectangular body from the
                // baseline, while an independent rounded cap follows its top edge.
                // A uniform scale changes the column width; clipping a single
                // rounded quad exposes square corners mid-animation.
                let bar = if current == 0 {
                    div()
                        .w_full()
                        .max_w(px(30.))
                        .h(px(2.))
                        .rounded(px(2.))
                        .bg(Hsla { a: 0.24, ..colors.border })
                        .into_any_element()
                } else {
                    let cap_height = height.min(9.0);
                    let fill_height = height - cap_height;
                    let bar_color = Hsla { a: 0.78, ..color };
                    let cap = div()
                        .w_full()
                        .h(px(cap_height))
                        .flex_none()
                        .rounded_t(px(5.))
                        .bg(bar_color);
                    let fill = div()
                        .w_full()
                        .min_h(px(0.))
                        .flex_1()
                        .rounded_b(px(5.))
                        .bg(bar_color);
                    let (cap, fill) = if animate {
                        let key = format!(
                            "manage-stat-{animation_scope}-{animation_sequence}-{index}"
                        );
                        (
                            cap.with_visual_animation(
                                SharedString::from(format!("{key}-cap")),
                                stat_chart_bar_cap_motion(index, fill_height),
                            )
                            .expect("animation declares presentation properties")
                            .into_any_element(),
                            fill.with_visual_animation(
                                SharedString::from(format!("{key}-fill")),
                                stat_chart_bar_fill_motion(index),
                            )
                            .expect("animation declares presentation properties")
                            .into_any_element(),
                        )
                    } else {
                        (cap.into_any_element(), fill.into_any_element())
                    };
                    div()
                        .w_full()
                        .max_w(px(30.))
                        .h(px(height))
                        .flex()
                        .flex_col()
                        .child(cap)
                        .child(fill)
                        .into_any_element()
                };

                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_end()
                    .gap(px(5.))
                    .child(
                        div()
                            .h(px(12.))
                            .text_size(px(9.))
                            .text_color(colors.text_secondary)
                            .child(if current == 0 {
                                SharedString::from("")
                            } else {
                                value_label(current)
                            }),
                    )
                    .child(bar)
                    .child(
                        div()
                            .text_size(px(9.))
                            .text_color(colors.text_secondary)
                            .child(day.date.format("%m/%d").to_string()),
                    )
            }),
        ))
}

fn format_duration(_i18n: &I18n, seconds: u64) -> SharedString {
    if seconds >= 3_600 {
        t!(
            "ManagePage.stats_hours",
            hours = format!("{:.1}", seconds as f64 / 3_600.0)
        )
    } else {
        t!("ManagePage.stats_minutes", minutes = seconds / 60)
    }
}

#[cfg(test)]
mod statistics_tests {
    use super::count_at_progress;

    #[test]
    fn count_animation_bounds_and_exact_terminal_value() {
        assert_eq!(count_at_progress(81, 0.0), 0);
        assert_eq!(count_at_progress(81, 0.5), 41);
        assert_eq!(count_at_progress(81, 1.0), 81);
        assert_eq!(count_at_progress(u64::MAX, 1.0), u64::MAX);
    }
}
