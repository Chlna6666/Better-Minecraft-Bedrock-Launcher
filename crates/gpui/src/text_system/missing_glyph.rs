use crate::SharedString;
use collections::FxHashSet;
use itertools::Itertools as _;
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

pub(super) const MAX_REPORTED_MISSING_GLYPHS: usize = 1024;

/// The spacing behavior required of a dynamically installed fallback font.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FallbackFontClass {
    /// A proportionally spaced fallback font.
    Proportional,
    /// A fixed-width fallback font.
    Monospace,
}

/// A grapheme cluster that exhausted all currently available font fallback.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MissingGlyph {
    grapheme: SharedString,
    font_class: FallbackFontClass,
}

impl MissingGlyph {
    /// Creates a missing-glyph report.
    pub fn new(grapheme: SharedString, font_class: FallbackFontClass) -> Self {
        Self {
            grapheme,
            font_class,
        }
    }

    /// Returns the unresolved grapheme cluster.
    pub fn grapheme(&self) -> &str {
        &self.grapheme
    }

    /// Returns the spacing behavior required of the fallback font.
    pub fn font_class(&self) -> FallbackFontClass {
        self.font_class
    }
}

/// Receives missing-glyph batches from a platform text shaper.
///
/// Implementations must remain nonblocking because reports can be emitted while the platform text
/// system holds its shaping lock.
pub trait MissingGlyphSink: Send + Sync {
    /// Reports grapheme clusters that exhausted font fallback.
    fn report(&self, missing_glyphs: Vec<MissingGlyph>);
}

#[derive(Default)]
struct MissingGlyphState {
    reported: FxHashSet<MissingGlyph>,
    reported_order: VecDeque<MissingGlyph>,
    generation: usize,
}

impl MissingGlyphState {
    fn reset(&mut self, generation: usize) {
        self.reported.clear();
        self.reported_order.clear();
        self.generation = generation;
    }
}

struct QueuedMissingGlyph {
    generation: usize,
    missing_glyph: MissingGlyph,
}

/// Lock-free producer endpoint used directly from shaping code.
///
/// Cross-report deduplication intentionally lives on the single consumer. Producers only dedupe
/// within one report batch and use a bounded nonblocking channel, so shaping never waits for App.
pub(super) struct MissingGlyphReporter {
    generation: Arc<AtomicUsize>,
    sender: async_channel::Sender<QueuedMissingGlyph>,
}

impl MissingGlyphSink for MissingGlyphReporter {
    fn report(&self, missing_glyphs: Vec<MissingGlyph>) {
        if self.sender.is_closed() {
            return;
        }

        let generation = self.generation.load(Ordering::Acquire);
        for missing_glyph in missing_glyphs.into_iter().unique() {
            let queued = QueuedMissingGlyph {
                generation,
                missing_glyph,
            };
            if self.sender.try_send(queued).is_err() {
                break;
            }
        }
    }
}

impl MissingGlyphReporter {
    pub(super) fn reset(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(test)]
    pub(super) fn report_for_test(&self, missing_glyphs: Vec<MissingGlyph>) {
        self.report(missing_glyphs);
    }
}

/// Single-consumer endpoint that owns cross-report deduplication state.
pub(crate) struct MissingGlyphReceiver {
    state: MissingGlyphState,
    generation: Arc<AtomicUsize>,
    receiver: async_channel::Receiver<QueuedMissingGlyph>,
}

impl MissingGlyphReceiver {
    pub(crate) async fn recv(
        &mut self,
    ) -> std::result::Result<Vec<MissingGlyph>, async_channel::RecvError> {
        loop {
            let queued = self.receiver.recv().await?;
            let mut missing_glyphs = Vec::new();

            for queued in std::iter::once(queued)
                .chain(std::iter::from_fn(|| self.receiver.try_recv().ok()))
                .take(MAX_REPORTED_MISSING_GLYPHS)
            {
                let generation = self.generation.load(Ordering::Acquire);
                if self.state.generation != generation {
                    self.state.reset(generation);
                    missing_glyphs.clear();
                }

                if queued.generation != generation
                    || !self.state.reported.insert(queued.missing_glyph.clone())
                {
                    continue;
                }

                self.state
                    .reported_order
                    .push_back(queued.missing_glyph.clone());
                missing_glyphs.push(queued.missing_glyph);

                if self.state.reported.len() > MAX_REPORTED_MISSING_GLYPHS
                    && let Some(expired) = self.state.reported_order.pop_front()
                {
                    self.state.reported.remove(&expired);
                }
            }

            if !missing_glyphs.is_empty() {
                return Ok(missing_glyphs);
            }

            // A producer can continuously refill the queue with reports the consumer has already
            // seen. Bound work in one poll so the foreground executor cannot be monopolized.
            let mut yielded = false;
            std::future::poll_fn(|cx| {
                if std::mem::replace(&mut yielded, true) {
                    std::task::Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            })
            .await;
        }
    }
}

impl Drop for MissingGlyphReceiver {
    fn drop(&mut self) {
        self.receiver.close();
        while self.receiver.try_recv().is_ok() {}
    }
}

pub(super) fn missing_glyph_channel() -> (Arc<MissingGlyphReporter>, MissingGlyphReceiver) {
    let (sender, receiver) = async_channel::bounded(MAX_REPORTED_MISSING_GLYPHS);
    let generation = Arc::<AtomicUsize>::default();
    (
        Arc::new(MissingGlyphReporter {
            generation: generation.clone(),
            sender,
        }),
        MissingGlyphReceiver {
            state: MissingGlyphState::default(),
            generation,
            receiver,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;

    fn missing(grapheme: &'static str) -> MissingGlyph {
        MissingGlyph::new(grapheme.into(), FallbackFontClass::Proportional)
    }

    #[test]
    fn receiver_deduplicates_reports_without_producer_mutex() {
        let (reporter, mut receiver) = missing_glyph_channel();
        reporter.report(vec![missing("a"), missing("a"), missing("b")]);

        let first = receiver
            .recv()
            .now_or_never()
            .expect("queued report should be immediately ready")
            .expect("reporting channel should stay open");
        assert_eq!(first, vec![missing("a"), missing("b")]);

        reporter.report(vec![missing("a")]);
        assert!(receiver.recv().now_or_never().is_none());

        reporter.reset();
        reporter.report(vec![missing("a")]);
        let after_reset = receiver
            .recv()
            .now_or_never()
            .expect("generation reset should make the glyph reportable again")
            .expect("reporting channel should stay open");
        assert_eq!(after_reset, vec![missing("a")]);
    }

    #[test]
    fn dropping_receiver_closes_nonblocking_producer() {
        let (reporter, receiver) = missing_glyph_channel();
        drop(receiver);
        assert!(reporter.sender.is_closed());
        reporter.report(vec![missing("closed")]);
        assert!(reporter.sender.is_empty());
    }
}
