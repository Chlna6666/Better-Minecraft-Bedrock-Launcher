use bedrock_render::{ChunkPos, Dimension};
use bedrock_world::SlimeChunkBounds;
use gpui::{Pixels, Point, px};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ChunkSelection {
    pub(super) start: ChunkPos,
    pub(super) end: ChunkPos,
}

impl ChunkSelection {
    pub(super) fn bounds(self) -> SlimeChunkBounds {
        SlimeChunkBounds {
            dimension: self.start.dimension,
            min_chunk_x: self.start.x.min(self.end.x),
            max_chunk_x: self.start.x.max(self.end.x),
            min_chunk_z: self.start.z.min(self.end.z),
            max_chunk_z: self.start.z.max(self.end.z),
        }
    }

    pub(super) fn chunk_count(self) -> usize {
        let bounds = self.bounds();
        let width = i64::from(bounds.max_chunk_x) - i64::from(bounds.min_chunk_x) + 1;
        let depth = i64::from(bounds.max_chunk_z) - i64::from(bounds.min_chunk_z) + 1;
        usize::try_from(width.saturating_mul(depth)).unwrap_or(usize::MAX)
    }

    pub(super) fn chunks(self) -> Vec<ChunkPos> {
        let bounds = self.bounds();
        let capacity = self.chunk_count();
        let mut chunks = Vec::with_capacity(capacity.min(4096));
        for z in bounds.min_chunk_z..=bounds.max_chunk_z {
            for x in bounds.min_chunk_x..=bounds.max_chunk_x {
                chunks.push(ChunkPos {
                    x,
                    z,
                    dimension: bounds.dimension,
                });
            }
        }
        chunks
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RightSelectionDrag {
    pub(super) start_position: Point<Pixels>,
    pub(super) start_chunk: ChunkPos,
    pub(super) current_chunk: ChunkPos,
    pub(super) moved: bool,
    pub(super) button: SelectionPointerButton,
    pub(super) intent: RightSelectionIntent,
}

impl RightSelectionDrag {
    pub(super) fn new(start_position: Point<Pixels>, start_chunk: ChunkPos) -> Self {
        Self::with_intent(
            start_position,
            start_chunk,
            SelectionPointerButton::Right,
            RightSelectionIntent::NewSelection,
        )
    }

    pub(super) fn existing_for_button(
        start_position: Point<Pixels>,
        start_chunk: ChunkPos,
        selection: ChunkSelection,
        target: ExistingSelectionTarget,
        button: SelectionPointerButton,
    ) -> Self {
        let intent = match target {
            ExistingSelectionTarget::Inside if button == SelectionPointerButton::Left => {
                RightSelectionIntent::Move(selection)
            }
            ExistingSelectionTarget::Inside => RightSelectionIntent::OpenMenu(selection),
            ExistingSelectionTarget::Outside => RightSelectionIntent::Cancel(selection),
            ExistingSelectionTarget::Resize(handle) => {
                RightSelectionIntent::Resize { selection, handle }
            }
        };
        Self::with_intent(start_position, start_chunk, button, intent)
    }

    fn with_intent(
        start_position: Point<Pixels>,
        start_chunk: ChunkPos,
        button: SelectionPointerButton,
        intent: RightSelectionIntent,
    ) -> Self {
        Self {
            start_position,
            start_chunk,
            current_chunk: start_chunk,
            moved: false,
            button,
            intent,
        }
    }

    pub(super) fn selection(self) -> ChunkSelection {
        match self.intent {
            RightSelectionIntent::NewSelection => ChunkSelection {
                start: self.start_chunk,
                end: self.current_chunk,
            },
            RightSelectionIntent::Resize { selection, handle } => {
                resize_chunk_selection(selection, handle, self.current_chunk)
            }
            RightSelectionIntent::Move(selection) => translate_chunk_selection(
                selection,
                self.current_chunk.x.saturating_sub(self.start_chunk.x),
                self.current_chunk.z.saturating_sub(self.start_chunk.z),
            ),
            RightSelectionIntent::OpenMenu(selection) | RightSelectionIntent::Cancel(selection) => {
                selection
            }
        }
    }

    pub(super) const fn changes_selection(self) -> bool {
        matches!(
            self.intent,
            RightSelectionIntent::NewSelection
                | RightSelectionIntent::Move(_)
                | RightSelectionIntent::Resize { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectionPointerButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RightSelectionIntent {
    NewSelection,
    Move(ChunkSelection),
    OpenMenu(ChunkSelection),
    Cancel(ChunkSelection),
    Resize {
        selection: ChunkSelection,
        handle: SelectionResizeHandle,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RightSelectionReleaseAction {
    ApplySelection,
    ApplySelectionAndOpenMenu,
    CancelSelection,
    KeepSelection,
    OpenMenu,
}

pub(super) const fn right_selection_release_action(
    button: SelectionPointerButton,
    intent: RightSelectionIntent,
    moved: bool,
) -> RightSelectionReleaseAction {
    match (button, intent, moved) {
        (_, RightSelectionIntent::Cancel(_), _) => RightSelectionReleaseAction::CancelSelection,
        (_, RightSelectionIntent::Move(_), false)
        | (SelectionPointerButton::Left, RightSelectionIntent::Resize { .. }, false) => {
            RightSelectionReleaseAction::KeepSelection
        }
        (_, RightSelectionIntent::Move(_), true)
        | (_, RightSelectionIntent::Resize { .. }, true) => {
            RightSelectionReleaseAction::ApplySelection
        }
        (SelectionPointerButton::Right, RightSelectionIntent::OpenMenu(_), false)
        | (SelectionPointerButton::Right, RightSelectionIntent::Resize { .. }, false) => {
            RightSelectionReleaseAction::OpenMenu
        }
        (_, RightSelectionIntent::OpenMenu(_), true) => RightSelectionReleaseAction::KeepSelection,
        (_, RightSelectionIntent::NewSelection, _) => {
            RightSelectionReleaseAction::ApplySelectionAndOpenMenu
        }
        (SelectionPointerButton::Left, RightSelectionIntent::OpenMenu(_), false) => {
            RightSelectionReleaseAction::KeepSelection
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExistingSelectionTarget {
    Inside,
    Outside,
    Resize(SelectionResizeHandle),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectionResizeHandle {
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SelectionScreenBounds {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) right: f32,
    pub(super) bottom: f32,
}

pub(super) fn existing_selection_target(
    position: Point<Pixels>,
    bounds: SelectionScreenBounds,
    tolerance: f32,
) -> ExistingSelectionTarget {
    let x = position.x / px(1.0);
    let y = position.y / px(1.0);
    let left = bounds.left.min(bounds.right);
    let right = bounds.left.max(bounds.right);
    let top = bounds.top.min(bounds.bottom);
    let bottom = bounds.top.max(bounds.bottom);
    let horizontal_tolerance = tolerance.min((right - left) * 0.25);
    let vertical_tolerance = tolerance.min((bottom - top) * 0.25);
    let within_horizontal_span =
        (left - horizontal_tolerance..=right + horizontal_tolerance).contains(&x);
    let within_vertical_span =
        (top - vertical_tolerance..=bottom + vertical_tolerance).contains(&y);
    let horizontal = within_horizontal_span
        .then(|| closest_resize_edge(y, top, bottom, vertical_tolerance, false))
        .flatten();
    let vertical = within_vertical_span
        .then(|| closest_resize_edge(x, left, right, horizontal_tolerance, true))
        .flatten();

    let handle = match (vertical, horizontal) {
        (Some(SelectionResizeHandle::West), Some(SelectionResizeHandle::North)) => {
            Some(SelectionResizeHandle::NorthWest)
        }
        (Some(SelectionResizeHandle::East), Some(SelectionResizeHandle::North)) => {
            Some(SelectionResizeHandle::NorthEast)
        }
        (Some(SelectionResizeHandle::West), Some(SelectionResizeHandle::South)) => {
            Some(SelectionResizeHandle::SouthWest)
        }
        (Some(SelectionResizeHandle::East), Some(SelectionResizeHandle::South)) => {
            Some(SelectionResizeHandle::SouthEast)
        }
        (Some(handle), None) | (None, Some(handle)) => Some(handle),
        _ => None,
    };
    if let Some(handle) = handle {
        ExistingSelectionTarget::Resize(handle)
    } else if (left..=right).contains(&x) && (top..=bottom).contains(&y) {
        ExistingSelectionTarget::Inside
    } else {
        ExistingSelectionTarget::Outside
    }
}

fn closest_resize_edge(
    value: f32,
    minimum: f32,
    maximum: f32,
    tolerance: f32,
    horizontal_axis: bool,
) -> Option<SelectionResizeHandle> {
    let minimum_distance = (value - minimum).abs();
    let maximum_distance = (value - maximum).abs();
    let minimum_handle = if horizontal_axis {
        SelectionResizeHandle::West
    } else {
        SelectionResizeHandle::North
    };
    let maximum_handle = if horizontal_axis {
        SelectionResizeHandle::East
    } else {
        SelectionResizeHandle::South
    };
    match (minimum_distance <= tolerance, maximum_distance <= tolerance) {
        (true, true) if minimum_distance <= maximum_distance => Some(minimum_handle),
        (true, true) => Some(maximum_handle),
        (true, false) => Some(minimum_handle),
        (false, true) => Some(maximum_handle),
        (false, false) => None,
    }
}

pub(super) fn resize_chunk_selection(
    selection: ChunkSelection,
    handle: SelectionResizeHandle,
    current: ChunkPos,
) -> ChunkSelection {
    let bounds = selection.bounds();
    let moves_west = matches!(
        handle,
        SelectionResizeHandle::West
            | SelectionResizeHandle::NorthWest
            | SelectionResizeHandle::SouthWest
    );
    let moves_east = matches!(
        handle,
        SelectionResizeHandle::East
            | SelectionResizeHandle::NorthEast
            | SelectionResizeHandle::SouthEast
    );
    let moves_north = matches!(
        handle,
        SelectionResizeHandle::North
            | SelectionResizeHandle::NorthEast
            | SelectionResizeHandle::NorthWest
    );
    let moves_south = matches!(
        handle,
        SelectionResizeHandle::South
            | SelectionResizeHandle::SouthEast
            | SelectionResizeHandle::SouthWest
    );
    ChunkSelection {
        start: ChunkPos {
            x: if moves_west {
                current.x
            } else {
                bounds.min_chunk_x
            },
            z: if moves_north {
                current.z
            } else {
                bounds.min_chunk_z
            },
            dimension: bounds.dimension,
        },
        end: ChunkPos {
            x: if moves_east {
                current.x
            } else {
                bounds.max_chunk_x
            },
            z: if moves_south {
                current.z
            } else {
                bounds.max_chunk_z
            },
            dimension: bounds.dimension,
        },
    }
}

pub(super) fn translate_chunk_selection(
    selection: ChunkSelection,
    delta_x: i32,
    delta_z: i32,
) -> ChunkSelection {
    let bounds = selection.bounds();
    ChunkSelection {
        start: ChunkPos {
            x: bounds.min_chunk_x.saturating_add(delta_x),
            z: bounds.min_chunk_z.saturating_add(delta_z),
            dimension: bounds.dimension,
        },
        end: ChunkPos {
            x: bounds.max_chunk_x.saturating_add(delta_x),
            z: bounds.max_chunk_z.saturating_add(delta_z),
            dimension: bounds.dimension,
        },
    }
}

pub(super) fn chunk_from_block(block_x: i32, block_z: i32, dimension: Dimension) -> ChunkPos {
    ChunkPos {
        x: block_x.div_euclid(16),
        z: block_z.div_euclid(16),
        dimension,
    }
}

pub(super) fn right_selection_moved(
    start: Point<Pixels>,
    current: Point<Pixels>,
    threshold: f32,
) -> bool {
    let delta_x = (current.x - start.x) / px(1.0);
    let delta_y = (current.y - start.y) / px(1.0);
    delta_x.hypot(delta_y) > threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn chunk_selection_bounds_normalize_negative_coordinates() {
        let selection = ChunkSelection {
            start: ChunkPos {
                x: 7,
                z: -12,
                dimension: Dimension::Overworld,
            },
            end: ChunkPos {
                x: -4,
                z: 3,
                dimension: Dimension::Overworld,
            },
        };
        let bounds = selection.bounds();

        assert_eq!(bounds.min_chunk_x, -4);
        assert_eq!(bounds.max_chunk_x, 7);
        assert_eq!(bounds.min_chunk_z, -12);
        assert_eq!(bounds.max_chunk_z, 3);
        assert_eq!(bounds.dimension, Dimension::Overworld);
    }

    #[::core::prelude::v1::test]
    fn right_selection_drag_builds_normalized_selection() {
        let mut drag = RightSelectionDrag::new(
            gpui::point(px(10.0), px(10.0)),
            ChunkPos {
                x: 4,
                z: 8,
                dimension: Dimension::Overworld,
            },
        );
        drag.current_chunk = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Overworld,
        };
        let bounds = drag.selection().bounds();

        assert_eq!(bounds.min_chunk_x, 1);
        assert_eq!(bounds.max_chunk_x, 4);
        assert_eq!(bounds.min_chunk_z, 2);
        assert_eq!(bounds.max_chunk_z, 8);
    }

    #[::core::prelude::v1::test]
    fn left_drag_inside_translates_existing_selection() {
        let selection = ChunkSelection {
            start: ChunkPos {
                x: -2,
                z: 4,
                dimension: Dimension::Overworld,
            },
            end: ChunkPos {
                x: 1,
                z: 8,
                dimension: Dimension::Overworld,
            },
        };
        let mut drag = RightSelectionDrag::existing_for_button(
            gpui::point(px(10.0), px(10.0)),
            ChunkPos {
                x: 0,
                z: 6,
                dimension: Dimension::Overworld,
            },
            selection,
            ExistingSelectionTarget::Inside,
            SelectionPointerButton::Left,
        );
        drag.current_chunk.x = 3;
        drag.current_chunk.z = 4;

        let bounds = drag.selection().bounds();
        assert_eq!(bounds.min_chunk_x, 1);
        assert_eq!(bounds.max_chunk_x, 4);
        assert_eq!(bounds.min_chunk_z, 2);
        assert_eq!(bounds.max_chunk_z, 6);
    }

    #[::core::prelude::v1::test]
    fn resize_selection_moves_only_the_selected_edges() {
        let selection = ChunkSelection {
            start: ChunkPos {
                x: 10,
                z: 20,
                dimension: Dimension::Overworld,
            },
            end: ChunkPos {
                x: 14,
                z: 26,
                dimension: Dimension::Overworld,
            },
        };
        let east = resize_chunk_selection(
            selection,
            SelectionResizeHandle::East,
            ChunkPos {
                x: 18,
                z: 23,
                dimension: Dimension::Overworld,
            },
        )
        .bounds();
        assert_eq!((east.min_chunk_x, east.max_chunk_x), (10, 18));
        assert_eq!((east.min_chunk_z, east.max_chunk_z), (20, 26));

        let north_west = resize_chunk_selection(
            selection,
            SelectionResizeHandle::NorthWest,
            ChunkPos {
                x: 7,
                z: 16,
                dimension: Dimension::Overworld,
            },
        )
        .bounds();
        assert_eq!((north_west.min_chunk_x, north_west.max_chunk_x), (7, 14));
        assert_eq!((north_west.min_chunk_z, north_west.max_chunk_z), (16, 26));
    }
}
