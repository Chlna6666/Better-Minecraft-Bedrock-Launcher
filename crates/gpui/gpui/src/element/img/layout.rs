use crate::{AnimatedFrame, AnyElement, RenderImage};
use std::sync::Arc;

/// Image data produced during layout for the following paint pass.
pub struct ImageLayout {
    pub(super) image: Option<Arc<RenderImage>>,
    pub(super) frame: Option<AnimatedFrame>,
    pub(super) replacement: Option<AnyElement>,
}
