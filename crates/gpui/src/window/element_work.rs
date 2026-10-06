//! Temporary local probe: attributes one frame's prepaint and paint time to concrete element types.
//!
//! This is diagnostic scaffolding used to locate a frame-generation hot spot on the GPUI foreground
//! thread. It is not a public API and must be removed once the investigation is closed.

use std::cell::RefCell;
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

thread_local! {
    static FRAME_WORK: RefCell<ElementWorkFrame> = RefCell::new(ElementWorkFrame::default());
}

pub(crate) fn reset() {
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
    let stack_depth = FRAME_WORK.with_borrow_mut(|frame| {
        frame.stack.push(Duration::ZERO);
        frame.stack.len()
    });

    let started_at = Instant::now();
    let result = run(window, cx);
    let elapsed = started_at.elapsed();

    FRAME_WORK.with_borrow_mut(|frame| {
        let entry_index = stack_depth - 1;
        if entry_index >= frame.stack.len() {
            return;
        }
        let descendants: Duration = frame.stack[entry_index + 1..].iter().copied().sum();
        frame.stack.truncate(entry_index);
        let sample = ElementWorkSample {
            type_name,
            elapsed: elapsed.saturating_sub(descendants),
        };
        match phase {
            ElementWorkPhase::Prepaint => frame.prepaint.push(sample),
            ElementWorkPhase::Paint => frame.paint.push(sample),
        }
    });

    result
}
