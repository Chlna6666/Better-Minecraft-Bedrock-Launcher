use super::layout::{
    IDE_DIVIDER_WIDTH, IDE_LEFT_DOCK_WIDTH, IDE_LEFT_STRIPE_WIDTH, IDE_SPLITTER_WIDTH,
    IDE_STATUS_BAR_HEIGHT, IDE_TOP_BAR_HEIGHT,
};
use bedrock_render::{ChunkPos, Dimension};
use gpui::SharedString;
use std::sync::Arc;

pub const RIGHT_PANEL_DEFAULT_WIDTH: f32 = 420.0;
pub const RIGHT_PANEL_MIN_WIDTH: f32 = 300.0;
pub const BOTTOM_PANEL_DEFAULT_HEIGHT: f32 = 260.0;
pub const BOTTOM_PANEL_MIN_HEIGHT: f32 = 170.0;
pub const MIN_CENTER_WIDTH: f32 = 360.0;
pub const MIN_CENTER_HEIGHT: f32 = 240.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapViewerBottomTab {
    ChunkTree,
    Players,
    Details,
    Diagnostics,
    History,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MapViewerRightPanel {
    #[default]
    Nbt,
    Preview3d,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockDrag {
    RightPanel,
    BottomPanel,
}

#[derive(Clone, Copy, Debug)]
pub struct DockDragState {
    pub drag: DockDrag,
    pub start_x: f32,
    pub start_y: f32,
    pub start_size: f32,
}

#[derive(Clone, Debug)]
pub struct MapViewerUiState {
    pub left_panel_open: bool,
    pub right_panel_open: bool,
    pub bottom_panel_open: bool,
    pub right_panel_width: f32,
    pub bottom_panel_height: f32,
    pub active_bottom_tab: MapViewerBottomTab,
    pub active_right_panel: MapViewerRightPanel,
    pub top_more_open: bool,
    pub context_more_open: bool,
    pub context_paste_open: bool,
    pub dock_drag: Option<DockDragState>,
}

impl Default for MapViewerUiState {
    fn default() -> Self {
        Self {
            left_panel_open: true,
            right_panel_open: false,
            bottom_panel_open: false,
            right_panel_width: RIGHT_PANEL_DEFAULT_WIDTH,
            bottom_panel_height: BOTTOM_PANEL_DEFAULT_HEIGHT,
            active_bottom_tab: MapViewerBottomTab::ChunkTree,
            active_right_panel: MapViewerRightPanel::Nbt,
            top_more_open: false,
            context_more_open: false,
            context_paste_open: false,
            dock_drag: None,
        }
    }
}

impl MapViewerUiState {
    pub fn clamp_sizes(&mut self, viewport_width: f32, viewport_height: f32) {
        self.right_panel_width = clamp_right_panel_width(self.right_panel_width, viewport_width);
        if self.left_panel_open
            && self.right_panel_open
            && viewport_width
                < IDE_LEFT_STRIPE_WIDTH
                    + IDE_DIVIDER_WIDTH
                    + IDE_LEFT_DOCK_WIDTH
                    + IDE_DIVIDER_WIDTH
                    + IDE_SPLITTER_WIDTH
                    + self.right_panel_width
                    + MIN_CENTER_WIDTH
        {
            self.left_panel_open = false;
        }
        self.bottom_panel_height =
            clamp_bottom_panel_height(self.bottom_panel_height, viewport_height);
    }

    pub fn set_right_panel_open(&mut self, open: bool) {
        self.right_panel_open = open;
        if open {
            self.right_panel_width = self.right_panel_width.max(RIGHT_PANEL_MIN_WIDTH);
        }
    }
}

#[derive(Clone, Debug)]
pub struct EditorDocument<T> {
    pub target: Option<T>,
    pub title: SharedString,
    pub text: SharedString,
    pub dirty: bool,
    pub loading: bool,
    pub saving: bool,
}

impl<T> Default for EditorDocument<T> {
    fn default() -> Self {
        Self {
            target: None,
            title: SharedString::from("未选择记录"),
            text: SharedString::from(""),
            dirty: false,
            loading: false,
            saving: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DbTreeNodeKind {
    Dimension(Dimension),
    Chunk(ChunkPos),
}

#[derive(Clone, Debug)]
pub struct DbTreeNode {
    pub id: SharedString,
    pub label: SharedString,
    pub description: SharedString,
    pub depth: usize,
    pub kind: DbTreeNodeKind,
    pub loading: bool,
    pub loaded: bool,
    pub expanded: bool,
    pub error: Option<SharedString>,
}

#[derive(Clone, Debug, Default)]
pub struct DbTreeSelection {
    pub node_id: Option<SharedString>,
    pub detail: Option<SharedString>,
}

#[derive(Clone, Debug, Default)]
pub struct DbTreeState {
    pub generation: u64,
    pub loading: bool,
    pub selected_tile: Option<(i32, i32)>,
    pub nodes: Arc<Vec<DbTreeNode>>,
    pub selection: DbTreeSelection,
    pub error: Option<SharedString>,
}

pub fn clamp_right_panel_width(width: f32, viewport_width: f32) -> f32 {
    let available_width = viewport_width
        - IDE_LEFT_STRIPE_WIDTH
        - IDE_DIVIDER_WIDTH
        - IDE_SPLITTER_WIDTH
        - MIN_CENTER_WIDTH;
    let max_width = (viewport_width * 0.45)
        .min(available_width)
        .max(RIGHT_PANEL_MIN_WIDTH);
    width.clamp(RIGHT_PANEL_MIN_WIDTH, max_width)
}

pub fn clamp_bottom_panel_height(height: f32, viewport_height: f32) -> f32 {
    let reserved_height = IDE_TOP_BAR_HEIGHT + IDE_STATUS_BAR_HEIGHT + IDE_SPLITTER_WIDTH;
    let max_height =
        (viewport_height - reserved_height - MIN_CENTER_HEIGHT).max(BOTTOM_PANEL_MIN_HEIGHT);
    height.clamp(BOTTOM_PANEL_MIN_HEIGHT, max_height)
}

pub fn chunk_tree_nodes_for_tile(
    dimension: Dimension,
    tile: (i32, i32),
    chunks: &[ChunkPos],
) -> Vec<DbTreeNode> {
    let mut nodes = Vec::new();
    nodes.push(DbTreeNode {
        id: SharedString::from(format!("dimension:{}", dimension.id())),
        label: SharedString::from(format!("Dimension {}", dimension.id())),
        description: SharedString::from(format!("Tile {}, {}", tile.0, tile.1)),
        depth: 0,
        kind: DbTreeNodeKind::Dimension(dimension),
        loading: false,
        loaded: true,
        expanded: true,
        error: None,
    });

    for chunk in chunks.iter().copied() {
        nodes.push(DbTreeNode {
            id: SharedString::from(format!(
                "chunk:{}:{}:{}",
                chunk.dimension.id(),
                chunk.x,
                chunk.z
            )),
            label: SharedString::from(format!("Chunk {}, {}", chunk.x, chunk.z)),
            description: SharedString::from("点击查看区块详情"),
            depth: 1,
            kind: DbTreeNodeKind::Chunk(chunk),
            loading: false,
            loaded: false,
            expanded: false,
            error: None,
        });
    }

    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_sizes_keep_center_usable() {
        assert_eq!(clamp_right_panel_width(900.0, 1000.0), 450.0);
        assert_eq!(clamp_right_panel_width(100.0, 1000.0), 300.0);
        assert_eq!(clamp_bottom_panel_height(800.0, 900.0), 566.0);
        assert_eq!(clamp_bottom_panel_height(80.0, 900.0), 170.0);
    }

    #[test]
    fn narrow_window_collapses_left_dock_when_right_dock_opens() {
        let mut state = MapViewerUiState {
            left_panel_open: true,
            right_panel_open: true,
            ..MapViewerUiState::default()
        };

        state.clamp_sizes(920.0, 720.0);

        assert!(!state.left_panel_open);
        let reserved = IDE_LEFT_STRIPE_WIDTH
            + IDE_DIVIDER_WIDTH
            + IDE_SPLITTER_WIDTH
            + state.right_panel_width;
        assert!(920.0 - reserved >= MIN_CENTER_WIDTH);
    }

    #[test]
    fn chunk_tree_nodes_only_show_dimension_and_chunks() {
        let chunk = ChunkPos {
            x: 1,
            z: -2,
            dimension: Dimension::Overworld,
        };
        let nodes = chunk_tree_nodes_for_tile(Dimension::Overworld, (0, 0), &[chunk]);
        assert_eq!(nodes.len(), 2);
        assert!(matches!(nodes[0].kind, DbTreeNodeKind::Dimension(_)));
        assert!(matches!(nodes[1].kind, DbTreeNodeKind::Chunk(_)));
        assert_eq!(nodes[1].depth, 1);
    }
}
