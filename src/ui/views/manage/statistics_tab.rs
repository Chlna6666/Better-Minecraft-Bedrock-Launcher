use super::*;
use crate::ui::animation::{settled_animation, stat_chart_bar_motion};
use chrono::{Days, Utc};
use std::time::{Duration, Instant};

const STAT_NUMBER_DURATION: Duration = Duration::from_millis(360);

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
        let i18n = cx.global::<I18n>();
        let text = match self.kind {
            StatMetricKind::Duration => format_duration(i18n, self.target),
            StatMetricKind::Count => t!("ManagePage.stats_count", count = self.target),
        };
        let value = div()
            .w_full()
            .text_size(px(20.))
            .font_weight(FontWeight::BOLD)
            .text_color(self.colors.text_primary)
            .child(text);

        let motion = if self.animate {
            Animation::from_spec(
                AnimationSpec::new(STAT_NUMBER_DURATION)
                    .fill_mode(FillMode::Both)
                    .ease(Easing::OutCubic),
            )
            .with_property(AnimationProperty::translation_opacity(
                point(px(0.0), px(8.0)),
                Point::default(),
                0.0,
                1.0,
            ))
        } else {
            settled_animation().with_property(AnimationProperty::translation_opacity(
                Point::default(),
                Point::default(),
                1.0,
                1.0,
            ))
        };

        value.with_animation(
            SharedString::from(format!("{}-presentation-{}", self.id.as_ref(), self.sequence)),
            motion,
            |value, _progress| value,
        )
    }
}

pub(super) fn render_statistics_tab(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
    now: Instant,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    let i18n = cx.global::<I18n>();
    let info = &version.game_info;
    let days = recent_days(info, 14);
    let max_sessions = days.iter().map(|day| day.sessions).max().unwrap_or(0).max(1);
    let max_play_time = days.iter().map(|day| day.play_time).max().unwrap_or(0).max(1);
    let animate =
        state.tab_animation_active(now) && !crate::core::ui_prefs::reduced_motion();
    let play_time_id = SharedString::from(format!(
        "manage-stat-total-play-time-{}",
        version.folder.as_ref()
    ));
    let launch_count_id = SharedString::from(format!(
        "manage-stat-launch-count-{}",
        version.folder.as_ref()
    ));

    div()
        .size_full()
        .overflow_y_scrollbar()
        .scrollbar_width(px(6.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .grid()
                .grid_cols(3)
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
                                    time.format("%Y-%m-%d %H:%M").to_string(),
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
        .min_h(px(86.))
        .p(px(14.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .bg(Hsla {
            a: 0.42,
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
        .text_size(px(20.))
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
    div()
        .p(px(14.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .bg(Hsla {
            a: 0.42,
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
                        .text_size(px(11.))
                        .text_color(colors.text_secondary)
                        .child(subtitle),
                ),
        )
        .child(
            div()
                .h(px(180.))
                .flex()
                .items_end()
                .gap(px(6.))
                .children(days.iter().enumerate().map(|(index, day)| {
                    let current = value(day);
                    let height = if current == 0 {
                        2.0
                    } else {
                        10.0 + 130.0 * current as f32 / maximum as f32
                    };
                    let bar = div()
                        .w_full()
                        .max_w(px(30.))
                        .h(px(height))
                        .rounded_t(px(5.))
                        .bg(Hsla { a: 0.72, ..color })
                        .with_animation(
                            SharedString::from(format!(
                                "manage-stat-{animation_scope}-{animation_sequence}-{index}"
                            )),
                            if animate {
                                stat_chart_bar_motion(index)
                            } else {
                                settled_animation().with_property(
                                    AnimationProperty::vertical_reveal(
                                        VerticalRevealEdge::Bottom,
                                        1.0,
                                        1.0,
                                    ),
                                )
                            },
                            |bar, _progress| bar,
                        )
                        .into_any_element();

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
                                .text_size(px(9.))
                                .text_color(colors.text_secondary)
                                .child(value_label(current)),
                        )
                        .child(bar)
                        .child(
                            div()
                                .text_size(px(9.))
                                .text_color(colors.text_secondary)
                                .child(day.date.format("%m/%d").to_string()),
                        )
                })),
        )
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
