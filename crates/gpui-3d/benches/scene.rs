//! Criterion benchmarks for scene preparation and retained buffer reuse.
#![allow(
    missing_docs,
    reason = "Criterion generates benchmark entry points without rustdoc"
)]

use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use gpui_3d::{
    AnimationScratch, Camera, Keyframe, Mesh, Node, PreparedScene, Scene, Transform,
    TransformTrack, Vec3, Vec3Track,
};
use std::{sync::Arc, time::Duration};

#[allow(
    clippy::cast_precision_loss,
    reason = "benchmark scenes are capped at 10,000 nodes and use small exact grid coordinates"
)]
fn prepare(c: &mut Criterion) {
    let mut group = c.benchmark_group("scene/prepare");
    let camera = Camera::perspective(
        Vec3::new(0.0, 0.0, 24.0),
        Vec3::ZERO,
        Vec3::Y,
        1.0,
        0.1,
        200.0,
    );
    for node_count in [100_usize, 1_000, 10_000] {
        let scene = build_scene(node_count);
        let mut retained =
            PreparedScene::new(&scene, camera, 1.6).expect("benchmark camera and scene are valid");
        group.bench_with_input(
            BenchmarkId::new("rebuild", node_count),
            &node_count,
            |bencher, _| {
                bencher.iter(|| {
                    black_box(
                        PreparedScene::new(black_box(&scene), camera, 1.6)
                            .expect("benchmark camera is valid"),
                    )
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("retained", node_count),
            &node_count,
            |bencher, _| {
                bencher.iter(|| {
                    retained
                        .update(black_box(&scene), camera, 1.6)
                        .expect("benchmark camera is valid");
                    black_box(&retained);
                });
            },
        );
    }
    for node_count in [100_usize, 1_000] {
        let (scene, tracks) = build_animated_scene(node_count);
        let mut retained = PreparedScene::default();
        let mut animation_scratch = AnimationScratch::new();
        group.bench_with_input(
            BenchmarkId::new("animated-evaluate-update", node_count),
            &node_count,
            |bencher, _| {
                bencher.iter(|| {
                    let evaluated = black_box(&scene)
                        .evaluate_with(
                            black_box(&tracks),
                            Duration::from_millis(500),
                            &mut animation_scratch,
                        )
                        .expect("benchmark animation tracks target live scene nodes");
                    retained
                        .update_evaluated(&evaluated, camera, 1.6)
                        .expect("benchmark camera is valid");
                    black_box(retained.draws.len())
                });
            },
        );
    }
    group.finish();
}

fn bounds(c: &mut Criterion) {
    let mut group = c.benchmark_group("scene/bounds");
    for node_count in [100_usize, 1_000, 10_000] {
        let scene = build_scene(node_count);
        group.bench_with_input(
            BenchmarkId::new("authored", node_count),
            &node_count,
            |bencher, _| {
                bencher.iter(|| {
                    black_box(
                        black_box(&scene)
                            .bounds()
                            .expect("benchmark scene transforms are finite"),
                    )
                });
            },
        );

        let (animated_scene, tracks) = build_animated_scene(node_count);
        let evaluated = animated_scene
            .evaluate(&tracks, Duration::from_millis(500))
            .expect("benchmark tracks target live scene nodes");
        group.bench_with_input(
            BenchmarkId::new("evaluated", node_count),
            &node_count,
            |bencher, _| {
                bencher.iter(|| {
                    black_box(
                        black_box(&evaluated)
                            .bounds()
                            .expect("benchmark sampled transforms are finite"),
                    )
                });
            },
        );
    }
    group.finish();
}

fn batches(c: &mut Criterion) {
    let mut group = c.benchmark_group("scene/batching");
    let camera = Camera::orthographic(
        Vec3::new(0.0, 0.0, 120.0),
        Vec3::ZERO,
        Vec3::Y,
        160.0,
        0.1,
        300.0,
    );
    for node_count in [100_usize, 1_000, 10_000] {
        let scene = build_scene(node_count);
        let prepared = PreparedScene::new(&scene, camera, 1.6)
            .expect("benchmark camera contains repeated mesh instances");
        assert_eq!(prepared.draws.len(), node_count);
        assert_eq!(prepared.draw_batches().count(), 1);
        group.bench_with_input(
            BenchmarkId::new("compatible-draws", node_count),
            &node_count,
            |bencher, _| {
                bencher.iter(|| black_box(black_box(&prepared).draw_batches().count()));
            },
        );
    }
    group.finish();
}

#[allow(
    clippy::cast_precision_loss,
    reason = "benchmark scenes are capped at 10,000 nodes and use small exact grid coordinates"
)]
fn build_scene(node_count: usize) -> Scene {
    let mesh = Arc::new(Mesh::cube());
    let mut scene = Scene::new();
    for index in 0..node_count {
        let x = (index % 100) as f32 * 1.25 - 62.0;
        let y = ((index / 100) % 100) as f32 * 1.25 - 62.0;
        let z = -((index / 10_000) as f32);
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_transform(Transform {
                        translation: Vec3::new(x, y, z),
                        ..Transform::IDENTITY
                    }),
            )
            .expect("benchmark node handle space is sufficient");
    }
    scene
}

#[allow(
    clippy::cast_precision_loss,
    reason = "benchmark scenes are capped at 1,000 nodes and use small exact grid coordinates"
)]
fn build_animated_scene(node_count: usize) -> (Scene, Vec<TransformTrack>) {
    let mesh = Arc::new(Mesh::cube());
    let mut scene = Scene::new();
    let mut tracks = Vec::with_capacity(node_count);
    for index in 0..node_count {
        let x = (index % 100) as f32 * 1.25 - 62.0;
        let y = ((index / 100) % 100) as f32 * 1.25 - 62.0;
        let translation = Vec3::new(x, y, 0.0);
        let node = scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_transform(Transform {
                        translation,
                        ..Transform::IDENTITY
                    }),
            )
            .expect("benchmark node handle space is sufficient");
        let track = TransformTrack::new(node).with_translation(
            Vec3Track::new([
                Keyframe::new(Duration::ZERO, translation),
                Keyframe::new(
                    Duration::from_secs(1),
                    translation + Vec3::new(0.0, 0.25, 0.0),
                ),
            ])
            .expect("benchmark animation keys are valid"),
        );
        tracks.push(track);
    }
    (scene, tracks)
}

fn build_primitives(c: &mut Criterion) {
    let mut group = c.benchmark_group("mesh/build");
    group.bench_function("cube", |bencher| {
        bencher.iter(|| black_box(Mesh::cube()));
    });
    group.bench_function("uv_sphere/32x16", |bencher| {
        bencher.iter(|| {
            black_box(
                Mesh::uv_sphere(1.0, [32, 16]).expect("benchmark sphere dimensions are valid"),
            )
        });
    });
    group.bench_function("cylinder/32", |bencher| {
        bencher.iter(|| {
            black_box(
                Mesh::cylinder(1.0, 2.0, 32).expect("benchmark cylinder dimensions are valid"),
            )
        });
    });
    let tangent_source =
        Mesh::uv_sphere(1.0, [64, 32]).expect("benchmark sphere dimensions are valid");
    group.bench_function("vertex-snapshot/uv-sphere-64x32", |bencher| {
        bencher.iter_batched(
            || tangent_source.vertices().to_vec(),
            |vertices| {
                black_box(
                    tangent_source
                        .with_vertices(vertices)
                        .expect("benchmark vertex updates preserve mesh invariants"),
                )
            },
            BatchSize::LargeInput,
        );
    });
    group.bench_function("generate-tangents/uv-sphere-64x32", |bencher| {
        bencher.iter(|| {
            black_box(
                tangent_source
                    .generate_tangents()
                    .expect("benchmark sphere has valid tangent frames"),
            )
        });
    });
    group.finish();
}

criterion_group!(benches, prepare, bounds, batches, build_primitives);
criterion_main!(benches);
