//! Small interactive outlines kept separate from the streamed world scene.

use super::*;

impl World3dEditorView {
    pub(super) fn update_target_scene(&mut self) {
        let scene = outline_scene(
            self.selection,
            self.target.as_ref(),
            self.target_outline_mesh.as_ref(),
            &self.target_outline_material,
            self.render_origin,
        );
        if scene.is_empty() && self.target_scene_view.is_none() {
            return;
        }
        let scene = Arc::new(scene);
        let camera = self.camera.gpu_camera(self.render_origin);
        let view = match self.target_scene_view.as_ref() {
            Some(view) => view.with_scene(scene).with_camera(camera),
            None => SceneView::new(scene, camera),
        };
        self.target_scene_view = Some(Arc::new(view.with_voxel_shader()));
    }
}

fn outline_scene(
    selection: Option<World3dSelection>,
    target: Option<&World3dTargetBlock>,
    target_mesh: Option<&Arc<Mesh>>,
    material: &Arc<Material>,
    origin: [i32; 3],
) -> Scene {
    let mut scene = Scene::new();
    if let Some(selection) = selection
        && let Some(mesh) = selection_outline_mesh(selection)
    {
        insert_outline(&mut scene, Arc::new(mesh), material, selection.min, origin);
    }
    if let Some(target) = target
        && let Some(mesh) = target_mesh
    {
        insert_outline(&mut scene, mesh.clone(), material, target.position, origin);
    }
    scene
}

fn insert_outline(
    scene: &mut Scene,
    mesh: Arc<Mesh>,
    material: &Arc<Material>,
    position: [i32; 3],
    origin: [i32; 3],
) {
    let _ = scene.insert(
        None,
        Node::new()
            .with_mesh(mesh)
            .with_materials([material.clone()])
            .with_transform(Transform {
                translation: Vec3::new(
                    position[0].saturating_sub(origin[0]) as f32,
                    position[1].saturating_sub(origin[1]) as f32,
                    position[2].saturating_sub(origin[2]) as f32,
                ),
                ..Transform::IDENTITY
            }),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        AlphaMode, Arc, ChunkPos, Dimension, World3dSelection, World3dTargetBlock,
        block_outline_mesh, outline_scene, selection_material,
    };

    #[test]
    fn selection_outlines_survive_without_a_hovered_block() {
        let selection =
            World3dSelection::from_corners(Dimension::Overworld, [0, 64, 0], [5, 70, 5]);
        let material = selection_material(AlphaMode::Blend);
        assert_eq!(
            outline_scene(Some(selection), None, None, &material, [0; 3]).len(),
            1
        );
        assert!(outline_scene(None, None, None, &material, [0; 3]).is_empty());
    }

    #[test]
    fn target_and_selection_share_a_bounded_overlay() {
        let selection =
            World3dSelection::from_corners(Dimension::Overworld, [0, 64, 0], [5, 70, 5]);
        let target = World3dTargetBlock {
            dimension: Dimension::Overworld,
            position: [0, 64, 0],
            normal: [0, 1, 0],
            block_name: "minecraft:stone".into(),
            chunk: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            loaded: true,
            fluid: false,
        };
        let mesh = Arc::new(block_outline_mesh().unwrap());
        let material = selection_material(AlphaMode::Blend);
        assert_eq!(
            outline_scene(
                Some(selection),
                Some(&target),
                Some(&mesh),
                &material,
                [0; 3]
            )
            .len(),
            2
        );
        assert_eq!(
            outline_scene(None, Some(&target), Some(&mesh), &material, [0; 3]).len(),
            1
        );
    }
}
