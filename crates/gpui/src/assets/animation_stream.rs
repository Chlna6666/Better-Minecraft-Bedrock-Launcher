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
static ANIMATION_WORKERS: OnceLock<AnimationWorkers> = OnceLock::new();
static ANIMATION_QUEUED_BYTES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AnimationQueueSnapshot {
    pub(crate) queued_bytes: usize,
}

pub(crate) fn animation_queue_snapshot() -> AnimationQueueSnapshot {
    AnimationQueueSnapshot {
        queued_bytes: ANIMATION_QUEUED_BYTES.load(Ordering::Acquire),
    }
}

fn atomic_saturating_sub(value: &AtomicUsize, amount: usize) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_sub(amount))
    });
}

pub(in crate::assets) fn record_animation_queue_bytes(byte_len: usize) {
    ANIMATION_QUEUED_BYTES.fetch_add(byte_len, Ordering::AcqRel);
}

pub(in crate::assets) fn release_animation_queue_bytes(byte_len: usize) {
    atomic_saturating_sub(&ANIMATION_QUEUED_BYTES, byte_len);
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

    pub(super) fn can_queue(&self) -> bool {
        self.queued_frame_count.load(Ordering::Acquire) < self.prefetch_frames
    }

    fn reserve_queued_frame(&self, byte_len: usize) -> bool {
        if self
            .queued_frame_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued < self.prefetch_frames).then_some(queued.saturating_add(1))
            })
            .is_err()
        {
            return false;
        }
        self.queued_byte_len.fetch_add(byte_len, Ordering::AcqRel);
        record_animation_queue_bytes(byte_len);
        true
    }

    pub(super) fn release_queued_frame(&self, byte_len: usize) {
        atomic_saturating_sub(&self.queued_frame_count, 1);
        atomic_saturating_sub(&self.queued_byte_len, byte_len);
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
        Ok(Some(Self {
            frames,
            source_index: 0,
            skip_before: shared_state.next_source_index,
            next_sequence: shared_state.next_sequence,
            pending_frame: None,
            state,
        }))
    }

    fn advance(&mut self) -> WorkProgress {
        let Some(state) = self.state.upgrade() else {
            return WorkProgress::Remove;
        };
        if !state.can_queue() {
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
        if !state.reserve_queued_frame(frame_byte_len) {
            crate::diagnostics::performance_metrics::record_animation_queue_backpressure();
            self.pending_frame = Some(frame);
            return WorkProgress::Backpressured;
        }

        match state.queue_sender.try_send(frame) {
            Ok(()) => {
                self.next_sequence = self.next_sequence.saturating_add(1);
                WorkProgress::Advanced
            }
            Err(TrySendError::Full(frame)) => {
                crate::diagnostics::performance_metrics::record_animation_queue_backpressure();
                state.release_queued_frame(frame_byte_len);
                self.pending_frame = Some(frame);
                WorkProgress::Backpressured
            }
            Err(TrySendError::Disconnected(_)) => {
                state.release_queued_frame(frame_byte_len);
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
