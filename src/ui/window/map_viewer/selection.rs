use bedrock_render::{ChunkPos, Dimension};
use bedrock_world::SlimeChunkBounds;
use gpui::{Pixels, Point, px};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static ADDITIVE_RIGHT_SELECTION_REQUESTED: AtomicBool = AtomicBool::new(false);
static NEXT_ADVANCED_SELECTION_ID: AtomicU64 = AtomicU64::new(1);
static ADVANCED_SELECTION_STATES: OnceLock<Mutex<HashMap<u64, AdvancedSelectionState>>> =
    OnceLock::new();
const MAX_RETAINED_ADVANCED_SELECTIONS: usize = 256;

#[derive(Clone, Debug)]
struct AdvancedSelectionState {
    selection: Option<ChunkSelection>,
    base_chunks: Arc<Vec<ChunkPos>>,
    current_chunks: Arc<Vec<ChunkPos>>,
    revision: u64,
}

fn advanced_selection_states() -> &'static Mutex<HashMap<u64, AdvancedSelectionState>> {
    ADVANCED_SELECTION_STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_advanced_selection_id() -> u64 {
    loop {
        let id = NEXT_ADVANCED_SELECTION_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

fn trim_advanced_selection_states(states: &mut HashMap<u64, AdvancedSelectionState>) {
    while states.len() > MAX_RETAINED_ADVANCED_SELECTIONS {
        let Some(oldest) = states.keys().copied().min() else {
            break;
        };
        states.remove(&oldest);
    }
}

pub(super) fn set_additive_right_selection_requested(additive: bool) {
    ADDITIVE_RIGHT_SELECTION_REQUESTED.store(additive, Ordering::Release);
}

fn take_additive_right_selection_requested() -> bool {
    ADDITIVE_RIGHT_SELECTION_REQUESTED.swap(false, Ordering::AcqRel)
}

fn remove_advanced_selection_state(selection: ChunkSelection) {
    if selection.advanced_id == 0 {
        return;
    }
    if let Ok(mut states) = advanced_selection_states().lock() {
        states.remove(&selection.advanced_id);
    }
}

pub(super) fn exact_selection_chunks(selection: ChunkSelection) -> Option<Arc<Vec<ChunkPos>>> {
    if selection.advanced_id == 0 {
        return None;
    }
    let states = advanced_selection_states().lock().ok()?;
    let state = states.get(&selection.advanced_id)?;
    (state.revision == selection.advanced_revision && state.selection == Some(selection))
        .then(|| state.current_chunks.clone())
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

fn normalize_chunk_set(chunks: impl IntoIterator<Item = ChunkPos>) -> Vec<ChunkPos> {
    let mut seen = HashSet::new();
    let mut chunks = chunks
        .into_iter()
        .filter(|chunk| seen.insert(*chunk))
        .collect::<Vec<_>>();
    chunks.sort_unstable_by_key(|chunk| (chunk.z, chunk.x));
    chunks
}

fn begin_additive_selection(selection: Option<ChunkSelection>) -> u64 {
    let base_chunks = selection
        .and_then(exact_selection_chunks)
        .map(|chunks| chunks.as_ref().clone())
        .or_else(|| selection.map(rectangular_selection_chunks))
        .unwrap_or_default();
    if let Some(selection) = selection {
        remove_advanced_selection_state(selection);
    }

    let id = next_advanced_selection_id();
    if let Ok(mut states) = advanced_selection_states().lock() {
        states.insert(
            id,
            AdvancedSelectionState {
                selection: None,
                base_chunks: Arc::new(base_chunks.clone()),
                current_chunks: Arc::new(base_chunks),
                revision: 0,
            },
        );
        trim_advanced_selection_states(&mut states);
    }
    id
}

fn begin_exact_move(selection: ChunkSelection, chunks: Arc<Vec<ChunkPos>>) -> u64 {
    remove_advanced_selection_state(selection);
    let id = next_advanced_selection_id();
    if let Ok(mut states) = advanced_selection_states().lock() {
        states.insert(
            id,
            AdvancedSelectionState {
                selection: None,
                base_chunks: chunks.clone(),
                current_chunks: chunks,
                revision: 0,
            },
        );
        trim_advanced_selection_states(&mut states);
    }
    id
}

fn apply_advanced_chunks(
    advanced_id: u64,
    chunks: Vec<ChunkPos>,
    fallback: ChunkSelection,
) -> ChunkSelection {
    let Ok(mut states) = advanced_selection_states().lock() else {
        return fallback;
    };
    let Some(state) = states.get_mut(&advanced_id) else {
        return fallback;
    };
    if state.current_chunks.as_ref() == &chunks {
        return state.selection.unwrap_or(fallback);
    }
    let Some(mut selection) = chunk_selection_from_chunks(&chunks) else {
        return state.selection.unwrap_or(fallback);
    };
    state.revision = state.revision.saturating_add(1).max(1);
    selection.advanced_id = advanced_id;
    selection.advanced_revision = state.revision;
    state.selection = Some(selection);
    state.current_chunks = Arc::new(chunks);
    selection
}

fn apply_additive_drag(advanced_id: u64, addition: ChunkSelection) -> ChunkSelection {
    let base_chunks = advanced_selection_states()
        .lock()
        .ok()
        .and_then(|states| states.get(&advanced_id).map(|state| state.base_chunks.clone()));
    let Some(base_chunks) = base_chunks else {
        return addition;
    };
    let merged = merged_selection_chunks(base_chunks.as_ref(), addition);
    apply_advanced_chunks(advanced_id, merged, addition)
}

fn apply_exact_move(
    advanced_id: u64,
    delta_x: i32,
    delta_z: i32,
    fallback: ChunkSelection,
) -> ChunkSelection {
    let base_chunks = advanced_selection_states()
        .lock()
        .ok()
        .and_then(|states| states.get(&advanced_id).map(|state| state.base_chunks.clone()));
    let Some(base_chunks) = base_chunks else {
        return translate_chunk_selection(fallback, delta_x, delta_z);
    };
    let translated = normalize_chunk_set(base_chunks.iter().copied().map(|chunk| ChunkPos {
        x: chunk.x.saturating_add(delta_x),
        z: chunk.z.saturating_add(delta_z),
        dimension: chunk.dimension,
    }));
    apply_advanced_chunks(
        advanced_id,
        translated,
        translate_chunk_selection(fallback, delta_x, delta_z),
    )
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ChunkSelection {
    pub(super) start: ChunkPos,
    pub(super) end: ChunkPos,
    advanced_id: u64,
    advanced_revision: u64,
}

impl ChunkSelection {
    fn rectangular(start: ChunkPos, end: ChunkPos) -> Self {
        Self {
            start,
            end,
            advanced_id: 0,
            advanced_revision: 0,
        }
    }

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
        exact_selection_chunks(self)
            .map(|chunks| chunks.len())
            .unwrap_or_else(|| rectangular_chunk_count(self))
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
    normalize_chunk_set(
        base_chunks
            .iter()
            .copied()
            .chain(rectangular_selection_chunks(addition)),
    )
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

    Some(ChunkSelection::rectangular(
        ChunkPos {
            x: min_x,
            z: min_z,
            dimension: first.dimension,
        },
        ChunkPos {
            x: max_x,
            z: max_z,
            dimension: first.dimension,
        },
    ))
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
        return chunks.contains(&chunk);
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
    advanced_id: Option<u64>,
}

impl RightSelectionDrag {
    pub(super) fn new(start_position: Point<Pixels>, start_chunk: ChunkPos) -> Self {
        if take_additive_right_selection_requested() {
            let advanced_id = begin_additive_selection(None);
            return Self::additive(start_position, start_chunk, advanced_id);
        }
        Self::with_intent(
            start_position,
            start_chunk,
            SelectionPointerButton::Right,
            RightSelectionIntent::NewSelection,
            None,
        )
    }

    fn additive(
        start_position: Point<Pixels>,
        start_chunk: ChunkPos,
        advanced_id: u64,
    ) -> Self {
        Self::with_intent(
            start_position,
            start_chunk,
            SelectionPointerButton::Right,
            RightSelectionIntent::AddSelection,
            Some(advanced_id),
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
            let advanced_id = begin_additive_selection(Some(selection));
            return Self::additive(start_position, start_chunk, advanced_id);
        }

        let exact_chunks = exact_selection_chunks(selection);
        let irregular = exact_chunks
            .as_deref()
            .is_some_and(|chunks| !selection_chunks_are_rectangular(selection, Some(chunks)));
        let target = if irregular {
            if selection_contains_chunk(selection, exact_chunks.as_deref(), start_chunk) {
                ExistingSelectionTarget::Inside
            } else {
                ExistingSelectionTarget::Outside
            }
        } else {
            target
        };
        let (intent, advanced_id) = match target {
            ExistingSelectionTarget::Inside => match button {
                SelectionPointerButton::Left if irregular => {
                    let advanced_id = exact_chunks.map(|chunks| begin_exact_move(selection, chunks));
                    (RightSelectionIntent::MoveExact(selection), advanced_id)
                }
                SelectionPointerButton::Left => (RightSelectionIntent::Move(selection), None),
                SelectionPointerButton::Right => {
                    (RightSelectionIntent::OpenMenu(selection), None)
                }
            },
            ExistingSelectionTarget::Outside => {
                remove_advanced_selection_state(selection);
                (RightSelectionIntent::Cancel(selection), None)
            }
            ExistingSelectionTarget::Resize(handle) => {
                remove_advanced_selection_state(selection);
                (RightSelectionIntent::Resize { selection, handle }, None)
            }
        };
        Self::with_intent(start_position, start_chunk, button, intent, advanced_id)
    }

    pub(super) fn with_intent(
        start_position: Point<Pixels>,
        start_chunk: ChunkPos,
        button: SelectionPointerButton,
        intent: RightSelectionIntent,
        advanced_id: Option<u64>,
    ) -> Self {
        Self {
            start_position,
            start_chunk,
            current_chunk: start_chunk,
            moved: false,
            button,
            intent,
            advanced_id,
        }
    }

    pub(super) fn selection(self) -> ChunkSelection {
        match self.intent {
            RightSelectionIntent::NewSelection => {
                ChunkSelection::rectangular(self.start_chunk, self.current_chunk)
            }
            RightSelectionIntent::AddSelection => self.advanced_id.map_or_else(
                || ChunkSelection::rectangular(self.start_chunk, self.current_chunk),
                |advanced_id| {
                    apply_additive_drag(
                        advanced_id,
                        ChunkSelection::rectangular(self.start_chunk, self.current_chunk),
                    )
                },
            ),
            RightSelectionIntent::Resize { selection, handle } => {
                resize_chunk_selection(selection, handle, self.current_chunk)
            }
            RightSelectionIntent::Move(selection) => translate_chunk_selection(
                selection,
                self.current_chunk.x.saturating_sub(self.start_chunk.x),
                self.current_chunk.z.saturating_sub(self.start_chunk.z),
            ),
            RightSelectionIntent::MoveExact(selection) => self.advanced_id.map_or_else(
                || {
                    translate_chunk_selection(
                        selection,
                        self.current_chunk.x.saturating_sub(self.start_chunk.x),
                        self.current_chunk.z.saturating_sub(self.start_chunk.z),
                    )
                },
                |advanced_id| {
                    apply_exact_move(
                        advanced_id,
                        self.current_chunk.x.saturating_sub(self.start_chunk.x),
                        self.current_chunk.z.saturating_sub(self.start_chunk.z),
                        selection,
                    )
                },
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
                | RightSelectionIntent::MoveExact(_)
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
    MoveExact(ChunkSelection),
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
        RightSelectionIntent::Move(_) | RightSelectionIntent::MoveExact(_) if !moved => {
            RightSelectionReleaseAction::KeepSelection
        }
        RightSelectionIntent::Resize { .. }
            if button == SelectionPointerButton::Left && !moved =>
        {
            RightSelectionReleaseAction::KeepSelection
        }
        RightSelectionIntent::Move(_)
        | RightSelectionIntent::MoveExact(_)
        | RightSelectionIntent::Resize { .. }
            if moved =>
        {
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
    ChunkSelection::rectangular(
        ChunkPos {
            x: min_x,
            z: min_z,
            dimension: bounds.dimension,
        },
        ChunkPos {
            x: max_x,
            z: max_z,
            dimension: bounds.dimension,
        },
    )
}

pub(super) fn translate_chunk_selection(
    selection: ChunkSelection,
    delta_x: i32,
    delta_z: i32,
) -> ChunkSelection {
    ChunkSelection::rectangular(
        ChunkPos {
            x: selection.start.x.saturating_add(delta_x),
            z: selection.start.z.saturating_add(delta_z),
            dimension: selection.start.dimension,
        },
        ChunkPos {
            x: selection.end.x.saturating_add(delta_x),
            z: selection.end.z.saturating_add(delta_z),
            dimension: selection.end.dimension,
        },
    )
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

    fn selection(start: ChunkPos, end: ChunkPos) -> ChunkSelection {
        ChunkSelection::rectangular(start, end)
    }

    #[test]
    fn selection_bounds_are_normalized() {
        let selection = selection(chunk(5, 8), chunk(2, 3));
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
        let selection = selection(chunk(1, 2), chunk(3, 4));
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
        let selection = selection(chunk(1, 2), chunk(3, 4));
        let resized = resize_chunk_selection(selection, SelectionResizeHandle::East, chunk(7, 3));
        let bounds = resized.bounds();
        assert_eq!(bounds.min_chunk_x, 1);
        assert_eq!(bounds.max_chunk_x, 7);
        assert_eq!(bounds.min_chunk_z, 2);
        assert_eq!(bounds.max_chunk_z, 4);
    }

    #[test]
    fn additive_selection_keeps_non_rectangular_exact_chunks() {
        let horizontal = selection(chunk(-1, 0), chunk(1, 0));
        let advanced_id = begin_additive_selection(Some(horizontal));
        let mut drag = RightSelectionDrag::additive(Point::default(), chunk(0, -1), advanced_id);
        drag.current_chunk = chunk(0, 1);
        let bounds = drag.selection();
        let exact = exact_selection_chunks(bounds).expect("exact selection chunks");

        assert_eq!(exact.len(), 5);
        assert_eq!(bounds.chunk_count(), 5);
        assert!(!selection_chunks_are_rectangular(bounds, Some(&exact)));
        assert!(selection_contains_chunk(bounds, Some(&exact), chunk(0, 0)));
        assert!(!selection_contains_chunk(bounds, Some(&exact), chunk(1, 1)));
    }

    #[test]
    fn irregular_selection_move_keeps_shape() {
        let horizontal = selection(chunk(-1, 0), chunk(1, 0));
        let add_id = begin_additive_selection(Some(horizontal));
        let mut add = RightSelectionDrag::additive(Point::default(), chunk(0, -1), add_id);
        add.current_chunk = chunk(0, 1);
        let selection = add.selection();
        let exact = exact_selection_chunks(selection).expect("exact selection chunks");
        let move_id = begin_exact_move(selection, exact);
        let mut drag = RightSelectionDrag::with_intent(
            Point::default(),
            chunk(0, 0),
            SelectionPointerButton::Left,
            RightSelectionIntent::MoveExact(selection),
            Some(move_id),
        );
        drag.current_chunk = chunk(2, 3);
        drag.moved = true;
        let moved = drag.selection();
        let moved_chunks = exact_selection_chunks(moved).expect("moved exact chunks");

        assert_eq!(moved_chunks.len(), 5);
        assert!(moved_chunks.contains(&chunk(2, 3)));
        assert!(moved_chunks.contains(&chunk(1, 3)));
        assert!(moved_chunks.contains(&chunk(2, 2)));
    }

    #[test]
    fn advanced_selection_ids_isolate_simultaneous_shapes() {
        let first_id = begin_additive_selection(Some(selection(chunk(0, 0), chunk(0, 0))));
        let mut first = RightSelectionDrag::additive(Point::default(), chunk(1, 0), first_id);
        first.current_chunk = chunk(1, 1);
        let first_selection = first.selection();
        let first_chunks = exact_selection_chunks(first_selection).expect("first exact chunks");

        let second_id = begin_additive_selection(Some(selection(chunk(10, 10), chunk(10, 10))));
        let mut second = RightSelectionDrag::additive(Point::default(), chunk(11, 10), second_id);
        second.current_chunk = chunk(11, 11);
        let second_selection = second.selection();
        let second_chunks = exact_selection_chunks(second_selection).expect("second exact chunks");

        assert_ne!(first_selection.advanced_id, second_selection.advanced_id);
        assert_eq!(first_chunks.len(), 3);
        assert_eq!(second_chunks.len(), 3);
        assert!(exact_selection_chunks(first_selection).is_some());
        assert!(exact_selection_chunks(second_selection).is_some());
    }

    #[test]
    fn advanced_revision_changes_when_shape_changes_inside_same_bounds() {
        let base = selection(chunk(-1, 0), chunk(1, 0));
        let advanced_id = begin_additive_selection(Some(base));
        let mut drag = RightSelectionDrag::additive(Point::default(), chunk(0, -1), advanced_id);
        drag.current_chunk = chunk(0, 1);
        let cross = drag.selection();
        drag.current_chunk = chunk(1, 1);
        let larger = drag.selection();

        assert_eq!(cross.bounds().min_chunk_x, larger.bounds().min_chunk_x);
        assert_eq!(cross.bounds().max_chunk_x, larger.bounds().max_chunk_x);
        assert_eq!(cross.bounds().min_chunk_z, larger.bounds().min_chunk_z);
        assert_eq!(cross.bounds().max_chunk_z, larger.bounds().max_chunk_z);
        assert_ne!(cross, larger);
    }

    #[test]
    fn additive_right_selection_does_not_open_context_menu() {
        let advanced_id = begin_additive_selection(None);
        let drag = RightSelectionDrag::additive(Point::default(), chunk(0, 0), advanced_id);
        assert_eq!(
            right_selection_release_action(drag.button, drag.intent, drag.moved),
            RightSelectionReleaseAction::ApplySelection
        );
    }
}
