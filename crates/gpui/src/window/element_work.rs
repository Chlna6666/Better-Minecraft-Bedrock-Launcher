//! Optional frame-local prepaint/paint probe.
//!
//! Enable with `BMCBL_GPUI_ELEMENT_WORK=1` while investigating long frames.
//! Normally disabled: taking two timestamps and multiple TLS borrows for every
//! nested element can perturb the very 240Hz frame budget being measured.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::{App, Window};

#[derive(Clone, Copy, Debug)]
struct ElementWorkSample {
    type_name: &'static str,
    elapsed: Duration,
}

#[derive(Default)]
struct ElementWorkFrame {
    prepaint: Vec<ElementWorkSample>,
    paint: Vec<ElementWorkSample>,
    /// One entry per element whose lifecycle callback is currently on the stack.
    stack: Vec<Duration>,
}

pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("BMCBL_GPUI_ELEMENT_WORK").is_ok_and(|v| v == "1"))
}

/// Per-frame element work summary.
#[derive(Default)]
pub(crate) struct ElementWorkSummary {
    /// Element prepaint callbacks executed in the frame.
    pub(crate) prepaint_elements: usize,
    /// Element paint callbacks executed in the frame.
    pub(crate) paint_elements: usize,
    /// Heaviest element self-times during prepaint.
    pub(crate) prepaint: Vec<(String, Duration)>,
    /// Heaviest element self-times during paint.
    pub(crate) paint: Vec<(String, Duration)>,
}

impl ElementWorkFrame {
    fn finish_sample(
        &mut self,
        type_name: &'static str,
        phase: ElementWorkPhase,
        elapsed: Duration,
    ) {
        // Pop exactly this callback. The previous implementation summed a
        // truncated descendant slice, so outer Div/CachedView samples were
        // inclusive, sometimes totalling >100ms in an actual 19ms frame.
        let descendants = self.stack.pop().expect("element work stack underflow");
        if let Some(parent) = self.stack.last_mut() {
            *parent = parent.saturating_add(elapsed);
        }
        let sample = ElementWorkSample {
            type_name,
            elapsed: elapsed.saturating_sub(descendants),
        };
        match phase {
            ElementWorkPhase::Prepaint => self.prepaint.push(sample),
            ElementWorkPhase::Paint => self.paint.push(sample),
        }
    }
}

thread_local! {
    static FRAME_WORK: RefCell<ElementWorkFrame> = RefCell::new(ElementWorkFrame::default());
}

pub(crate) fn reset() {
    if !enabled() {
        return;
    }
    FRAME_WORK.with_borrow_mut(|frame| {
        frame.prepaint.clear();
        frame.paint.clear();
        frame.stack.clear();
    });
}

fn summarized(mut samples: Vec<ElementWorkSample>) -> Vec<(String, Duration)> {
    samples.sort_by_key(|sample| std::cmp::Reverse(sample.elapsed));
    let mut summarized: Vec<(String, Duration)> = Vec::new();
    for sample in samples {
        if let Some(existing) = summarized
            .iter_mut()
            .find(|(type_name, _)| *type_name == *sample.type_name)
        {
            existing.1 += sample.elapsed;
        } else {
            summarized.push((sample.type_name.to_owned(), sample.elapsed));
        }
    }
    summarized.sort_by_key(|(_, elapsed)| std::cmp::Reverse(*elapsed));
    summarized.truncate(12);
    summarized
}

/// Returns the per-element-type aggregates and element counts for the recorded frame.
pub(crate) fn take() -> ElementWorkSummary {
    if !enabled() {
        return ElementWorkSummary::default();
    }
    FRAME_WORK.with_borrow_mut(|frame| ElementWorkSummary {
        prepaint_elements: frame.prepaint.len(),
        paint_elements: frame.paint.len(),
        prepaint: summarized(std::mem::take(&mut frame.prepaint)),
        paint: summarized(std::mem::take(&mut frame.paint)),
    })
}

/// Which lifecycle phase a sample belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ElementWorkPhase {
    Prepaint,
    Paint,
}

/// Runs one element lifecycle callback and records its own wall time under the element's concrete
/// type. Nested element callbacks are excluded, so the reported value is the element's own work
/// rather than an inclusive subtree total.
pub(crate) fn measure_named<R>(
    type_name: &'static str,
    phase: ElementWorkPhase,
    window: &mut Window,
    cx: &mut App,
    run: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    if !enabled() {
        return run(window, cx);
    }
    FRAME_WORK.with_borrow_mut(|frame| frame.stack.push(Duration::ZERO));
    let started_at = Instant::now();
    let result = run(window, cx);
    let elapsed = started_at.elapsed();
    FRAME_WORK.with_borrow_mut(|frame| frame.finish_sample(type_name, phase, elapsed));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_element_work_excludes_descendants_from_parent_self_time() {
        let mut frame = ElementWorkFrame::default();
        frame.stack.push(Duration::ZERO); // parent
        frame.stack.push(Duration::ZERO); // first child
        frame.finish_sample("child-a", ElementWorkPhase::Prepaint, Duration::from_millis(3));
        frame.stack.push(Duration::ZERO); // second child
        frame.finish_sample("child-b", ElementWorkPhase::Prepaint, Duration::from_millis(5));
        frame.finish_sample("parent", ElementWorkPhase::Prepaint, Duration::from_millis(12));
        assert!(frame.stack.is_empty());
        assert_eq!(frame.prepaint[0].elapsed, Duration::from_millis(3));
        assert_eq!(frame.prepaint[1].elapsed, Duration::from_millis(5));
        assert_eq!(frame.prepaint[2].elapsed, Duration::from_millis(4));
    }

    #[test]
    fn multilevel_nesting_does_not_double_count_grandchildren() {
        let mut frame = ElementWorkFrame::default();
        frame.stack.push(Duration::ZERO);
        frame.stack.push(Duration::ZERO);
        frame.stack.push(Duration::ZERO);
        frame.finish_sample("grandchild", ElementWorkPhase::Paint, Duration::from_millis(2));
        frame.finish_sample("child", ElementWorkPhase::Paint, Duration::from_millis(4));
        frame.finish_sample("parent", ElementWorkPhase::Paint, Duration::from_millis(7));
        let self_total: Duration = frame.paint.iter().map(|s| s.elapsed).sum();
        assert_eq!(self_total, Duration::from_millis(7));
        assert_eq!(frame.paint.last().unwrap().elapsed, Duration::from_millis(3));
    }
}
