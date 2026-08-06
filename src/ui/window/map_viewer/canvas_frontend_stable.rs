// The active map path now uses one retained GPUI canvas containing independent tile images.
// There is no macro-page image, no dual representation, and no promotion/demotion state machine.
// canvas_base keeps the snapshot incrementally patched at tile granularity, so a changed chunk
// only replaces the affected tile image while every unchanged image handle remains reusable.
pub(super) use super::canvas_base::*;
