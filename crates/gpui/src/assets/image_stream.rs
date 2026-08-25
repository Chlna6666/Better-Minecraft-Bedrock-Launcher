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
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
    pub(super) queued_byte_len: Arc<AtomicUsize>,
    pub(super) delivered_byte_len: AtomicUsize,
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
    let Some(shared_state) = state.upgrade() else {
        return;
    };
    let source = shared_state.source.clone();
    let target = shared_state.target;
    let sender = shared_state.queue_sender.clone();
    let queued_byte_len = shared_state.queued_byte_len.clone();
    let mut next_sequence = shared_state.next_sequence;
    let mut skipped_frames = shared_state.next_source_index;
    drop(shared_state);

    loop {
        match push_streaming_cycle(
            &source,
            target,
            &sender,
            &queued_byte_len,
            &mut next_sequence,
            skipped_frames,
        ) {
            Ok(StreamCycle::Dropped) => break,
            Ok(StreamCycle::Finished { frames_pushed }) if frames_pushed > 0 => {
                skipped_frames = 0;
            }
            Ok(StreamCycle::Finished { .. }) => {
                if let Some(state) = state.upgrade() {
                    state.completed.store(true, Ordering::Release);
                }
                break;
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
    Finished { frames_pushed: usize },
    Dropped,
}

fn push_streaming_cycle(
    source: &AnimatedImageSource,
    target: Option<ImageDecodeTarget>,
    sender: &SyncSender<AnimatedFrame>,
    queued_byte_len: &AtomicUsize,
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
                queued_byte_len,
                next_sequence,
                skipped_frames,
            )
        }
        ImageFormat::Png => {
            let decoder = PngDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            if !decoder.is_apng()? {
                return Ok(StreamCycle::Finished { frames_pushed: 0 });
            }
            push_streaming_frames(
                decoder.apng()?.into_frames(),
                target,
                sender,
                queued_byte_len,
                next_sequence,
                skipped_frames,
            )
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            if !decoder.has_animation() {
                return Ok(StreamCycle::Finished { frames_pushed: 0 });
            }
            let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
            push_streaming_frames(
                decoder.into_frames(),
                target,
                sender,
                queued_byte_len,
                next_sequence,
                skipped_frames,
            )
        }
        _ => Ok(StreamCycle::Finished { frames_pushed: 0 }),
    }
}

fn push_streaming_frames(
    frames: image::Frames<'_>,
    target: Option<ImageDecodeTarget>,
    sender: &SyncSender<AnimatedFrame>,
    queued_byte_len: &AtomicUsize,
    next_sequence: &mut usize,
    skipped_frames: usize,
) -> Result<StreamCycle> {
    let mut frames_pushed = 0usize;
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
        let frame_byte_len = frame.byte_len();
        queued_byte_len.fetch_add(frame_byte_len, Ordering::Relaxed);
        if sender.send(frame).is_err() {
            queued_byte_len.fetch_sub(frame_byte_len, Ordering::Relaxed);
            return Ok(StreamCycle::Dropped);
        }
        *next_sequence = next_sequence.saturating_add(1);
        frames_pushed = frames_pushed.saturating_add(1);
    }

    Ok(StreamCycle::Finished { frames_pushed })
}
