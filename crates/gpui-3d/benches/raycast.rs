//! Criterion benchmarks for static mesh ray queries.
#![allow(
    missing_docs,
    reason = "Criterion generates benchmark entry points without rustdoc"
)]

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use gpui_3d::{Mesh, Node, Ray, RaycastScratch, Scene, Vec2, Vec3, Vertex};
use std::sync::Arc;

/// Measures BVH ray queries as mesh triangle count grows.
#[allow(
    clippy::cast_precision_loss,
    reason = "The fixed benchmark grid is capped at 16,384 triangles; all generated coordinates are at most 127 and exactly representable in f32."
)]
fn raycast(c: &mut Criterion) {
    let mut group = c.benchmark_group("scene/raycast");
    for (triangle_count, side) in [(64_u32, 8_u32), (1_024, 32), (16_384, 128)] {
        let mut vertices = Vec::with_capacity(triangle_count as usize * 3);
        let mut indices = Vec::with_capacity(triangle_count as usize * 3);
        for triangle in 0..triangle_count {
            let x = (triangle % side) as f32 * 2.0;
            let y = (triangle / side) as f32 * 2.0;
            let first =
                u32::try_from(vertices.len()).expect("benchmark vertex count fits draw indices");
            vertices.extend([
                vertex(x, y, -2.0),
                vertex(x + 1.0, y, -2.0),
                vertex(x + 0.5, y + 1.0, -2.0),
            ]);
            indices.extend([first, first + 1, first + 2]);
        }
        let mesh = Arc::new(Mesh::new(vertices, indices).expect("benchmark mesh is valid"));
        let mut scene = Scene::new();
        scene
            .insert(None, Node::new().with_mesh(mesh))
            .expect("benchmark scene has one valid root");
        let target = triangle_count / 2;
        let ray = Ray::new(
            Vec3::new(
                (target % side) as f32 * 2.0 + 0.5,
                (target / side) as f32 * 2.0 + 0.25,
                0.0,
            ),
            -Vec3::Z,
        )
        .expect("benchmark ray is valid");
        let mut scratch = RaycastScratch::new();

        group.bench_with_input(
            BenchmarkId::new("bvh", triangle_count),
            &triangle_count,
            |bencher, _| {
                bencher.iter(|| {
                    black_box(
                        scene
                            .raycast(black_box(ray), &mut scratch)
                            .expect("benchmark ray is valid"),
                    )
                });
            },
        );
    }
    group.finish();
}

/// Creates a benchmark vertex at the requested mesh-local position.
fn vertex(x: f32, y: f32, z: f32) -> Vertex {
    Vertex {
        position: Vec3::new(x, y, z),
        normal: Vec3::Z,
        uv: Vec2::new(x, y),
        color: [1.0; 4],
    }
}

criterion_group!(benches, raycast);
criterion_main!(benches);
