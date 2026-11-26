//! CPU viewport picking and projection tests.

#![allow(
    clippy::cast_precision_loss,
    reason = "Test viewports convert small pixel extents to f32 centers; exact rounding is irrelevant to the assertions."
)]

use gpui_3d::{
    Camera, Keyframe, Light, Material, Mesh, Node, NodeId, PickError, RaycastScratch, Scene,
    Transform, TransformTrack, Vec2, Vec3, Vec3Track, pick, pick_evaluated, project,
};
use std::sync::Arc;
use std::time::Duration;

const VIEWPORT: (u32, u32) = (128, 96);

fn camera() -> Camera {
    Camera::perspective(
        Vec3::new(0.0, 0.0, 5.0),
        Vec3::ZERO,
        Vec3::Y,
        45.0_f32.to_radians(),
        0.1,
        100.0,
    )
}

fn cube_scene() -> (Scene, NodeId) {
    let mut scene = Scene::new();
    let mut material = Material::new();
    material.base_color = [0.8, 0.4, 0.2, 1.0];
    let node = scene
        .insert(
            None,
            Node::new()
                .with_mesh(Arc::new(Mesh::cube()))
                .with_materials([Arc::new(material.validated().expect("material"))])
                .with_transform(Transform {
                    scale: Vec3::new(1.6, 1.6, 1.6),
                    ..Transform::default()
                }),
        )
        .expect("cube node");
    scene
        .insert(
            None,
            Node::new().with_light(Light::Directional {
                direction: Vec3::new(-0.3, -0.7, -0.5),
                color: [1.0, 1.0, 1.0],
                intensity: 2.0,
            }),
        )
        .expect("light");
    (scene, node)
}

#[test]
fn center_pick_hits_the_cube_and_projects_back_to_center() {
    let (scene, expected_node) = cube_scene();
    let mut scratch = RaycastScratch::new();
    let pixel = (VIEWPORT.0 as f32 / 2.0, VIEWPORT.1 as f32 / 2.0);
    let hit = pick(&scene, camera(), VIEWPORT, pixel, &mut scratch)
        .expect("pick")
        .expect("center ray hits the cube");
    assert_eq!(hit.node, expected_node);
    assert!(hit.distance > 3.5 && hit.distance < 4.6, "{hit:?}");
    assert!((hit.normal.length() - 1.0).abs() < 1.0e-3);
    assert!(hit.uv.x.is_finite() && hit.uv.y.is_finite());
    let (x, y, depth) = project(camera(), VIEWPORT, hit.position)
        .expect("project")
        .expect("hit position is visible");
    assert!((x - pixel.0).abs() < 1.0, "projected x {x} vs {pixel:?}");
    assert!((y - pixel.1).abs() < 1.0, "projected y {y} vs {pixel:?}");
    assert!(depth > 0.0 && depth < 1.0, "depth {depth}");
}

#[test]
fn off_center_pixels_miss_and_behind_camera_projection_is_none() {
    let (scene, _) = cube_scene();
    let mut scratch = RaycastScratch::new();
    assert!(
        pick(&scene, camera(), VIEWPORT, (0.5, 0.5), &mut scratch)
            .expect("pick")
            .is_none()
    );
    assert!(
        project(camera(), VIEWPORT, Vec3::new(0.0, 0.0, 10.0))
            .expect("project")
            .is_none()
    );
}

#[test]
fn animated_pick_uses_the_evaluated_pose() {
    let (scene, node) = cube_scene();
    let track = TransformTrack::new(node).with_translation(
        Vec3Track::new([
            Keyframe::new(Duration::ZERO, Vec3::ZERO),
            Keyframe::new(Duration::from_secs(1), Vec3::new(0.0, 0.0, 0.6)),
        ])
        .expect("translation track"),
    );
    let evaluated = scene
        .evaluate(&[track], Duration::from_millis(500))
        .expect("evaluate");
    let mut scratch = RaycastScratch::new();
    let hit = pick_evaluated(
        &evaluated,
        camera(),
        VIEWPORT,
        (VIEWPORT.0 as f32 / 2.0, VIEWPORT.1 as f32 / 2.0),
        &mut scratch,
    )
    .expect("animated pick")
    .expect("center ray still hits the moved cube");
    assert_eq!(hit.node, node);
}

#[test]
fn invalid_viewport_is_rejected() {
    let (scene, _) = cube_scene();
    let mut scratch = RaycastScratch::new();
    let error =
        pick(&scene, camera(), (0, 96), (1.0, 1.0), &mut scratch).expect_err("zero-width viewport");
    assert!(matches!(error, PickError::Camera(_)));
    assert!(
        project(camera(), (0, 96), Vec3::ZERO)
            .expect("project")
            .is_none()
    );
}

#[test]
fn camera_ray_matches_pick_helper_convention() {
    let (scene, expected_node) = cube_scene();
    let camera = camera();
    let aspect = VIEWPORT.0 as f32 / VIEWPORT.1 as f32;
    let ray = camera.ray(Vec2::new(0.0, 0.0), aspect).expect("center ray");
    let mut scratch = RaycastScratch::new();
    let hit = scene
        .raycast(ray, &mut scratch)
        .expect("raycast")
        .expect("hit");
    assert_eq!(hit.node, expected_node);
}
