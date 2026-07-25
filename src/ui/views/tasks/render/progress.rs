use super::*;
use crate::ui::views::tasks::ThreadSegment;

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

#[derive(Clone, Copy, Debug, PartialEq)]
struct ThreadSegmentGeometry {
    left: f32,
    width: f32,
    progress: f32,
}

fn thread_segment_geometry(segment: &ThreadSegment) -> Option<ThreadSegmentGeometry> {
    if !segment.active || segment.progress >= 1.0 {
        return None;
    }

    let left = segment.start_offset.clamp(0.0, 1.0);
    let right = (left + segment.width_fraction.max(0.0)).clamp(left, 1.0);
    let width = right - left;
    if width <= f32::EPSILON {
        return None;
    }

    Some(ThreadSegmentGeometry {
        left,
        width,
        progress: segment.progress.clamp(0.0, 1.0),
    })
}

fn thread_progress_overlay(
    segment: &ThreadSegment,
    accent: Hsla,
    colors: &ThemeColors,
) -> Option<AnyElement> {
    let geometry = thread_segment_geometry(segment)?;
    Some(
        div()
            .absolute()
            .top(px(0.))
            .bottom(px(0.))
            .left(relative(geometry.left))
            .w(relative(geometry.width))
            .border_l_1()
            .border_r_1()
            .border_color(Hsla {
                a: 0.72,
                ..colors.surface
            })
            .bg(Hsla { a: 0.18, ..accent })
            .child(
                div()
                    .h_full()
                    .w(relative(geometry.progress))
                    .bg(Hsla { a: 0.82, ..accent }),
            )
            .into_any_element(),
    )
}

pub(crate) fn progress_panel(
    _task_id: &str,
    kind: TaskVisualKind,
    colors: &ThemeColors,
    status: &str,
    percent_opt: Option<f64>,
    thread_segments: &[ThreadSegment],
) -> Div {
    let progress_mode = progress_mode(status, percent_opt);
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
                .bg(fill);
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
            bar = bar.child(
                div()
                    .h_full()
                    .w(relative(progress.max(0.0)))
                    .rounded_full()
                    .bg(fill),
            );
        }
    }

    if thread_segments.len() > 1 && !matches!(status, "completed" | "cancelled" | "error") {
        bar = bar.children(
            thread_segments
                .iter()
                .filter_map(|segment| thread_progress_overlay(segment, fill, colors)),
        );
    }

    bar
}

#[cfg(test)]
mod tests {
    use super::{TaskProgressMode, ThreadSegmentGeometry, progress_mode, thread_segment_geometry};
    use crate::ui::views::tasks::ThreadSegment;

    fn segment(
        active: bool,
        start_offset: f32,
        width_fraction: f32,
        progress: f32,
    ) -> ThreadSegment {
        ThreadSegment {
            active,
            start_offset,
            width_fraction,
            progress,
        }
    }

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

    #[test]
    fn active_thread_uses_its_file_range_inside_global_bar() {
        assert_eq!(
            thread_segment_geometry(&segment(true, 0.25, 0.125, 0.4)),
            Some(ThreadSegmentGeometry {
                left: 0.25,
                width: 0.125,
                progress: 0.4,
            })
        );
    }

    #[test]
    fn completed_thread_segment_disappears_after_merging() {
        assert_eq!(
            thread_segment_geometry(&segment(false, 0.25, 0.125, 1.0)),
            None
        );
        assert_eq!(
            thread_segment_geometry(&segment(true, 0.25, 0.125, 1.0)),
            None
        );
    }

    #[test]
    fn thread_range_is_clamped_to_global_bar() {
        let geometry = thread_segment_geometry(&segment(true, 0.9, 0.4, 0.5))
            .expect("valid thread range should produce geometry");

        assert!((geometry.left - 0.9).abs() < f32::EPSILON);
        assert!((geometry.width - 0.1).abs() < 1e-6);
        assert!((geometry.progress - 0.5).abs() < f32::EPSILON);
    }
}
