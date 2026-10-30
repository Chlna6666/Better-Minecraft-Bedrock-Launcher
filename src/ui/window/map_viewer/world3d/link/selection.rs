//! Dimension-scoped selection exchange and explicit navigation back to the map.
use super::*;

fn can_receive_selection(
    current: BwDimension,
    source: BwDimension,
    selection: Option<World3dSelection>,
    writing: bool,
) -> bool {
    if selection.is_some_and(|selection| selection.dimension != source) {
        return false;
    }
    current == source || (selection.is_some() && !writing)
}

fn selection_chunks(selection: World3dSelection) -> ChunkSelection {
    ChunkSelection {
        start: BwChunkPos {
            x: selection.min[0].div_euclid(16),
            z: selection.min[2].div_euclid(16),
            dimension: selection.dimension,
        },
        end: BwChunkPos {
            x: selection.max[0].div_euclid(16),
            z: selection.max[2].div_euclid(16),
            dimension: selection.dimension,
        },
    }
}

impl MapViewerWindowView {
    /// Locates a 3D target without applying coordinates to a different dimension.
    pub(crate) fn locate_world3d_in_map(
        &mut self,
        dimension: BwDimension,
        position: [i32; 3],
        selection: Option<World3dSelection>,
        cx: &mut Context<Self>,
    ) -> bool {
        if dimension != self.dimension && self.map_write_busy() {
            self.status = "地图正在写入，暂不能跨维度定位；请等待完成".into();
            cx.notify();
            return false;
        }
        self.set_dimension(dimension, cx);
        self.step_y(position[1].saturating_sub(self.y_layer), cx);
        if let Some(selection) = selection.filter(|selection| selection.dimension == dimension) {
            self.set_world3d_selection_from_editor(dimension, Some(selection), cx);
        }
        self.center_viewport_on_block(position[0], position[2], cx);
        self.status = format!(
            "已从 3D 定位 X {} · Y {} · Z {}",
            position[0], self.y_layer, position[2]
        )
        .into();
        let window = self.owner_window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        });
        cx.notify();
        true
    }

    pub(crate) fn set_world3d_selection_from_editor(
        &mut self,
        dimension: BwDimension,
        selection: Option<World3dSelection>,
        cx: &mut Context<Self>,
    ) {
        if !can_receive_selection(self.dimension, dimension, selection, self.map_write_busy()) {
            return;
        }
        if dimension != self.dimension {
            self.set_dimension(dimension, cx);
        }
        self.professional.volume_selection = selection;
        self.professional.selection = selection.map(selection_chunks);
        self.professional.selection_stats = None;
        if self.preview_3d.source == Preview3dSource::Selection {
            self.invalidate_preview_3d_mesh();
        }
        self.invalidate_professional_overlay_for_viewport_change();
        self.professional.pending_overlay_refresh = true;
        self.sync_volume_selection_to_3d(cx);
        self.refresh_professional_render_caches(cx);
        let colors = self.theme_colors(cx);
        self.sync_canvas_snapshot(colors, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{BwDimension, World3dSelection, can_receive_selection, selection_chunks};

    #[test]
    fn clearing_a_selection_only_affects_its_source_dimension() {
        assert!(!can_receive_selection(
            BwDimension::Nether,
            BwDimension::Overworld,
            None,
            false
        ));
        assert!(can_receive_selection(
            BwDimension::Nether,
            BwDimension::Nether,
            None,
            false
        ));
    }

    #[test]
    fn cross_dimension_selection_waits_for_world_write() {
        let selection = Some(World3dSelection::from_corners(
            BwDimension::Nether,
            [0, 1, 0],
            [1, 3, 1],
        ));
        assert!(!can_receive_selection(
            BwDimension::Overworld,
            BwDimension::Nether,
            selection,
            true
        ));
        assert!(can_receive_selection(
            BwDimension::Overworld,
            BwDimension::Nether,
            selection,
            false
        ));
        assert!(can_receive_selection(
            BwDimension::Nether,
            BwDimension::Nether,
            selection,
            true
        ));
        assert!(!can_receive_selection(
            BwDimension::Overworld,
            BwDimension::Overworld,
            selection,
            false
        ));
    }

    #[test]
    fn block_selection_projects_negative_chunk_edges_without_losing_height() {
        let volume =
            World3dSelection::from_corners(BwDimension::Overworld, [-17, -20, -1], [0, 9, 16]);
        let chunks = selection_chunks(volume);
        assert_eq!([chunks.start.x, chunks.start.z], [-2, -1]);
        assert_eq!([chunks.end.x, chunks.end.z], [0, 1]);
        assert_eq!([volume.min[1], volume.max[1]], [-20, 9]);
    }
}
