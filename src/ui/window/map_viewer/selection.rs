use bedrock_render::{ChunkPos, Dimension};
use bedrock_world::SlimeChunkBounds;
use gpui::{Pixels, Point, px};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static ADDITIVE_RIGHT_SELECTION_REQUESTED: AtomicBool = AtomicBool::new(false);
static ADVANCED_SELECTION_STATE: OnceLock<Mutex<Option<AdvancedSelectionState>>> = OnceLock::new();

#[derive(Clone, Debug)]
struct AdvancedSelectionState {
    selection: ChunkSelection,
    base_chunks: Arc<Vec<ChunkPos>>,
    current_chunks: Arc<Vec<ChunkPos>>,
}

fn advanced_selection_state() -> &'static Mutex<Option<AdvancedSelectionState>> {
    ADVANCED_SELECTION_STATE.get_or_init(|| Mutex::new(None))
}

pub(super) fn set_additive_right_selection_requested(additive: bool) {
    ADDITIVE_RIGHT_SELECTION_REQUESTED.store(additive, Ordering::Release);
}

fn take_additive_right_selection_requested() -> bool {
    ADDITIVE_RIGHT_SELECTION_REQUESTED.swap(false, Ordering::AcqRel)
}

fn clear_advanced_selection_state() {
    if let Ok(mut state) = advanced_selection_state().lock() {
        *state = None;
    }
}

pub(super) fn exact_selection_chunks(selection: ChunkSelection) -> Option<Arc<Vec<ChunkPos>>> {
    let state = advanced_selection_state().lock().ok()?;
    let state = state.as_ref()?;
    (state.selection == selection).then(|| state.current_chunks.clone())
}

fn rectangular_chunk_count(selection: ChunkSelection) -> usize {
    let bounds = selection.bounds();
    let width = i64::from(bounds.max_chunk_x)
        .saturating_sub(i64::from(bounds.min_chunk_x))
        .saturating_add(1);
    let height = i64::from(bounds.max_chunk_z)
        .saturating_sub(i64::from(bounds.min_chunk_z))
        .saturating_add(1);
    usize::try_from(width.saturating_mul(height)).unwrap_or(usize::MAX)
}

fn rectangular_selection_chunks(selection: ChunkSelection) -> Vec<ChunkPos> {
    let bounds = selection.bounds();
    let mut chunks = Vec::with_capacity(rectangular_chunk_count(selection));
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

fn begin_additive_selection(selection: Option<ChunkSelection>) {
    let base_chunks = selection
        .and_then(exact_selection_chunks)
        .map(|chunks| chunks.as_ref().clone())
        .or_else(|| selection.map(rectangular_selection_chunks))
        .unwrap_or_default();
    let base_chunks = Arc::new(base_chunks);
    let fallback = selection.or_else(|| chunk_selection_from_chunks(base_chunks.as_ref()));
    if let Some(selection) = fallback
        && let Ok(mut state) = advanced_selection_state().lock()
    {
        *state = Some(AdvancedSelectionState {
            selection,
            base_chunks: base_chunks.clone(),
            current_chunks: base_chunks,
        });
    } else if selection.is_none() && let Ok(mut state) = advanced_selection_state().lock() {
        *state = None;
    }
}

fn alternate_selection_for_bounds(
    bounds: SlimeChunkBounds,
    previous: Option<ChunkSelection>,
) -> ChunkSelection {
    let min_min = ChunkPos {
        x: bounds.min_chunk_x,
        z: bounds.min_chunk_z,
        dimension: bounds.dimension,
    };
    let max_max = ChunkPos {
        x: bounds.max_chunk_x,
        z: bounds.max_chunk_z,
        dimension: bounds.dimension,
    };
    let min_max = ChunkPos {
        x: bounds.min_chunk_x,
        z: bounds.max_chunk_z,
        dimension: bounds.dimension,
    };
    let max_min = ChunkPos {
        x: bounds.max_chunk_x,
        z: bounds.min_chunk_z,
        dimension: bounds.dimension,
    };
    let candidates = [
        ChunkSelection {
            start: min_min,
            end: max_max,
        },
        ChunkSelection {
            start: max_max,
            end: min_min,
        },
        ChunkSelection {
            start: min_max,
            end: max_min,
        },
        ChunkSelection {
            start: max_min,
            end: min_max,
        },
    ];
    candidates
        .into_iter()
        .find(|candidate| Some(*candidate) != previous)
        .unwrap_or(candidates[0])
}

fn apply_additive_drag(addition: ChunkSelection) -> ChunkSelection {
    let Ok(mut state) = advanced_selection_state().lock() else {
        return addition;
    };
    let Some(current) = state.as_mut() else {
        let chunks = Arc::new(rectangular_selection_chunks(addition));
        *state = Some(AdvancedSelectionState {
            selection: addition,
            base_chunks: Arc::new(Vec::new()),
            current_chunks: chunks,
        });
        return addition;
    };

    let merged = merged_selection_chunks(current.base_chunks.as_ref(), addition);
    if current.current_chunks.as_ref() == &merged {
        return current.selection;
    }
    let Some(bounds_selection) = chunk_selection_from_chunks(&merged) else {
        return current.selection;
    };
    let bounds = bounds_selection.bounds();
    let previous = (current.selection.bounds() == bounds).then_some(current.selection);
    current.selection = alternate_selection_for_bounds(bounds, previous);
    current.current_chunks = Arc::new(merged);
    current.selection
}

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
        rectangular_chunk_count(self)
    }

    pub(super) fn chunks(self) -> Vec<ChunkPos> {
        exact_selection_chunks(self)
            .map(|chunks| chunks.as_ref().clone())
            .unwrap_or_else(|| rectangular_selection_chunks(self))
    }
}

pub(super) fn selection_chunks(
    selection: Option<ChunkSelection>,
    exact_chunks: Option<&[ChunkPos]>,
) -> Vec<ChunkPos> {
    if let Some(chunks) = exact_chunks {
        return chunks.to_vec();
    }
    selection.map(ChunkSelection::chunks).unwrap_or_default()
}

pub(super) fn merged_selection_chunks(
    base_chunks: &[ChunkPos],
    addition: ChunkSelection,
) -> Vec<ChunkPos> {
    let mut chunks = base_chunks.iter().copied().collect::<BTreeSet<_>>();
    chunks.extend(rectangular_selection_chunks(addition));
    chunks.into_iter().collect()
}

pub(super) fn chunk_selection_from_chunks(chunks: &[ChunkPos]) -> Option<ChunkSelection> {
    let first = *chunks.first()?;
    if chunks
        .iter()
        .any(|chunk| chunk.dimension != first.dimension)
    {
        return None;
    }

    let mut min_x = first.x;
    let mut max_x = first.x;
    let mut min_z = first.z;
    let mut max_z = first.z;
    for chunk in chunks.iter().copied().skip(1) {
        min_x = min_x.min(chunk.x);
        max_x = max_x.max(chunk.x);
        min_z = min_z.min(chunk.z);
        max_z = max_z.max(chunk.z);
    }

    Some(ChunkSelection {
        start: ChunkPos {
            x: min_x,
            z: min_z,
            dimension: first.dimension,
        },
        end: ChunkPos {
            x: max_x,
            z: max_z,
            dimension: first.dimension,
        },
    })
}

pub(super) fn selection_chunks_are_rectangular(
    selection: ChunkSelection,
    exact_chunks: Option<&[ChunkPos]>,
) -> bool {
    let Some(chunks) = exact_chunks else {
        return true;
    };
    if chunks.len() != rectangular_chunk_count(selection) {
        return false;
    }
    let bounds = selection.bounds();
    chunks.iter().all(|chunk| {
        chunk.dimension == bounds.dimension
            && chunk.x >= bounds.min_chunk_x
            && chunk.x <= bounds.max_chunk_x
            && chunk.z >= bounds.min_chunk_z
            && chunk.z <= bounds.max_chunk_z
    })
}

pub(super) fn selection_contains_chunk(
    selection: ChunkSelection,
    exact_chunks: Option<&[ChunkPos]>,
    chunk: ChunkPos,
) -> bool {
    if let Some(chunks) = exact_chunks {
        return chunks.binary_search(&chunk).is_ok();
    }
    let bounds = selection.bounds();
    chunk.dimension == bounds.dimension
        && chunk.x >= bounds.min_chunk_x
        && chunk.x <= bounds.max_chunk_x
        && chunk.z >= bounds.min_chunk_z
        && chunk.z <= bounds.max_chunk_z
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
        if take_additive_right_selection_requested() {
            begin_additive_selection(None);
            return Self::additive(start_position, start_chunk);
        }
        clear_advanced_selection_state();
        Self::with_intent(
            start_position,
            start_chunk,
            SelectionPointerButton::Right,
            RightSelectionIntent::NewSelection,
        )
    }

    pub(super) fn additive(start_position: Point<Pixels>, start_chunk: ChunkPos) -> Self {
        Self::with_intent(
            start_position,
            start_chunk,
            SelectionPointerButton::Right,
            RightSelectionIntent::AddSelection,
        )
    }

    pub(super) fn existing_for_button(
        start_position: Point<Pixels>,
        start_chunk: ChunkPos,
        selection: ChunkSelection,
        target: ExistingSelectionTarget,
        button: SelectionPointerButton,
    ) -> Self {
        if button == SelectionPointerButton::Right && take_additive_right_selection_requested() {
            begin_additive_selection(Some(selection));
            return Self::additive(start_position, start_chunk);
        }

        let exact_chunks = exact_selection_chunks(selection);
        let target = if exact_chunks
            .as_deref()
            .is_some_and(|chunks| !selection_chunks_are_rectangular(selection, Some(chunks)))
        {
            if selection_contains_chunk(selection, exact_chunks.as_deref(), start_chunk) {
                ExistingSelectionTarget::Inside
            } else {
                ExistingSelectionTarget::Outside
            }
        } else {
            target
        };
        let intent = match target {
            ExistingSelectionTarget::Inside => match button {
                SelectionPointerButton::Left => RightSelectionIntent::Move(selection),
                SelectionPointerButton::Right => RightSelectionIntent::OpenMenu(selection),
            },
            ExistingSelectionTarget::Outside => RightSelectionIntent::Cancel(selection),
            ExistingSelectionTarget::Resize(handle) => RightSelectionIntent::Resize {
                selection,
                handle,
            },
        };
        if matches!(
            intent,
            RightSelectionIntent::Move(_)
                | RightSelectionIntent::Cancel(_)
                | RightSelectionIntent::Resize { .. }
        ) {
            clear_advanced_selection_state();
        }
        Self::with_intent(start_position, start_chunk, button, intent)
    }

    pub(super) fn with_intent(
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
            RightSelectionIntent::AddSelection => apply_additive_drag(ChunkSelection {
                start: self.start_chunk,
                end: self.current_chunk,
            }),
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
                | RightSelectionIntent::AddSelection
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

#[derive(Clone, Copy, Debug)]
pub(super) enum RightSelectionIntent {
    NewSelection,
    AddSelection,
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

pub(super) fn right_selection_release_action(
    button: SelectionPointerButton,
    intent: RightSelectionIntent,
    moved: bool,
) -> RightSelectionReleaseAction {
    match intent {
        RightSelectionIntent::Cancel(_) => RightSelectionReleaseAction::CancelSelection,
        RightSelectionIntent::Move(_) if !moved => RightSelectionReleaseAction::KeepSelection,
        RightSelectionIntent::Resize { .. }
            if button == SelectionPointerButton::Left && !moved =>
        {
            RightSelectionReleaseAction::KeepSelection
        }
        RightSelectionIntent::Move(_) | RightSelectionIntent::Resize { .. } if moved => {
            RightSelectionReleaseAction::ApplySelection
        }
        RightSelectionIntent::OpenMenu(_)
            if button == SelectionPointerButton::Right && !moved =>
        {
            RightSelectionReleaseAction::OpenMenu
        }
        RightSelectionIntent::OpenMenu(_) if moved => {
            RightSelectionReleaseAction::KeepSelection
        }
        RightSelectionIntent::NewSelection => {
            RightSelectionReleaseAction::ApplySelectionAndOpenMenu
        }
        RightSelectionIntent::AddSelection => RightSelectionReleaseAction::ApplySelection,
        RightSelectionIntent::OpenMenu(_) => RightSelectionReleaseAction::KeepSelection,
        RightSelectionIntent::Resize { .. } => RightSelectionReleaseAction::KeepSelection,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExistingSelectionTarget {
    Outside,
    Inside,
    Resize(SelectionResizeHandle),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SelectionResizeHandle {
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SelectionScreenBounds {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) right: f32,
    pub(super) bottom: f32,
}

pub(super) fn existing_selection_target(
    position: Point<Pixels>,
    bounds: SelectionScreenBounds,
    tolerance_px: f32,
) -> ExistingSelectionTarget {
    let x = position.x / px(1.0);
    let y = position.y / px(1.0);
    if x < bounds.left - tolerance_px
        || x > bounds.right + tolerance_px
        || y < bounds.top - tolerance_px
        || y > bounds.bottom + tolerance_px
    {
        return ExistingSelectionTarget::Outside;
    }

    let near_left = (x - bounds.left).abs() <= tolerance_px;
    let near_right = (x - bounds.right).abs() <= tolerance_px;
    let near_top = (y - bounds.top).abs() <= tolerance_px;
    let near_bottom = (y - bounds.bottom).abs() <= tolerance_px;
    let handle = match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => Some(SelectionResizeHandle::NorthWest),
        (_, true, true, _) => Some(SelectionResizeHandle::NorthEast),
        (true, _, _, true) => Some(SelectionResizeHandle::SouthWest),
        (_, true, _, true) => Some(SelectionResizeHandle::SouthEast),
        (_, _, true, _) => Some(SelectionResizeHandle::North),
        (_, true, _, _) => Some(SelectionResizeHandle::East),
        (_, _, _, true) => Some(SelectionResizeHandle::South),
        (true, _, _, _) => Some(SelectionResizeHandle::West),
        _ => None,
    };
    if let Some(handle) = handle {
        return ExistingSelectionTarget::Resize(handle);
    }
    if x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom {
        ExistingSelectionTarget::Inside
    } else {
        ExistingSelectionTarget::Outside
    }
}

pub(super) fn resize_chunk_selection(
    selection: ChunkSelection,
    handle: SelectionResizeHandle,
    current: ChunkPos,
) -> ChunkSelection {
    let bounds = selection.bounds();
    let mut min_x = bounds.min_chunk_x;
    let mut max_x = bounds.max_chunk_x;
    let mut min_z = bounds.min_chunk_z;
    let mut max_z = bounds.max_chunk_z;
    match handle {
        SelectionResizeHandle::NorthWest => {
            min_x = current.x.min(max_x);
            min_z = current.z.min(max_z);
        }
        SelectionResizeHandle::North => min_z = current.z.min(max_z),
        SelectionResizeHandle::NorthEast => {
            max_x = current.x.max(min_x);
            min_z = current.z.min(max_z);
        }
        SelectionResizeHandle::East => max_x = current.x.max(min_x),
        SelectionResizeHandle::SouthEast => {
            max_x = current.x.max(min_x);
            max_z = current.z.max(min_z);
        }
        SelectionResizeHandle::South => max_z = current.z.max(min_z),
        SelectionResizeHandle::SouthWest => {
            min_x = current.x.min(max_x);
            max_z = current.z.max(min_z);
        }
        SelectionResizeHandle::West => min_x = current.x.min(max_x),
    }
    ChunkSelection {
        start: ChunkPos {
            x: min_x,
            z: min_z,
            dimension: bounds.dimension,
        },
        end: ChunkPos {
            x: max_x,
            z: max_z,
            dimension: bounds.dimension,
        },
    }
}

pub(super) fn translate_chunk_selection(
    selection: ChunkSelection,
    delta_x: i32,
    delta_z: i32,
) -> ChunkSelection {
    ChunkSelection {
        start: ChunkPos {
            x: selection.start.x.saturating_add(delta_x),
            z: selection.start.z.saturating_add(delta_z),
            dimension: selection.start.dimension,
        },
        end: ChunkPos {
            x: selection.end.x.saturating_add(delta_x),
            z: selection.end.z.saturating_add(delta_z),
            dimension: selection.end.dimension,
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
    threshold_px: f32,
) -> bool {
    let dx = (current.x - start.x) / px(1.0);
    let dy = (current.y - start.y) / px(1.0);
    dx.abs().max(dy.abs()) >= threshold_px
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(x: i32, z: i32) -> ChunkPos {
        ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        }
    }

    #[test]
    fn selection_bounds_are_normalized() {
        let selection = ChunkSelection {
            start: chunk(5, 8),
            end: chunk(2, 3),
        };
        let bounds = selection.bounds();
        assert_eq!(bounds.min_chunk_x, 2);
        assert_eq!(bounds.max_chunk_x, 5);
        assert_eq!(bounds.min_chunk_z, 3);
        assert_eq!(bounds.max_chunk_z, 8);
        assert_eq!(selection.chunk_count(), 24);
    }

    #[test]
    fn right_drag_selection_is_normalized() {
        let mut drag = RightSelectionDrag::new(Point::default(), chunk(4, 7));
        drag.current_chunk = chunk(1, 2);
        let bounds = drag.selection().bounds();
        assert_eq!(bounds.min_chunk_x, 1);
        assert_eq!(bounds.max_chunk_x, 4);
        assert_eq!(bounds.min_chunk_z, 2);
        assert_eq!(bounds.max_chunk_z, 7);
    }

    #[test]
    fn left_drag_inside_selection_moves_it() {
        let selection = ChunkSelection {
            start: chunk(1, 2),
            end: chunk(3, 4),
        };
        let mut drag = RightSelectionDrag::existing_for_button(
            Point::default(),
            chunk(2, 3),
            selection,
            ExistingSelectionTarget::Inside,
            SelectionPointerButton::Left,
        );
        drag.current_chunk = chunk(4, 5);
        drag.moved = true;
        let moved = drag.selection().bounds();
        assert_eq!(moved.min_chunk_x, 3);
        assert_eq!(moved.max_chunk_x, 5);
        assert_eq!(moved.min_chunk_z, 4);
        assert_eq!(moved.max_chunk_z, 6);
    }

    #[test]
    fn selection_resize_uses_requested_edge() {
        let selection = ChunkSelection {
            start: chunk(1, 2),
            end: chunk(3, 4),
        };
        let resized = resize_chunk_selection(selection, SelectionResizeHandle::East, chunk(7, 3));
        let bounds = resized.bounds();
        assert_eq!(bounds.min_chunk_x, 1);
        assert_eq!(bounds.max_chunk_x, 7);
        assert_eq!(bounds.min_chunk_z, 2);
        assert_eq!(bounds.max_chunk_z, 4);
    }

    #[test]
    fn additive_selection_keeps_non_rectangular_exact_chunks() {
        clear_advanced_selection_state();
        let horizontal = ChunkSelection {
            start: chunk(-1, 0),
            end: chunk(1, 0),
        };
        begin_additive_selection(Some(horizontal));
        let mut drag = RightSelectionDrag::additive(Point::default(), chunk(0, -1));
        drag.current_chunk = chunk(0, 1);
        let bounds = drag.selection();
        let exact = exact_selection_chunks(bounds).expect("exact selection chunks");

        assert_eq!(exact.len(), 5);
        assert_eq!(bounds.chunk_count(), 9);
        assert!(!selection_chunks_are_rectangular(bounds, Some(&exact)));
        assert!(selection_contains_chunk(bounds, Some(&exact), chunk(0, 0)));
        assert!(!selection_contains_chunk(bounds, Some(&exact), chunk(1, 1)));
        clear_advanced_selection_state();
    }

    #[test]
    fn additive_right_selection_does_not_open_context_menu() {
        let drag = RightSelectionDrag::additive(Point::default(), chunk(0, 0));
        assert_eq!(
            right_selection_release_action(drag.button, drag.intent, drag.moved),
            RightSelectionReleaseAction::ApplySelection
        );
    }
}
