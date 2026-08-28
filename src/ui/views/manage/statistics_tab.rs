use super::*;
use crate::ui::components::scroll::ScrollableElement as _;
use chrono::{Days, Utc};

pub(super) fn render_statistics_tab(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
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
                    format_duration(i18n, info.total_play_time),
                ))
                .child(stat_card(
                    colors,
                    t!("ManagePage.stats_launch_count"),
                    t!("ManagePage.stats_count", count = info.total_sessions),
                ))
                .child(stat_card(
                    colors,
                    t!("ManagePage.stats_last_launch"),
                    info.last_play_time.map_or_else(
                        || t!("ManagePage.stats_never_launched"),
                        |time| SharedString::from(time.format("%Y-%m-%d %H:%M").to_string()),
                    ),
                )),
        )
        .child(chart_card(
            colors,
            t!("ManagePage.stats_daily_launches"),
            t!("ManagePage.stats_last_14_days"),
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

fn stat_card(colors: &ThemeColors, label: SharedString, value: SharedString) -> Div {
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
        .child(
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(colors.text_primary)
                .child(value),
        )
}

fn chart_card(
    colors: &ThemeColors,
    title: SharedString,
    subtitle: SharedString,
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
                .children(days.iter().map(|day| {
                    let current = value(day);
                    let height = if current == 0 {
                        2.0
                    } else {
                        10.0 + 130.0 * current as f32 / maximum as f32
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
                                .text_size(px(9.))
                                .text_color(colors.text_secondary)
                                .child(value_label(current)),
                        )
                        .child(
                            div()
                                .w_full()
                                .max_w(px(30.))
                                .h(px(height))
                                .rounded_t(px(5.))
                                .bg(Hsla { a: 0.72, ..color }),
                        )
                        .child(
                            div()
                                .text_size(px(9.))
                                .text_color(colors.text_secondary)
                                .child(day.date.format("%m/%d").to_string()),
                        )
                })),
        )
}

fn format_duration(i18n: &I18n, seconds: u64) -> SharedString {
    if seconds >= 3_600 {
        t!(
            "ManagePage.stats_hours",
            hours = format!("{:.1}", seconds as f64 / 3_600.0)
        )
    } else {
        t!("ManagePage.stats_minutes", minutes = seconds / 60)
    }
}
