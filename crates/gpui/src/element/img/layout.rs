use crate::{AnimatedFrame, AnyElement};

/// Image data produced during layout for the following paint pass.
pub struct ImageLayout {
    pub(super) frame: Option<AnimatedFrame>,
    pub(super) replacement: Option<AnyElement>,
}
