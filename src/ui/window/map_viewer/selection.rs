use bedrock_render::{ChunkPos, Dimension};
use bedrock_world::SlimeChunkBounds;
use gpui::{Pixels, Point, px};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static ADDITIVE_RIGHT_SELECTION_REQUESTED: AtomicBool = AtomicBool::new(false);
static NEXT_ADVANCED_SELECTION_SESSION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_ADVANCED_SELECTION_TOKEN: AtomicU32 = AtomicU32::new(1);
static ADVANCED_SELECTION_REGISTRY: OnceLock<Mutex<AdvancedSelectionRegistry>> = OnceLock::new();
const MAX_RETAINED_ADVANCED_SELECTIONS: usize = 4096;
const ADVANCED_SELECTION_TOKEN_MASK: u32 = 0x7fff_ffff;

#[derive(Clone, Debug)]
struct AdvancedSelectionState {
    selection: Option<ChunkSelection>,
    base_chunks: Arc<Vec<ChunkPos>>,
    current_chunks: Arc<Vec<ChunkPos>>,
    token: Option<u32>,
}

#[derive(Default)]
struct AdvancedSelectionRegistry {
    sessions: HashMap<u64, AdvancedSelectionState>,
    token_sessions: HashMap<u32, u64>,
}

fn advanced_selection_registry() -> &'static Mutex<AdvancedSelectionRegistry> {
    ADVANCED_SELECTION_REGISTRY.get_or_init(|| Mutex::new(AdvancedSelectionRegistry::default()))
}

fn next_advanced_selection_session_id() -> u64 {
    loop {
        let id = NEXT_ADVANCED_SELECTION_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        if id != 0 {
            return id;
        }
    }
}

fn advanced_selection_marker_dimension(token: u32) -> Dimension {
    let token = token & ADVANCED_SELECTION_TOKEN_MASK;
    Dimension::Unknown(i32::MIN.saturating_add(token as i32))
}

fn advanced_selection_token(selection: ChunkSelection) -> Option<u32> {
    if selection.start.dimension == selection.end.dimension {
        return None;
    }
    let Dimension::Unknown(marker) = selection.end.dimension else {
        return None;
    };
    if marker >= 0 {
        return None;
    }
    Some(marker.wrapping_sub(i32::MIN) as u32)
}

fn next_advanced_selection_token_for_dimension(
    registry: &AdvancedSelectionRegistry,
    logical_dimension: Dimension,
) -> u32 {
    loop {
        let token = NEXT_ADVANCED_SELECTION_TOKEN
            .fetch_add(1, Ordering::Relaxed)
            & ADVANCED_SELECTION_TOKEN_MASK;
        if registry.token_sessions.contains_key(&token) {
            continue;
        }
        if advanced_selection_marker_dimension(token) == logical_dimension {
            continue;
        }
        return token;
    }
}

fn trim_advanced_selection_registry(registry: &mut AdvancedSelectionRegistry) {
    while registry.sessions.len() > MAX_RETAINED_ADVANCED_SELECTIONS {
        let Some(oldest_session_id) = registry.sessions.keys().copied().min() else {
            break;
        };
        if let Some(state) = registry.sessions.remove(&oldest_session_id)
            && let Some(token) = state.token
        {
            registry.token_sessions.remove(&token);
        }
    }
}

pub(super) fn set_additive_right_selection_requested(additive: bool) {
    ADDITIVE_RIGHT_SELECTION_REQUESTED.store(additive, Ordering::Release);
}

fn take_additive_right_selection_requested() -> bool {
    ADDITIVE_RIGHT_SELECTION_REQUESTED.swap(false, Ordering::AcqRel)
}

fn remove_advanced_selection_state(selection: ChunkSelection) {
    let Some(token) = advanced_selection_token(selection) else {
        return;
    };
    let Ok(mut registry) = advanced_selection_registry().lock() else {
        return;
    };
    let Some(session_id) = registry.token_sessions.remove(&token) else {
        return;
    };
    if registry
        .sessions
        .get(&session_id)
        .is_some_and(|state| state.selection == Some(selection))
    {
        registry.sessions.remove(&session_id);
    }
}

pub(super) fn exact_selection_chunks(selection: ChunkSelection) -> Option<Arc<Vec<ChunkPos>>> {
    let token = advanced_selection_token(selection)?;
    let registry = advanced_selection_registry().lock().ok()?;
    let session_id = *registry.token_sessions.get(&token)?;
    let state = registry.sessions.get(&session_id)?;
    (state.selection == Some(selection)).then(|| state.current_chunks.clone())
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

fn advanced_selection_identity(
    registry: &AdvancedSelectionRegistry,
    chunks: &[ChunkPos],
) -> Option<(ChunkSelection, u32)> {
    let mut selection = chunk_selection_from_chunks(chunks)?;
    let token = next_advanced_selection_token_for_dimension(registry, selection.start.dimension);
    selection.end.dimension = advanced_selection_marker_dimension(token);
    Some((selection, token))
}

fn create_advanced_selection_session(base_chunks: Vec<ChunkPos>) -> u64 {
    let session_id = next_advanced_selection_session_id();
    let Ok(mut registry) = advanced_selection_registry().lock() else {
        return session_id;
    };
    let identity = advanced_selection_identity(&registry, &base_chunks);
    let (selection, token) = identity.map_or((None, None), |(selection, token)| {
        (Some(selection), Some(token))
    });
    if let Some(token) = token {
        registry.token_sessions.insert(token, session_id);
    }
    registry.sessions.insert(
        session_id,
        AdvancedSelectionState {
            selection,
            base_chunks: Arc::new(base_chunks.clone()),
            current_chunks: Arc::new(base_chunks),
            token,
        },
    );
    trim_advanced_selection_registry(&mut registry);
    session_id
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
    create_advanced_selection_session(base_chunks)
}

fn begin_exact_move(selection: ChunkSelection, chunks: Arc<Vec<ChunkPos>>) -> u64 {
    remove_advanced_selection_state(selection);
    create_advanced_selection_session(chunks.as_ref().clone())
}

fn apply_advanced_chunks(
    session_id: u64,
    chunks: Vec<ChunkPos>,
    fallback: ChunkSelection,
) -> ChunkSelection {
    let Ok(mut registry) = advanced_selection_registry().lock() else {
        return fallback;
    };

    if let Some(state) = registry.sessions.get(&session_id)
        && state.current_chunks.as_ref() == &chunks
    {
        return state.selection.unwrap_or(fallback);
    }

    let Some((selection, token)) = advanced_selection_identity(&registry, &chunks) else {
        return registry
            .sessions
            .get(&session_id)
            .and_then(|state| state.selection)
            .unwrap_or(fallback);
    };

    let old_token = registry
        .sessions
        .get(&session_id)
        .and_then(|state| state.token);
    if let Some(old_token) = old_token {
        registry.token_sessions.remove(&old_token);
    }
    registry.token_sessions.insert(token, session_id);

    let Some(state) = registry.sessions.get_mut(&session_id) else {
        registry.token_sessions.remove(&token);
        return fallback;
    };
    state.selection = Some(selection);
    state.current_chunks = Arc::new(chunks);
    state.token = Some(token);
    selection
}

fn apply_additive_drag(session_id: u64, addition: ChunkSelection) -> ChunkSelection {
    let base_chunks = advanced_selection_registry()
        .lock()
        .ok()
        .and_then(|registry| {
            registry
                .sessions
                .get(&session_id)
                .map(|state| state.base_chunks.clone())
        });
    let Some(base_chunks) = base_chunks else {
        return addition;
    };
    let merged = merged_selection_chunks(base_chunks.as_ref(), addition);
    apply_advanced_chunks(session_id, merged, addition)
}

fn apply_exact_move(
    session_id: u64,
    delta_x: i32,
    delta_z: i32,
    fallback: ChunkSelection,
) -> ChunkSelection {
    let base_chunks = advanced_selection_registry()
        .lock()
        .ok()
        .and_then(|registry| {
            registry
                .sessions
                .get(&session_id)
                .map(|state| state.base_chunks.clone())
        });
    let Some(base_chunks) = base_chunks else {
        return translate_chunk_selection(fallback, delta_x, delta_z);
    };
    let translated = normalize_chunk_set(base_chunks.iter().copied().map(|chunk| ChunkPos {
        x: chunk.x.saturating_add(delta_x),
        z: chunk.z.saturating_add(delta_z),
        dimension: chunk.dimension,
    }));
    apply_advanced_chunks(
        session_id,
        translated,
        translate_chunk_selection(fallback, delta_x, delta_z),
    )
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ChunkSelection {
    pub(super) start: ChunkPos,
    pub(super) end: ChunkPos,
}

impl ChunkSelection {
    fn rectangular(start: ChunkPos, end: ChunkPos) -> Self {
        Self { start, end }
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
    advanced_session_id: Option<u64>,
}

impl RightSelectionDrag {
    pub(super) fn new(start_position: Point<Pixels>, start_chunk: ChunkPos) -> Self {
        if take_additive_right_selection_requested() {
            let session_id = begin_additive_selection(None);
            return Self::additive(start_position, start_chunk, session_id);
        }
        Self::with_intent(
            start_position,
            start_chunk,
            SelectionPointerButton::Right,
            RightSelectionIntent::NewSelection,
            None,
        )
    }

    fn additive(start_position: Point<Pixels>, start_chunk: ChunkPos, session_id: u64) -> Self {
        Self::with_intent(
            start_position,
            start_chunk,
            SelectionPointerButton::Right,
            RightSelectionIntent::AddSelection,
            Some(session_id),
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
            let session_id = begin_additive_selection(Some(selection));
            return Self::additive(start_position, start_chunk, session_id);
        }

        let exact_chunks = exact_selection_chunks(selection);
        let exact_chunk_slice = exact_chunks.as_ref().map(|chunks| chunks.as_slice());
        let irregular = exact_chunk_slice
            .is_some_and(|chunks| !selection_chunks_are_rectangular(selection, Some(chunks)));
        let target = if irregular {
            if selection_contains_chunk(selection, exact_chunk_slice, start_chunk) {
                ExistingSelectionTarget::Inside
            } else {
                ExistingSelectionTarget::Outside
            }
        } else {
            target
        };
        let (intent, advanced_session_id) = match target {
            ExistingSelectionTarget::Inside => match button {
                SelectionPointerButton::Left if irregular => {
                    let session_id = exact_chunks.map(|chunks| begin_exact_move(selection, chunks));
                    (RightSelectionIntent::MoveExact(selection), session_id)
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
        Self::with_intent(
            start_position,
            start_chunk,
            button,
            intent,
            advanced_session_id,
        )
    }

    fn with_intent(
        start_position: Point<Pixels>,
        start_chunk: ChunkPos,
        button: SelectionPointerButton,
        intent: RightSelectionIntent,
        advanced_session_id: Option<u64>,
    ) -> Self {
        Self {
            start_position,
            start_chunk,
            current_chunk: start_chunk,
            moved: false,
            button,
            intent,
            advanced_session_id,
        }
    }

    pub(super) fn selection(self) -> ChunkSelection {
        match self.intent {
            RightSelectionIntent::NewSelection => {
                ChunkSelection::rectangular(self.start_chunk, self.current_chunk)
            }
            RightSelectionIntent::AddSelection => self.advanced_session_id.map_or_else(
                || ChunkSelection::rectangular(self.start_chunk, self.current_chunk),
                |session_id| {
                    apply_additive_drag(
                        session_id,
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
            RightSelectionIntent::MoveExact(selection) => self.advanced_session_id.map_or_else(
                || {
                    translate_chunk_selection(
                        selection,
                        self.current_chunk.x.saturating_sub(self.start_chunk.x),
                        self.current_chunk.z.saturating_sub(self.start_chunk.z),
                    )
                },
                |session_id| {
                    apply_exact_move(
                        session_id,
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
        RightSelectionIntent::Move(_) | RightSelectionIntent::MoveExact(_) => {
            if moved {
                RightSelectionReleaseAction::ApplySelection
            } else {
                RightSelectionReleaseAction::KeepSelection
            }
        }
        RightSelectionIntent::Resize { .. } => {
            if moved {
                RightSelectionReleaseAction::ApplySelection
            } else {
                RightSelectionReleaseAction::KeepSelection
            }
        }
        RightSelectionIntent::OpenMenu(_) => {
            if button == SelectionPointerButton::Right && !moved {
                RightSelectionReleaseAction::OpenMenu
            } else {
                RightSelectionReleaseAction::KeepSelection
            }
        }
        RightSelectionIntent::NewSelection => {
            RightSelectionReleaseAction::ApplySelectionAndOpenMenu
        }
        RightSelectionIntent::AddSelection => RightSelectionReleaseAction::ApplySelection,
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
    let dimension = selection.bounds().dimension;
    ChunkSelection::rectangular(
        ChunkPos {
            x: selection.start.x.saturating_add(delta_x),
            z: selection.start.z.saturating_add(delta_z),
            dimension,
        },
        ChunkPos {
            x: selection.end.x.saturating_add(delta_x),
            z: selection.end.z.saturating_add(delta_z),
            dimension,
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
        ChunkSelection { start, end }
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
        let session_id = begin_additive_selection(Some(horizontal));
        let mut drag = RightSelectionDrag::additive(Point::default(), chunk(0, -1), session_id);
        drag.current_chunk = chunk(0, 1);
        let bounds = drag.selection();
        let exact = exact_selection_chunks(bounds).expect("exact selection chunks");

        assert_eq!(exact.len(), 5);
        assert_eq!(bounds.chunk_count(), 5);
        assert!(!selection_chunks_are_rectangular(bounds, Some(&exact)));
        assert!(selection_contains_chunk(bounds, Some(&exact), chunk(0, 0)));
        assert!(!selection_contains_chunk(bounds, Some(&exact), chunk(1, 1)));
        assert_ne!(bounds.start.dimension, bounds.end.dimension);
        assert_eq!(bounds.bounds().dimension, Dimension::Overworld);
    }

    #[test]
    fn additive_selection_preserves_base_before_pointer_moves() {
        let base = selection(chunk(-1, 0), chunk(1, 0));
        let session_id = begin_additive_selection(Some(base));
        let drag = RightSelectionDrag::additive(Point::default(), chunk(0, 0), session_id);
        let inherited = drag.selection();
        let exact = exact_selection_chunks(inherited).expect("inherited exact chunks");

        assert_eq!(exact.len(), 3);
        assert_eq!(inherited.bounds(), base.bounds());
    }

    #[test]
    fn irregular_selection_move_keeps_shape() {
        let horizontal = selection(chunk(-1, 0), chunk(1, 0));
        let add_session_id = begin_additive_selection(Some(horizontal));
        let mut add =
            RightSelectionDrag::additive(Point::default(), chunk(0, -1), add_session_id);
        add.current_chunk = chunk(0, 1);
        let selection = add.selection();
        let exact = exact_selection_chunks(selection).expect("exact selection chunks");
        let move_session_id = begin_exact_move(selection, exact);
        let mut drag = RightSelectionDrag::with_intent(
            Point::default(),
            chunk(0, 0),
            SelectionPointerButton::Left,
            RightSelectionIntent::MoveExact(selection),
            Some(move_session_id),
        );
        let zero_delta = drag.selection();
        let zero_delta_chunks = exact_selection_chunks(zero_delta).expect("zero delta exact chunks");
        assert_eq!(zero_delta_chunks.len(), 5);

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
    fn independent_sessions_use_distinct_selection_tokens() {
        let first_session =
            begin_additive_selection(Some(selection(chunk(0, 0), chunk(0, 0))));
        let mut first = RightSelectionDrag::additive(Point::default(), chunk(1, 0), first_session);
        first.current_chunk = chunk(1, 1);
        let first_selection = first.selection();

        let second_session =
            begin_additive_selection(Some(selection(chunk(0, 0), chunk(0, 0))));
        let mut second = RightSelectionDrag::additive(Point::default(), chunk(1, 0), second_session);
        second.current_chunk = chunk(1, 1);
        let second_selection = second.selection();

        assert_ne!(first_selection, second_selection);
        assert!(exact_selection_chunks(first_selection).is_some());
        assert!(exact_selection_chunks(second_selection).is_some());
        assert_eq!(first_selection.bounds(), second_selection.bounds());
    }

    #[test]
    fn selection_identity_changes_when_shape_changes_inside_same_bounds() {
        let base = selection(chunk(-1, 0), chunk(1, 0));
        let session_id = begin_additive_selection(Some(base));
        let mut drag = RightSelectionDrag::additive(Point::default(), chunk(0, -1), session_id);
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
    fn plain_struct_literal_remains_compatible() {
        let start = chunk(3, 4);
        let end = chunk(8, 9);
        let selection = ChunkSelection { start, end };
        assert_eq!(selection.start, start);
        assert_eq!(selection.end, end);
        assert!(exact_selection_chunks(selection).is_none());
    }

    #[test]
    fn additive_right_selection_does_not_open_context_menu() {
        let session_id = begin_additive_selection(None);
        let drag = RightSelectionDrag::additive(Point::default(), chunk(0, 0), session_id);
        assert_eq!(
            right_selection_release_action(drag.button, drag.intent, drag.moved),
            RightSelectionReleaseAction::ApplySelection
        );
    }
}
