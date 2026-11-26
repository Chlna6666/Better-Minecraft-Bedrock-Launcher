//! Frames scene bounds, samples an animated scene, and raycasts the prepared pose.

use gpui_3d::{
    AnimationScratch, Keyframe, Material, Mesh, Node, OrbitCamera, PreparedScene, Projection,
    RaycastScratch, Scene, SceneView, SceneViewRaycastScratch, Transform, TransformTrack, Vec2,
    Vec3, Vec3Track, Vertex,
};
use std::{error::Error, sync::Arc, time::Duration};

fn main() -> Result<(), Box<dyn Error>> {
    let mut scene = Scene::new();
    let cube_mesh = Arc::new(Mesh::cube());
    let mut updated_vertices = cube_mesh.vertices().to_vec();
    for vertex in &mut updated_vertices {
        vertex.position.z += 0.2;
    }
    let updated_cube = cube_mesh.with_vertices(updated_vertices)?;
    println!(
        "updated cube bounds: {:?}; source bounds: {:?}",
        updated_cube.bounds(),
        cube_mesh.bounds()
    );
    let cube_material = Arc::new(Material::new());
    let cube = scene.insert(
        None,
        Node::new()
            .with_mesh(cube_mesh.clone())
            .with_materials([cube_material.clone()]),
    )?;
    scene.insert(
        None,
        Node::new()
            .with_mesh(cube_mesh)
            .with_materials([cube_material])
            .with_transform(Transform {
                translation: Vec3::new(-1.4, 0.0, 0.0),
                ..Transform::IDENTITY
            }),
    )?;
    let imported_triangle = Mesh::new(
        [
            Vertex {
                position: Vec3::new(2.2, -0.2, 0.0),
                normal: Vec3::ZERO,
                uv: Vec2::ZERO,
                color: [1.0; 4],
            },
            Vertex {
                position: Vec3::new(2.8, -0.2, 0.0),
                normal: Vec3::ZERO,
                uv: Vec2::new(1.0, 0.0),
                color: [1.0; 4],
            },
            Vertex {
                position: Vec3::new(2.2, 0.4, 0.0),
                normal: Vec3::ZERO,
                uv: Vec2::new(0.0, 1.0),
                color: [1.0; 4],
            },
        ],
        [0, 1, 2],
    )?
    .generate_normals()?
    .generate_tangents()?
    .into_parts()
    .0;
    scene.insert(None, Node::new().with_mesh(Arc::new(imported_triangle)))?;
    let track = TransformTrack::new(cube).with_translation(Vec3Track::new([
        Keyframe::new(Duration::ZERO, Vec3::ZERO),
        Keyframe::new(Duration::from_secs(2), Vec3::new(2.0, 0.0, 0.0)),
    ])?);

    let authored_bounds = scene
        .bounds()?
        .ok_or("example scene has no authored mesh bounds")?;
    let sample_time = Duration::from_secs(1);
    let mut animation_scratch = AnimationScratch::new();
    let framing_bounds = {
        let evaluated = scene.evaluate_with(
            std::slice::from_ref(&track),
            sample_time,
            &mut animation_scratch,
        )?;
        evaluated
            .bounds()?
            .ok_or("example scene has no mesh bounds")?
    };
    println!("authored bounds: {authored_bounds:?}; sampled bounds: {framing_bounds:?}");
    let camera_controls = OrbitCamera::new(
        Vec3::new(1.0, 0.0, 0.0),
        0.0,
        0.0,
        4.0,
        Projection::Orthographic {
            height: 4.0,
            near: 0.1,
            far: 20.0,
        },
    )?
    .fit_bounds(framing_bounds, 4.0 / 3.0, 1.1)?
    .orbit(Vec2::new(0.12, 0.06))?
    .pan(Vec2::new(-0.1, 0.05))?
    .zoom(0.95)?;
    let camera = camera_controls.camera();
    let ray = camera.ray(Vec2::ZERO, 4.0 / 3.0)?;
    let mut query_scratch = RaycastScratch::new();
    for time in [Duration::ZERO, sample_time] {
        let evaluated =
            scene.evaluate_with(std::slice::from_ref(&track), time, &mut animation_scratch)?;
        let prepared = PreparedScene::new_evaluated(&evaluated, camera, 4.0 / 3.0)?;
        let hit = evaluated.raycast(ray, &mut query_scratch)?.is_some();
        println!(
            "sample {time:?}: {} visible draw(s), {} draw batch(es), ray hit: {hit}",
            prepared.draws.len(),
            prepared.draw_batches().count(),
        );
    }
    println!(
        "authored translation remains: {:?}",
        scene.node(cube).map(|node| node.transform.translation)
    );

    // Scene-view raycasts use the same sampled pose as GPUI rendering.
    let scene_view =
        SceneView::new(Arc::new(scene), camera).with_animation([track], sample_time)?;
    let mut raycast_scratch = SceneViewRaycastScratch::new();
    let scene_view_hit = scene_view.raycast(
        Vec2::new(320.0, 240.0),
        Vec2::new(640.0, 480.0),
        &mut raycast_scratch,
    )?;

    println!("scene-view pixel raycast hit: {}", scene_view_hit.is_some());
    Ok(())
}
