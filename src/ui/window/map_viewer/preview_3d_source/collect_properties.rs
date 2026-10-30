//! Resolves immutable palette-entry properties once instead of once for every occupied voxel.

use super::{
    Arc, BlockModelRepository, BlockState, HashMap, Preview3dBlockClass,
    Preview3dBlockEntityModelState, Preview3dConnectionKind, Preview3dDetailShape,
    Preview3dMaterialName, Preview3dNeighborModel, Preview3dStairState, preview_3d_connection_kind,
    preview_3d_detail_shape_for_block, preview_3d_fluid_height,
    preview_3d_material_block_name_for_state, preview_3d_material_name_for_block,
    preview_3d_neighbor_model, preview_3d_normalized_block_name,
    preview_3d_resolved_detail_shape_for_block, preview_3d_stair_state,
};

pub(super) struct BlockPropertiesCache<'a> {
    models: Option<&'a BlockModelRepository>,
    entries: HashMap<usize, BlockProperties>,
}

impl<'a> BlockPropertiesCache<'a> {
    pub(super) fn new(models: Option<&'a BlockModelRepository>) -> Self {
        Self {
            models,
            entries: HashMap::default(),
        }
    }

    pub(super) fn resolve(
        &mut self,
        state: &BlockState,
        class: Preview3dBlockClass,
    ) -> &BlockProperties {
        // The cache is local to one immutable ChunkData traversal. Palette entries keep stable
        // addresses for that traversal, so no state hashing, cloning or persistent pointer is needed.
        self.entries
            .entry(std::ptr::from_ref(state) as usize)
            .or_insert_with(|| BlockProperties::new(state, class, self.models))
    }
}

pub(super) struct BlockProperties {
    pub(super) material: Preview3dMaterialName,
    pub(super) normalized_name: Arc<str>,
    pub(super) stair_state: Option<Preview3dStairState>,
    pub(super) connection_kind: Option<Preview3dConnectionKind>,
    pub(super) neighbor_model: Preview3dNeighborModel,
    pub(super) fluid_height: f32,
    shape: Option<Arc<Preview3dDetailShape>>,
    resolved_shape: bool,
}

impl BlockProperties {
    pub(super) fn new(
        state: &BlockState,
        class: Preview3dBlockClass,
        models: Option<&BlockModelRepository>,
    ) -> Self {
        let material_source = preview_3d_material_block_name_for_state(state, class);
        let resolved = models
            .and_then(|models| preview_3d_resolved_detail_shape_for_block(models, state, class));
        let resolved_shape = resolved.is_some();
        let shape = resolved
            .or_else(|| {
                (!matches!(
                    class,
                    Preview3dBlockClass::Water | Preview3dBlockClass::Lava
                ))
                .then(|| preview_3d_detail_shape_for_block(state, None))
                .flatten()
            })
            .map(Arc::new);
        Self {
            material: preview_3d_material_name_for_block(material_source.as_ref(), class),
            normalized_name: Arc::from(preview_3d_normalized_block_name(&state.name)),
            stair_state: preview_3d_stair_state(state),
            connection_kind: preview_3d_connection_kind(state),
            neighbor_model: if resolved_shape {
                Preview3dNeighborModel::None
            } else {
                preview_3d_neighbor_model(state)
            },
            fluid_height: preview_3d_fluid_height(state),
            shape,
            resolved_shape,
        }
    }

    pub(super) fn shape(
        &self,
        state: &BlockState,
        entity: Option<Preview3dBlockEntityModelState>,
    ) -> Option<Arc<Preview3dDetailShape>> {
        if self.resolved_shape || entity.is_none() {
            self.shape.clone()
        } else {
            // Paired chests depend on block-entity coordinates, not only the palette entry.
            preview_3d_detail_shape_for_block(state, entity).map(Arc::new)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn repeated_palette_entries_share_materials_and_detail_geometry() {
        let state = BlockState {
            name: "minecraft:oak_stairs".into(),
            states: BTreeMap::new(),
            version: None,
        };
        let mut cache = BlockPropertiesCache::new(None);
        let first = cache.resolve(&state, Preview3dBlockClass::DetailOpaque);
        let material = first.material.clone();
        let shape = first.shape(&state, None).unwrap();
        let second = cache.resolve(&state, Preview3dBlockClass::DetailOpaque);
        assert!(Arc::ptr_eq(&material, &second.material));
        assert!(Arc::ptr_eq(&shape, &second.shape(&state, None).unwrap()));
        assert_eq!(cache.entries.len(), 1);
    }
}
