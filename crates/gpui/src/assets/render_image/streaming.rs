use super::{frame::AnimatedFrame, source::AnimatedImageSource};
use crate::assets::decode::resample_bgra_frame_to_target;
use crate::assets::types::ImageDecodeTarget;
use crate::{BackgroundExecutor, Result};
use image::{
    AnimationDecoder, ImageFormat, Rgba,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use std::{
    io::Cursor,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    thread,
};

pub(in crate::assets) struct StreamingImageState {
    pub(super) source: AnimatedImageSource,
    pub(super) target: Option<ImageDecodeTarget>,
    pub(super) first_frame: AnimatedFrame,
    pub(super) queue_sender: SyncSender<AnimatedFrame>,
    pub(super) queue_receiver: parking_lot::Mutex<std::sync::mpsc::Receiver<AnimatedFrame>>,
    pub(super) next_sequence: usize,
    pub(super) next_source_index: usize,
    pub(in crate::assets) decode_task_running: AtomicBool,
    pub(super) completed: AtomicBool,
}

impl StreamingImageState {
    pub(in crate::assets) fn ensure_decode_task(self: &Arc<Self>, _executor: &BackgroundExecutor) {
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
        if let Err(error) = thread::Builder::new()
            .name("gpui-animation-decode".to_string())
            .spawn(move || decode_streaming_frames(state))
        {
            self.decode_task_running.store(false, Ordering::Release);
            self.completed.store(true, Ordering::Release);
            log::debug!("failed to start animated image decoder: {error}");
        }
    }
}

fn decode_streaming_frames(state: Weak<StreamingImageState>) {
    loop {
        let Some(shared_state) = state.upgrade() else {
            break;
        };
        let source = shared_state.source.clone();
        let target = shared_state.target;
        let sender = shared_state.queue_sender.clone();
        let mut next_sequence = shared_state.next_sequence;
        let mut skipped_frames = shared_state.next_source_index;
        drop(shared_state);

        match push_streaming_cycle(&source, target, &sender, &mut next_sequence, skipped_frames) {
            Ok(StreamCycle::Dropped) => break,
            Ok(StreamCycle::Finished) => {
                skipped_frames = 0;
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
    Dropped,
}

fn push_streaming_cycle(
    source: &AnimatedImageSource,
    target: Option<ImageDecodeTarget>,
    sender: &SyncSender<AnimatedFrame>,
    next_sequence: &mut usize,
    skipped_frames: usize,
) -> Result<StreamCycle> {
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            push_streaming_frames(
                decoder.into_frames(),
                target,
                sender,
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
                sender,
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
                sender,
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
    sender: &SyncSender<AnimatedFrame>,
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
        if sender.send(frame).is_err() {
            return Ok(StreamCycle::Dropped);
        }
        *next_sequence = next_sequence.saturating_add(1);
    }

    Ok(StreamCycle::Finished)
}
