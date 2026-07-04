use super::{frame::AnimatedFrame, source::AnimatedImageSource};
use crate::assets::decode::resample_bgra_frame_to_target;
use crate::assets::types::ImageDecodeTarget;
use crate::{BackgroundExecutor, Result};
use crossbeam_queue::ArrayQueue;
use image::{
    AnimationDecoder, ImageFormat, Rgba,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use std::{
    io::Cursor,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

pub(in crate::assets) struct StreamingImageState {
    pub(super) source: AnimatedImageSource,
    pub(super) target: Option<ImageDecodeTarget>,
    pub(super) first_frame: AnimatedFrame,
    pub(super) queue: ArrayQueue<AnimatedFrame>,
    pub(super) next_sequence: AtomicUsize,
    pub(super) next_source_index: AtomicUsize,
    pub(super) decode_task_running: AtomicBool,
    pub(super) completed: AtomicBool,
}

impl StreamingImageState {
    pub(in crate::assets) fn ensure_decode_task(self: &Arc<Self>, executor: &BackgroundExecutor) {
        if self.completed.load(Ordering::Acquire) {
            return;
        }
        if self
            .decode_task_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let state = Arc::downgrade(self);
        let executor = executor.clone();
        executor
            .spawn(async move {
                decode_streaming_frames(state);
            })
            .detach();
    }
}

fn decode_streaming_frames(state: Weak<StreamingImageState>) {
    loop {
        let Some(shared_state) = state.upgrade() else {
            break;
        };
        let source = shared_state.source.clone();
        let target = shared_state.target;
        let mut next_sequence = shared_state.next_sequence.load(Ordering::Acquire);
        let skipped_frames = shared_state.next_source_index.load(Ordering::Acquire);
        drop(shared_state);

        match push_streaming_cycle(&source, target, &state, &mut next_sequence, skipped_frames) {
            Ok(StreamCycle::Dropped | StreamCycle::Paused) => break,
            Ok(StreamCycle::Finished) => {
                if let Some(state) = state.upgrade() {
                    state.next_sequence.store(next_sequence, Ordering::Release);
                    state.next_source_index.store(0, Ordering::Release);
                }
            }
            Err(error) => {
                if let Some(state) = state.upgrade() {
                    log::debug!("animated image streaming decode failed: {error}");
                    state.completed.store(true, Ordering::Release);
                }
                break;
            }
        }
    }

    if let Some(state) = state.upgrade() {
        state.decode_task_running.store(false, Ordering::Release);
    }
}

enum StreamCycle {
    Finished,
    Paused,
    Dropped,
}

fn push_streaming_cycle(
    source: &AnimatedImageSource,
    target: Option<ImageDecodeTarget>,
    state: &Weak<StreamingImageState>,
    next_sequence: &mut usize,
    skipped_frames: usize,
) -> Result<StreamCycle> {
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            push_streaming_frames(
                decoder.into_frames(),
                target,
                state,
                next_sequence,
                skipped_frames,
            )
        }
        ImageFormat::Png => {
            let decoder = PngDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            if !decoder.is_apng()? {
                return Ok(StreamCycle::Finished);
            }
            push_streaming_frames(
                decoder.apng()?.into_frames(),
                target,
                state,
                next_sequence,
                skipped_frames,
            )
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            if !decoder.has_animation() {
                return Ok(StreamCycle::Finished);
            }
            let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
            push_streaming_frames(
                decoder.into_frames(),
                target,
                state,
                next_sequence,
                skipped_frames,
            )
        }
        _ => Ok(StreamCycle::Finished),
    }
}

fn push_streaming_frames(
    frames: image::Frames<'_>,
    target: Option<ImageDecodeTarget>,
    state: &Weak<StreamingImageState>,
    next_sequence: &mut usize,
    skipped_frames: usize,
) -> Result<StreamCycle> {
    for (source_index, frame) in frames.enumerate() {
        if source_index < skipped_frames {
            continue;
        }

        let frame = AnimatedFrame::from_rgba_frame(*next_sequence, frame?);
        let frame = if let Some(target) = target {
            resample_bgra_frame_to_target(frame, target)?
        } else {
            frame
        };
        let Some(state) = state.upgrade() else {
            return Ok(StreamCycle::Dropped);
        };
        if state.queue.push(frame).is_err() {
            return Ok(StreamCycle::Paused);
        }
        *next_sequence = next_sequence.saturating_add(1);
        state.next_sequence.store(*next_sequence, Ordering::Release);
        state
            .next_source_index
            .store(source_index.saturating_add(1), Ordering::Release);
    }

    Ok(StreamCycle::Finished)
}
