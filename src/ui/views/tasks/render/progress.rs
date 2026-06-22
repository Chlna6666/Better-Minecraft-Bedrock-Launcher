use super::*;
use gpui::prelude::FluentBuilder as _;

#[derive(Clone, Copy, Debug, PartialEq)]
enum TaskProgressMode {
    Determinate(f32),
    Indeterminate,
    Idle(f32),
}

fn progress_mode(status: &str, percent_opt: Option<f64>) -> TaskProgressMode {
    if status == "running" {
        return percent_opt
            .map(|percent| TaskProgressMode::Determinate((percent as f32 / 100.0).clamp(0.0, 1.0)))
            .unwrap_or(TaskProgressMode::Indeterminate);
    }

    TaskProgressMode::Idle(
        percent_opt
            .map(|percent| (percent as f32 / 100.0).clamp(0.0, 1.0))
            .unwrap_or(0.0),
    )
}

pub(crate) fn progress_panel(
    _task_id: &str,
    kind: TaskVisualKind,
    colors: &ThemeColors,
    status: &str,
    percent_opt: Option<f64>,
) -> Div {
    let progress_mode = progress_mode(status, percent_opt);
    let progress = match progress_mode {
        TaskProgressMode::Determinate(progress) | TaskProgressMode::Idle(progress) => progress,
        TaskProgressMode::Indeterminate => 0.0,
    };
    let fill = task_status_accent(status, kind, colors);
    let track = Hsla {
        a: if status == "completed" { 0.18 } else { 0.10 },
        ..fill
    };

    let mut bar = div()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(track)
        .relative()
        .overflow_hidden();

    match progress_mode {
        TaskProgressMode::Determinate(progress) => {
            let fill_bar = div()
                .relative()
                .h_full()
                .w(relative(progress.max(0.0)))
                .rounded_full()
                .bg(fill)
                .when(progress < 0.04, |this| this.min_w(px(14.)));
            bar = bar.child(fill_bar);
        }
        TaskProgressMode::Indeterminate => {
            bar = bar.child(
                div()
                    .absolute()
                    .top(px(0.))
                    .bottom(px(0.))
                    .left(relative(0.16))
                    .right(relative(0.16))
                    .rounded_full()
                    .bg(Hsla { a: 0.68, ..fill })
                    .child(
                        div()
                            .absolute()
                            .top(px(0.))
                            .bottom(px(0.))
                            .left(relative(0.34))
                            .right(relative(0.34))
                            .rounded_full()
                            .bg(Hsla {
                                a: 0.18,
                                ..colors.surface
                            }),
                    ),
            );
        }
        TaskProgressMode::Idle(progress) => {
            let fill_bar = div()
                .h_full()
                .w(relative(progress.max(0.0)))
                .rounded_full()
                .bg(fill);
            bar = bar.child(fill_bar);
        }
    }

    bar
}

#[cfg(test)]
mod tests {
    use super::TaskProgressMode;
    use super::progress_mode;

    #[test]
    fn determinate_running_progress_uses_clamped_ratio() {
        assert_eq!(
            progress_mode("running", Some(125.0)),
            TaskProgressMode::Determinate(1.0)
        );
        assert_eq!(
            progress_mode("running", Some(25.0)),
            TaskProgressMode::Determinate(0.25)
        );
    }

    #[test]
    fn missing_running_ratio_selects_indeterminate_mode() {
        assert_eq!(
            progress_mode("running", None),
            TaskProgressMode::Indeterminate
        );
    }

    #[test]
    fn terminal_states_do_not_use_shimmer_modes() {
        assert_eq!(
            progress_mode("completed", Some(100.0)),
            TaskProgressMode::Idle(1.0)
        );
        assert_eq!(progress_mode("error", None), TaskProgressMode::Idle(0.0));
    }
}
