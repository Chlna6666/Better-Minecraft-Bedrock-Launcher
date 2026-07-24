use super::*;
use crate::{
    AtlasTextureId, AtlasTile, Bounds, ContentMask, DevicePixels, Edges, Hsla, ScaledPixels,
    WgslShaderSource, bounds, point, px, size,
};
use std::sync::{Arc, OnceLock};

const TEST_GPU_MESH_3D_SHADER_SOURCE: &str = r#"
struct GlobalParams {
    viewport_size: vec2<f32>,
    premultiplied_alpha: u32,
    pad: u32,
};

struct MeshParams {
    bounds_origin: vec2<f32>,
    bounds_size: vec2<f32>,
    content_mask_origin: vec2<f32>,
    content_mask_size: vec2<f32>,
    view_proj_model: mat4x4<f32>,
};

struct MeshVertex {
    position_x: f32,
    position_y: f32,
    position_z: f32,
    color_r: f32,
    color_g: f32,
    color_b: f32,
    color_a: f32,
};

struct MeshOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: GlobalParams;
@group(0) @binding(20) var<storage, read> mesh_params: array<MeshParams>;
@group(0) @binding(21) var<storage, read> mesh_vertices: array<MeshVertex>;

@vertex
fn vs_test_mesh(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> MeshOut {
    let vertex = mesh_vertices[vertex_index];
    let params = mesh_params[instance_index];
    let viewport = max(globals.viewport_size, vec2<f32>(1.0));
    let position = params.bounds_origin + params.bounds_size * vec2<f32>(0.5, 0.5);
    let device = position / viewport * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    var out: MeshOut;
    out.position = vec4<f32>(device, 0.0, 1.0);
    out.color = vec4<f32>(vertex.color_r, vertex.color_g, vertex.color_b, vertex.color_a);
    return out;
}

@fragment
fn fs_test_mesh(input: MeshOut) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

fn test_gpu_mesh_3d_shader() -> Arc<GpuMesh3dShader> {
    static SHADER: OnceLock<Arc<GpuMesh3dShader>> = OnceLock::new();
    if let Some(shader) = SHADER.get() {
        return shader.clone();
    }
    let source =
        WgslShaderSource::from_source("test-gpu-mesh-3d-shader", TEST_GPU_MESH_3D_SHADER_SOURCE)
            .expect("test shader should validate");
    let _ = SHADER.set(Arc::new(GpuMesh3dShader::new(
        Arc::new(source),
        "vs_test_mesh",
        "fs_test_mesh",
    )));
    SHADER
        .get()
        .expect("test shader should be initialized")
        .clone()
}

fn monochrome_sprite(order: DrawOrder, pad: u32) -> MonochromeSprite {
    MonochromeSprite {
        order,
        pad,
        animation_id: None,
        bounds: bounds(
            point(ScaledPixels(0.0), ScaledPixels(0.0)),
            size(ScaledPixels(1.0), ScaledPixels(1.0)),
        ),
        content_mask: ContentMask {
            bounds: bounds(
                point(ScaledPixels(0.0), ScaledPixels(0.0)),
                size(ScaledPixels(10.0), ScaledPixels(10.0)),
            ),
            ..Default::default()
        },
        color: Hsla::default(),
        tile: AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: crate::AtlasTextureKind::Monochrome,
            },
            tile_id: crate::TileId(0),
            padding: 1,
            bounds: bounds(
                point(DevicePixels(1), DevicePixels(1)),
                size(DevicePixels(1), DevicePixels(1)),
            ),
        },
        transformation: TransformationMatrix::unit(),
    }
}

#[test]
fn backdrop_blur_does_not_force_scene_full_redraw_fallback() {
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0))).scale(1.0);
    let mut scene = Scene::default();

    scene.insert_primitive(PaintBackdropBlur {
        order: 0,
        animation_id: None,
        bounds,
        content_mask: ContentMask {
            bounds,
            ..Default::default()
        },
        corner_radii: Default::default(),
        radius: ScaledPixels(8.0),
        downsample: 2,
        levels: 3,
        saturation: 1.0,
        tint: None,
    });

    assert!(scene.has_backdrop_blurs());
    assert_eq!(
        scene.backdrop_blur_bounds().collect::<Vec<_>>(),
        vec![bounds]
    );
    assert!(!scene.requires_full_redraw_fallback());
}

#[test]
fn monochrome_sprite_batches_split_by_sampling() {
    let mut scene = Scene::default();
    scene.insert_primitive(monochrome_sprite(0, MonochromeSpriteSampling::Glyph as u32));
    scene.insert_primitive(monochrome_sprite(
        0,
        MonochromeSpriteSampling::Linear as u32,
    ));
    scene.finish();

    let batches = scene.batches().collect::<Vec<_>>();
    let monochrome_batches = batches
        .iter()
        .filter(|batch| matches!(batch, PrimitiveBatch::MonochromeSprites { .. }))
        .count();

    assert_eq!(monochrome_batches, 2);
}

#[test]
fn scene_batches_use_draw_order_then_primitive_kind() {
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0))).scale(1.0);
    let content_mask = ContentMask {
        bounds,
        ..Default::default()
    };
    let mut scene = Scene::default();

    scene.push_layer(bounds);
    scene.insert_primitive(PaintGpuMesh3d {
        order: 0,
        bounds,
        content_mask: content_mask.clone(),
        mesh: Arc::new(GpuMesh3d::new(
            vec![GpuMesh3dVertex {
                position: [0.0, 0.0, 0.0],
                color: [1.0, 1.0, 1.0, 1.0],
            }],
            vec![0],
            GpuMesh3dDrawRanges {
                opaque: GpuMesh3dRange { start: 0, count: 1 },
                glass: GpuMesh3dRange::default(),
                water: GpuMesh3dRange::default(),
            },
            [0.0, 0.0, 0.0],
            1.0,
            1.0,
            test_gpu_mesh_3d_shader(),
        )),
        parameters: GpuMesh3dDrawParameters {
            view_projection_model: [[1.0, 0.0, 0.0, 0.0]; 4],
        },
    });
    scene.insert_primitive(Quad {
        bounds,
        content_mask,
        ..Quad::default()
    });
    scene.pop_layer();
    scene.finish();

    let batches = scene.batches().collect::<Vec<_>>();
    assert!(matches!(batches[0], PrimitiveBatch::Quads(_)));
    assert!(matches!(batches[1], PrimitiveBatch::GpuMeshes3d(_)));
}

#[test]
fn retained_prefix_replay_preserves_draw_orders() {
    let layer_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(40.0), px(40.0))).scale(1.0);
    let first_bounds = Bounds::new(point(px(2.0), px(2.0)), size(px(12.0), px(12.0))).scale(1.0);
    let second_bounds = Bounds::new(point(px(8.0), px(8.0)), size(px(12.0), px(12.0))).scale(1.0);
    let following_bounds =
        Bounds::new(point(px(10.0), px(10.0)), size(px(12.0), px(12.0))).scale(1.0);

    let mut previous = Scene::default();
    previous.insert_primitive(Quad {
        bounds: first_bounds,
        content_mask: ContentMask {
            bounds: layer_bounds,
            ..Default::default()
        },
        ..Quad::default()
    });
    previous.push_layer(layer_bounds);
    previous.insert_primitive(Quad {
        bounds: second_bounds,
        content_mask: ContentMask {
            bounds: layer_bounds,
            ..Default::default()
        },
        ..Quad::default()
    });
    previous.pop_layer();

    let replay_end = previous.len();
    let expected_replayed_orders = previous
        .paint_operations
        .iter()
        .filter_map(|operation| match operation {
            PaintOperation::Primitive(primitive) => Some(primitive.order()),
            PaintOperation::StartLayer(_) | PaintOperation::EndLayer => None,
        })
        .collect::<Vec<_>>();

    let mut replayed = Scene::default();
    replayed.replay(0..replay_end, &previous);
    let actual_replayed_orders = replayed
        .paint_operations
        .iter()
        .filter_map(|operation| match operation {
            PaintOperation::Primitive(primitive) => Some(primitive.order()),
            PaintOperation::StartLayer(_) | PaintOperation::EndLayer => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(actual_replayed_orders, expected_replayed_orders);

    let following = Quad {
        bounds: following_bounds,
        content_mask: ContentMask {
            bounds: layer_bounds,
            ..Default::default()
        },
        ..Quad::default()
    };
    previous.insert_primitive(following.clone());
    replayed.insert_primitive(following);

    assert_eq!(
        replayed.quads.last().map(|quad| quad.order),
        previous.quads.last().map(|quad| quad.order)
    );
}

#[test]
fn retained_suffix_replay_accepts_matching_rebuilt_prefix() {
    let content_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(40.0), px(40.0))).scale(1.0);
    let first = Quad {
        bounds: Bounds::new(point(px(2.0), px(2.0)), size(px(12.0), px(12.0))).scale(1.0),
        content_mask: ContentMask {
            bounds: content_bounds,
            ..Default::default()
        },
        ..Quad::default()
    };
    let second = Quad {
        bounds: Bounds::new(point(px(8.0), px(8.0)), size(px(12.0), px(12.0))).scale(1.0),
        content_mask: ContentMask {
            bounds: content_bounds,
            ..Default::default()
        },
        ..Quad::default()
    };

    let mut previous = Scene::default();
    previous.insert_primitive(first.clone());
    previous.insert_primitive(second);

    let mut replayed = Scene::default();
    replayed.insert_primitive(first);
    replayed.replay(1..previous.len(), &previous);

    assert!(!replayed.retained_prefix_invalid);
    assert_eq!(replayed.retained_prefix_verified_len, previous.len());
    assert_eq!(
        replayed
            .quads
            .iter()
            .map(|quad| quad.order)
            .collect::<Vec<_>>(),
        previous
            .quads
            .iter()
            .map(|quad| quad.order)
            .collect::<Vec<_>>()
    );
}

#[test]
fn retained_suffix_replay_recomputes_after_prefix_bounds_change() {
    let content_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(50.0), px(50.0))).scale(1.0);
    let first = Quad {
        bounds: Bounds::new(point(px(2.0), px(2.0)), size(px(12.0), px(12.0))).scale(1.0),
        content_mask: ContentMask {
            bounds: content_bounds,
            ..Default::default()
        },
        ..Quad::default()
    };
    let changed_first = Quad {
        bounds: Bounds::new(point(px(30.0), px(30.0)), size(px(12.0), px(12.0))).scale(1.0),
        ..first.clone()
    };
    let second = Quad {
        bounds: Bounds::new(point(px(8.0), px(8.0)), size(px(12.0), px(12.0))).scale(1.0),
        content_mask: ContentMask {
            bounds: content_bounds,
            ..Default::default()
        },
        ..Quad::default()
    };

    let mut previous = Scene::default();
    previous.insert_primitive(first);
    previous.insert_primitive(second);

    let mut replayed = Scene::default();
    replayed.insert_primitive(changed_first);
    replayed.replay(1..previous.len(), &previous);

    assert!(replayed.retained_prefix_invalid);
}

#[test]
fn prepared_quad_runs_split_solid_and_bordered_quads() {
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0))).scale(1.0);
    let content_mask = ContentMask {
        bounds,
        ..Default::default()
    };
    let mut scene = Scene::default();
    scene.insert_primitive(Quad {
        bounds,
        content_mask: content_mask.clone(),
        background: Hsla::white().into(),
        ..Quad::default()
    });
    scene.insert_primitive(Quad {
        bounds,
        content_mask,
        background: Hsla::white().into(),
        border_color: Hsla::black(),
        border_widths: Edges::all(ScaledPixels(1.0)),
        ..Quad::default()
    });

    scene.finish();

    let quad_runs = scene
        .prepared_batches()
        .iter()
        .filter_map(|batch| match batch {
            PreparedSceneBatch::Quads(run) => Some(run),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(quad_runs.len(), 2);
    assert!(quad_runs[0].is_solid);
    assert!(!quad_runs[1].is_solid);
    assert_eq!(quad_runs[0].range, 0..1);
    assert_eq!(quad_runs[1].range, 1..2);
}

#[test]
fn scene_batches_gpu_mesh_3d_in_draw_order() {
    let mesh = Arc::new(GpuMesh3d::new(
        vec![GpuMesh3dVertex {
            position: [0.0, 0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
        }],
        vec![0],
        GpuMesh3dDrawRanges {
            opaque: GpuMesh3dRange { start: 0, count: 1 },
            glass: GpuMesh3dRange::default(),
            water: GpuMesh3dRange::default(),
        },
        [0.0, 0.0, 0.0],
        1.0,
        1.0,
        test_gpu_mesh_3d_shader(),
    ));
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0))).scale(1.0);
    let content_mask = ContentMask {
        bounds,
        ..Default::default()
    };
    let parameters = GpuMesh3dDrawParameters {
        view_projection_model: [[1.0, 0.0, 0.0, 0.0]; 4],
    };
    let mut scene = Scene::default();

    scene.insert_primitive(Quad {
        bounds,
        content_mask: content_mask.clone(),
        ..Quad::default()
    });
    scene.insert_primitive(PaintGpuMesh3d {
        order: 0,
        bounds,
        content_mask,
        mesh: mesh.clone(),
        parameters,
    });
    scene.finish();

    let batches = scene.batches().collect::<Vec<_>>();
    assert!(matches!(batches[0], PrimitiveBatch::Quads(_)));
    let PrimitiveBatch::GpuMeshes3d(meshes) = batches[1] else {
        panic!("expected gpu mesh batch");
    };
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0].mesh.id, mesh.id);
    assert_eq!(meshes[0].parameters, parameters);
}

#[test]
fn gpu_mesh_3d_generation_is_stable_for_draw_parameter_changes() {
    let mesh = GpuMesh3d::new(
        vec![GpuMesh3dVertex {
            position: [1.0, 2.0, 3.0],
            color: [0.25, 0.5, 0.75, 1.0],
        }],
        vec![0],
        GpuMesh3dDrawRanges::default(),
        [0.0, 0.0, 0.0],
        1.0,
        1.0,
        test_gpu_mesh_3d_shader(),
    )
    .with_generation(42);
    let before_id = mesh.id;
    let before_generation = mesh.generation;
    let parameters_a = GpuMesh3dDrawParameters {
        view_projection_model: [[1.0, 0.0, 0.0, 0.0]; 4],
    };
    let parameters_b = GpuMesh3dDrawParameters {
        view_projection_model: [[2.0, 0.0, 0.0, 0.0]; 4],
    };

    assert_ne!(parameters_a, parameters_b);
    assert_eq!(mesh.id, before_id);
    assert_eq!(mesh.generation, before_generation);
}
