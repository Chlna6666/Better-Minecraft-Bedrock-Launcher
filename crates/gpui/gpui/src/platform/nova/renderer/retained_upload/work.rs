// Ported from longbridge/gpui-fast fast/layers/work.rs.
// Upstream revision: 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
// Copyright 2022-2025 Zed Industries, Inc. Copyright 2026 Longbridge.
// SPDX-License-Identifier: Apache-2.0

//! A bounded estimate of whether rebuilding a layer still saves work.
//!
//! Counting changed frames alone misses infrequent repaints of large
//! overscan regions. Count the rows and drawing operations actually rebuilt
//! in units of the visible content instead, including scroll extensions and
//! input rebuilds. This is a work estimate, not a CPU or GPU timing guarantee.

const WINDOW: usize = 32;
/// How many times what drawing content directly costs painting it into a
/// layer afresh costs: it is laid out, prepainted and painted as it is
/// without the layer, and besides, every tile it reaches is hashed and
/// rasterized again, and its records are built anew. Painting a list's
/// shown rows afresh every frame costs two to two and a half times drawing
/// them without the layer, and a scrolling `div`'s content 1.7 times per
/// viewport painted (gpui_perf's scroll and chat scenarios). Rows added to
/// a layer that keeps the rest cost about what drawing them directly does.
pub(crate) const REPAINT_COST: f32 = 2.;
// Leave room for hashing, carrying records and rasterizing/compositing tiles.
const FRAME_OVERHEAD: f32 = 0.25;
// A changed frame rebuilding more than two viewports creates a latency
// spike even when sparse updates make its average work look cheap. A
// refresh paints content afresh, its work counted at [`REPAINT_COST`].
const MAX_REFRESH_WORK: f32 = 2. * REPAINT_COST;
// A layer whose last this many drawn frames cost more than drawing directly would
// have, their upkeep included, half of them or more each costing more, is
// dropped without waiting for the whole window: a scroll too fast for its
// overscan, or content refreshed on every other frame, does not pay for it,
// and every frame it is kept costs more than drawing without it. One broad
// refresh among cheap frames does not trip it.
const RECENT: usize = 6;
// Two such refreshes this many frames apart or closer make the spikes
// recurring; further apart, the frames composited between them save far
// more than the spikes cost, as a transcript notified now and then does.
const REFRESH_SPIKE_FRAMES: u64 = 120;

pub(crate) struct WorkBudget {
    samples: [f32; WINDOW],
    total: f32,
    next: usize,
    len: usize,
    frame: Option<u64>,
    /// The frames of the last two refreshes that rebuilt more than
    /// [`MAX_REFRESH_WORK`], the latest first.
    expensive_refreshes: [Option<u64>; 2],
}

impl Default for WorkBudget {
    fn default() -> Self {
        Self {
            samples: [0.; WINDOW],
            total: 0.,
            next: 0,
            len: 0,
            frame: None,
            expensive_refreshes: [None; 2],
        }
    }
}

impl WorkBudget {
    pub(crate) fn note_refresh(&mut self, frame: u64, work: f32) {
        if work > MAX_REFRESH_WORK && self.expensive_refreshes[0] != Some(frame) {
            self.expensive_refreshes = [Some(frame), self.expensive_refreshes[0]];
        }
    }

    pub(crate) fn note(&mut self, frame: u64, work: f32) {
        let work = work.clamp(0., WINDOW as f32 * 2.);
        match self.frame {
            // Building the cache once is an investment, not recurring work.
            None => self.frame = Some(frame),
            Some(previous) if previous == frame => {
                if self.len > 0 {
                    let last = (self.next + WINDOW - 1) % WINDOW;
                    self.samples[last] += work;
                    self.total += work;
                }
            }
            Some(_) => {
                self.total -= self.samples[self.next];
                self.samples[self.next] = work + FRAME_OVERHEAD;
                self.total += self.samples[self.next];
                self.next = (self.next + 1) % WINDOW;
                self.len = (self.len + 1).min(WINDOW);
                self.frame = Some(frame);
            }
        }
    }

    pub(crate) fn over_budget(&self) -> bool {
        let spikes = match self.expensive_refreshes {
            [Some(latest), Some(earlier)] => latest - earlier <= REFRESH_SPIKE_FRAMES,
            _ => false,
        };
        spikes || (self.len == WINDOW && self.total >= WINDOW as f32) || self.recent_over()
    }

    /// Whether the last [`RECENT`] frames cost more than drawing directly
    /// would have, half of them or more each costing more.
    fn recent_over(&self) -> bool {
        if self.len < RECENT {
            return false;
        }
        let direct = 1.;
        let recent = (1..=RECENT).map(|back| self.samples[(self.next + WINDOW - back) % WINDOW]);
        let over = recent.clone().filter(|sample| *sample > direct).count();
        over * 2 >= RECENT && recent.sum::<f32>() > direct * RECENT as f32
    }
}

