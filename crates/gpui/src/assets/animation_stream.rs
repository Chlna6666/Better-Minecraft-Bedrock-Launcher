use super::resample::resample_bgra_frame;
use super::{AnimatedFrame, EncodedImage, ImageRenderSize};
use crate::Result;
use image::{
    AnimationDecoder, Frames, ImageFormat, Rgba,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use std::{
    io::Cursor,
    sync::{
        Arc, OnceLock, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
    time::Instant,
};

const UNASSIGNED_WORKER: usize = usize::MAX;
const DEFAULT_GLOBAL_PREFETCH_BYTE_LIMIT: usize = 96 * 1024 * 1024;
static ANIMATION_WORKERS: OnceLock<AnimationWorkers> = OnceLock::new();

struct AnimationQueueBudget {
    queued_bytes: AtomicUsize,
    byte_limit: AtomicUsize,
    // Releases only broadcast when at least one worker observed global backpressure.
    capacity_waiting: AtomicBool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AnimationQueueSnapshot {
    pub(crate) queued_bytes: usize,
    pub(crate) byte_limit: usize,
}

fn animation_queue_budget() -> &'static AnimationQueueBudget {
    static BUDGET: OnceLock<AnimationQueueBudget> = OnceLock::new();
    BUDGET.get_or_init(|| AnimationQueueBudget {
        queued_bytes: AtomicUsize::new(0),
        byte_limit: AtomicUsize::new(DEFAULT_GLOBAL_PREFETCH_BYTE_LIMIT),
        capacity_waiting: AtomicBool::new(false),
    })
}

pub(crate) fn animation_queue_snapshot() -> AnimationQueueSnapshot {
    let budget = animation_queue_budget();
    AnimationQueueSnapshot {
        queued_bytes: budget.queued_bytes.load(Ordering::Acquire),
        byte_limit: budget.byte_limit.load(Ordering::Acquire),
    }
}

pub(crate) fn configure_animation_queue(byte_limit: usize) {
    animation_queue_budget()
        .byte_limit
        .store(byte_limit.max(4), Ordering::Release);
}

pub(in crate::assets) fn reserve_animation_queue_bytes(byte_len: usize) -> bool {
    let budget = animation_queue_budget();
    loop {
        let queued = budget.queued_bytes.load(Ordering::Acquire);
        let limit = budget.byte_limit.load(Ordering::Acquire);
        if queued != 0 && queued.saturating_add(byte_len) > limit {
            budget.capacity_waiting.store(true, Ordering::Release);
            // Close the race with a release between the failed capacity check and waiter marking.
            if budget.queued_bytes.load(Ordering::Acquire) == queued {
                return false;
            }
            continue;
        }
        if budget
            .queued_bytes
            .compare_exchange_weak(
                queued,
                queued.saturating_add(byte_len),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return true;
        }
    }
}

fn animation_queue_has_capacity(byte_len: usize) -> bool {
    let budget = animation_queue_budget();
    loop {
        let queued = budget.queued_bytes.load(Ordering::Acquire);
        let limit = budget.byte_limit.load(Ordering::Acquire);
        if queued == 0 || queued.saturating_add(byte_len) <= limit {
            return true;
        }
        budget.capacity_waiting.store(true, Ordering::Release);
        // A concurrent release may already have made this frame admissible.
        if budget.queued_bytes.load(Ordering::Acquire) == queued {
            return false;
        }
    }
}

pub(in crate::assets) fn release_animation_queue_bytes(byte_len: usize) {
    let budget = animation_queue_budget();
    budget.queued_bytes.fetch_sub(byte_len, Ordering::AcqRel);
    if !budget.capacity_waiting.swap(false, Ordering::AcqRel) {
        return;
    }
    if let Some(workers) = ANIMATION_WORKERS.get() {
        crate::diagnostics::performance_metrics::record_animation_worker_pool_wake();
        workers.wake_all();
    }
}

pub(in crate::assets) struct AnimationStream {
    pub(super) source: EncodedImage,
    pub(super) target: Option<ImageRenderSize>,
    pub(super) first_frame: AnimatedFrame,
    pub(super) queue_sender: SyncSender<AnimatedFrame>,
    pub(super) queue_receiver: parking_lot::Mutex<std::sync::mpsc::Receiver<AnimatedFrame>>,
    pub(super) next_sequence: usize,
    pub(super) next_source_index: usize,
    pub(super) prefetch_frames: usize,
    pub(super) prefetch_byte_limit: usize,
    pub(super) queued_frame_count: AtomicUsize,
    pub(super) queued_byte_len: AtomicUsize,
    pub(super) delivered_byte_len: AtomicUsize,
    pub(in crate::assets) stream_task_running: AtomicBool,
    pub(super) completed: AtomicBool,
    pub(super) worker_index: AtomicUsize,
}

impl AnimationStream {
    pub(in crate::assets) fn ensure_stream_task(self: &Arc<Self>) {
        if self.completed.load(Ordering::Acquire) {
            return;
        }

        if self
            .stream_task_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let Some(worker_index) = animation_workers().register(Arc::downgrade(self)) else {
                self.stream_task_running.store(false, Ordering::Release);
                self.completed.store(true, Ordering::Release);
                log::debug!("failed to start the GPUI animation worker pool");
                return;
            };
            self.worker_index.store(worker_index, Ordering::Release);
        } else {
            let worker_index = self.worker_index.load(Ordering::Acquire);
            if worker_index != UNASSIGNED_WORKER {
                animation_workers().wake(worker_index);
            }
        }
    }

    pub(super) fn can_queue(&self, byte_len: usize) -> bool {
        if self.queued_frame_count.load(Ordering::Acquire) >= self.prefetch_frames {
            return false;
        }
        let queued = self.queued_byte_len.load(Ordering::Acquire);
        (queued == 0 || queued.saturating_add(byte_len) <= self.prefetch_byte_limit)
            && animation_queue_has_capacity(byte_len)
    }

    fn reserve_queue_bytes(&self, byte_len: usize) -> bool {
        loop {
            let queued = self.queued_byte_len.load(Ordering::Acquire);
            if queued != 0 && queued.saturating_add(byte_len) > self.prefetch_byte_limit {
                return false;
            }
            if self
                .queued_byte_len
                .compare_exchange_weak(
                    queued,
                    queued.saturating_add(byte_len),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                if reserve_animation_queue_bytes(byte_len) {
                    return true;
                }
                self.queued_byte_len.fetch_sub(byte_len, Ordering::AcqRel);
                return false;
            }
        }
    }

    pub(super) fn release_queued_frame(&self, byte_len: usize) {
        self.queued_frame_count.fetch_sub(1, Ordering::AcqRel);
        self.queued_byte_len.fetch_sub(byte_len, Ordering::AcqRel);
        release_animation_queue_bytes(byte_len);
    }
}

impl Drop for AnimationStream {
    fn drop(&mut self) {
        let queued_byte_len = self.queued_byte_len.swap(0, Ordering::AcqRel);
        if queued_byte_len != 0 {
            release_animation_queue_bytes(queued_byte_len);
        }
    }
}

struct AnimationWorkers {
    workers: Vec<AnimationWorker>,
    next_worker: AtomicUsize,
}

struct AnimationWorker {
    sender: mpsc::Sender<Weak<AnimationStream>>,
    thread: thread::Thread,
}

impl AnimationWorkers {
    fn new() -> Self {
        let available = thread::available_parallelism().map_or(2, usize::from);
        let worker_count = available.div_ceil(2).clamp(1, 4);
        let mut workers = Vec::with_capacity(worker_count);

        for worker_index in 0..worker_count {
            let (sender, receiver) = mpsc::channel();
            let builder = thread::Builder::new().name(format!("gpui-animation-{worker_index}"));
            match builder.spawn(move || animation_worker(receiver)) {
                Ok(worker) => workers.push(AnimationWorker {
                    sender,
                    thread: worker.thread().clone(),
                }),
                Err(error) => log::debug!("failed to start GPUI animation worker: {error}"),
            }
        }

        Self {
            workers,
            next_worker: AtomicUsize::new(0),
        }
    }

    fn register(&self, state: Weak<AnimationStream>) -> Option<usize> {
        if self.workers.is_empty() {
            return None;
        }
        let worker_index = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let worker = &self.workers[worker_index];
        worker.sender.send(state).ok()?;
        worker.thread.unpark();
        Some(worker_index)
    }

    fn wake(&self, worker_index: usize) {
        if let Some(worker) = self.workers.get(worker_index) {
            worker.thread.unpark();
        }
    }

    fn wake_all(&self) {
        for worker in &self.workers {
            worker.thread.unpark();
        }
    }
}

fn animation_workers() -> &'static AnimationWorkers {
    ANIMATION_WORKERS.get_or_init(AnimationWorkers::new)
}

struct AnimationWork {
    state: Weak<AnimationStream>,
    frames: Frames<'static>,
    source_index: usize,
    skip_before: usize,
    next_sequence: usize,
    pending_frame: Option<AnimatedFrame>,
}

enum WorkProgress {
    Advanced,
    Backpressured,
    Remove,
}

impl AnimationWork {
    fn new(state: Weak<AnimationStream>) -> Result<Option<Self>> {
        let Some(shared_state) = state.upgrade() else {
            return Ok(None);
        };
        let frames = match animation_frames(&shared_state.source) {
            Ok(Some(frames)) => frames,
            Ok(None) => {
                shared_state.completed.store(true, Ordering::Release);
                shared_state
                    .stream_task_running
                    .store(false, Ordering::Release);
                return Ok(None);
            }
            Err(error) => {
                shared_state.completed.store(true, Ordering::Release);
                shared_state
                    .stream_task_running
                    .store(false, Ordering::Release);
                return Err(error);
            }
        };
        let work = Self {
            frames,
            source_index: 0,
            skip_before: shared_state.next_source_index,
            next_sequence: shared_state.next_sequence,
            pending_frame: None,
            state,
        };
        Ok(Some(work))
    }

    fn advance(&mut self) -> WorkProgress {
        let Some(state) = self.state.upgrade() else {
            return WorkProgress::Remove;
        };
        let expected_byte_len = self
            .pending_frame
            .as_ref()
            .map_or_else(|| state.first_frame.byte_len(), AnimatedFrame::byte_len);
        if !state.can_queue(expected_byte_len) {
            crate::diagnostics::performance_metrics::record_animation_queue_backpressure();
            return WorkProgress::Backpressured;
        }

        let frame = if let Some(frame) = self.pending_frame.take() {
            frame
        } else {
            loop {
                let Some(frame) = self.frames.next() else {
                    let started_at = Instant::now();
                    let restarted_frames = animation_frames(&state.source);
                    crate::diagnostics::performance_metrics::record_animation_loop_restart(
                        started_at.elapsed(),
                        matches!(&restarted_frames, Ok(Some(_))),
                    );
                    match restarted_frames {
                        Ok(Some(frames)) => {
                            self.frames = frames;
                            self.source_index = 0;
                            self.skip_before = 0;
                            return WorkProgress::Advanced;
                        }
                        Ok(None) => state.completed.store(true, Ordering::Release),
                        Err(error) => {
                            log::debug!("animated image restart failed: {error}");
                            state.completed.store(true, Ordering::Release);
                        }
                    }
                    state.stream_task_running.store(false, Ordering::Release);
                    return WorkProgress::Remove;
                };

                let source_index = self.source_index;
                self.source_index = self.source_index.saturating_add(1);
                if source_index < self.skip_before {
                    continue;
                }

                let frame = match frame {
                    Ok(frame) => AnimatedFrame::from_rgba_frame(self.next_sequence, frame),
                    Err(error) => {
                        log::debug!("animated image frame failed: {error}");
                        state.completed.store(true, Ordering::Release);
                        state.stream_task_running.store(false, Ordering::Release);
                        return WorkProgress::Remove;
                    }
                };
                let frame = if let Some(target) = state.target {
                    match resample_bgra_frame(frame, target) {
                        Ok(frame) => frame,
                        Err(error) => {
                            log::debug!("animated image resize failed: {error}");
                            state.completed.store(true, Ordering::Release);
                            state.stream_task_running.store(false, Ordering::Release);
                            return WorkProgress::Remove;
                        }
                    }
                } else {
                    frame
                };
                break frame;
            }
        };
        let frame_byte_len = frame.byte_len();
        if !state.reserve_queue_bytes(frame_byte_len) {
            crate::diagnostics::performance_metrics::record_animation_queue_backpressure();
            self.pending_frame = Some(frame);
            return WorkProgress::Backpressured;
        }
        state.queued_frame_count.fetch_add(1, Ordering::AcqRel);

        match state.queue_sender.try_send(frame) {
            Ok(()) => {
                self.next_sequence = self.next_sequence.saturating_add(1);
                WorkProgress::Advanced
            }
            Err(TrySendError::Full(frame)) => {
                crate::diagnostics::performance_metrics::record_animation_queue_backpressure();
                state.queued_frame_count.fetch_sub(1, Ordering::AcqRel);
                state
                    .queued_byte_len
                    .fetch_sub(frame_byte_len, Ordering::AcqRel);
                release_animation_queue_bytes(frame_byte_len);
                self.pending_frame = Some(frame);
                WorkProgress::Backpressured
            }
            Err(TrySendError::Disconnected(_)) => {
                state.queued_frame_count.fetch_sub(1, Ordering::AcqRel);
                state
                    .queued_byte_len
                    .fetch_sub(frame_byte_len, Ordering::AcqRel);
                release_animation_queue_bytes(frame_byte_len);
                state.completed.store(true, Ordering::Release);
                state.stream_task_running.store(false, Ordering::Release);
                WorkProgress::Remove
            }
        }
    }
}

fn animation_worker(receiver: mpsc::Receiver<Weak<AnimationStream>>) {
    let mut work = Vec::<AnimationWork>::new();
    loop {
        if work.is_empty() {
            let Ok(state) = receiver.recv() else {
                return;
            };
            match AnimationWork::new(state) {
                Ok(Some(animation)) => work.push(animation),
                Ok(None) => {}
                Err(error) => log::debug!("animated image stream setup failed: {error}"),
            }
        }
        while let Ok(state) = receiver.try_recv() {
            match AnimationWork::new(state) {
                Ok(Some(animation)) => work.push(animation),
                Ok(None) => {}
                Err(error) => log::debug!("animated image stream setup failed: {error}"),
            }
        }

        let mut advanced = false;
        work.retain_mut(|animation| match animation.advance() {
            WorkProgress::Advanced => {
                advanced = true;
                true
            }
            WorkProgress::Backpressured => true,
            WorkProgress::Remove => false,
        });

        if !advanced {
            thread::park();
        }
    }
}

fn animation_frames(source: &EncodedImage) -> Result<Option<Frames<'static>>> {
    let bytes = Arc::clone(&source.bytes);
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(bytes))?;
            Ok(Some(decoder.into_frames()))
        }
        ImageFormat::Png => {
            let decoder = PngDecoder::new(Cursor::new(bytes))?;
            if decoder.is_apng()? {
                Ok(Some(decoder.apng()?.into_frames()))
            } else {
                Ok(None)
            }
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes))?;
            if decoder.has_animation() {
                let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
                Ok(Some(decoder.into_frames()))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}
