use super::frame::AnimatedFrame;
use smallvec::SmallVec;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct AnimatedImageSource {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) format: image::ImageFormat,
}

pub(crate) struct DecodedAnimation {
    pub(crate) first_frame: AnimatedFrame,
    pub(crate) remaining_frames: SmallVec<[AnimatedFrame; 8]>,
    pub(crate) is_complete: bool,
}
