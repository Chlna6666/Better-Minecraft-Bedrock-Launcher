//! CPU tests for glTF 2.0 / GLB import.

#![allow(
    clippy::approx_constant,
    clippy::cast_possible_truncation,
    clippy::manual_is_multiple_of,
    clippy::unreadable_literal,
    clippy::useless_format,
    reason = "The fixtures build tiny GLB containers and animation values where exact bit patterns, not numeric styling, are what the assertions test."
)]

use gpui_3d::{GltfError, GltfLoadOptions, load_bytes};
use std::time::Duration;

fn json_chunk(json: &str) -> Vec<u8> {
    let mut bytes = json.as_bytes().to_vec();
    while bytes.len() % 4 != 0 {
        bytes.push(b' ');
    }
    bytes
}

fn bin_chunk(mut bin: Vec<u8>) -> Vec<u8> {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    bin
}

fn glb(json: &str, bin: Vec<u8>) -> Vec<u8> {
    let json = json_chunk(json);
    let bin = bin_chunk(bin);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x4654_6C67_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    let total = 12 + 8 + json.len() + 8 + bin.len();
    bytes.extend_from_slice(&(total as u32).to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x4E4F_534A_u32.to_le_bytes());
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x004E_4942_u32.to_le_bytes());
    bytes.extend_from_slice(&bin);
    bytes
}

fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

struct TriangleDocument {
    json: String,
    bin: Vec<u8>,
}

/// One triangle with position, normal, UV, and 32-bit indices plus an optional animation.
fn triangle_document(animation: bool) -> TriangleDocument {
    let mut bin = Vec::new();
    bin.extend_from_slice(&floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]));
    bin.extend_from_slice(&floats(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
    bin.extend_from_slice(&floats(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
    bin.extend_from_slice(&[0_u8, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]);
    let animation_json = if animation {
        let times_offset = bin.len();
        bin.extend_from_slice(&floats(&[0.0, 1.0]));
        let values_offset = bin.len();
        bin.extend_from_slice(&floats(&[
            0.0, 0.0, 0.0, 1.0, 0.7071068, 0.0, 0.0, 0.7071068,
        ]));
        format!(
            r#",
  "animations": [
    {{
      "name": "spin",
      "samplers": [{{ "input": 4, "output": 5, "interpolation": "LINEAR" }}],
      "channels": [{{ "sampler": 0, "target": {{ "node": 0, "path": "rotation" }} }}]
    }}
  ],
  "accessors_extra": []
  "#,
        )
        .replace(
            r#""accessors_extra": []"#,
            &format!(
                r#""accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5125, "count": 3, "type": "SCALAR" }},
    {{ "bufferView": 4, "componentType": 5126, "count": 2, "type": "SCALAR" }},
    {{ "bufferView": 5, "componentType": 5126, "count": 2, "type": "VEC4" }}
  ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": 36, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": 72, "byteLength": 24 }},
    {{ "buffer": 0, "byteOffset": 96, "byteLength": 12 }},
    {{ "buffer": 0, "byteOffset": {times_offset}, "byteLength": 8 }},
    {{ "buffer": 0, "byteOffset": {values_offset}, "byteLength": 32 }}
  ]"#
            ),
        )
    } else {
        r#",
  "accessors": [
    { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3" },
    { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" },
    { "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" },
    { "bufferView": 3, "componentType": 5125, "count": 3, "type": "SCALAR" }
  ],
  "bufferViews": [
    { "buffer": 0, "byteOffset": 0, "byteLength": 36 },
    { "buffer": 0, "byteOffset": 36, "byteLength": 36 },
    { "buffer": 0, "byteOffset": 72, "byteLength": 24 },
    { "buffer": 0, "byteOffset": 96, "byteLength": 12 }
  ]"#
        .to_string()
    };
    let byte_length = bin.len();
    let json = format!(
        r#"{{
  "asset": {{ "version": "2.0" }},
  "scene": 0,
  "scenes": [{{ "nodes": [0] }}],
  "nodes": [{{ "mesh": 0 }}],
  "meshes": [{{ "primitives": [{{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }}, "indices": 3 }}] }}],
  "buffers": [{{ "byteLength": {byte_length} }}]{animation_json}
}}"#
    );
    TriangleDocument { json, bin }
}

fn load(document: &TriangleDocument) -> Result<gpui_3d::GltfScene, GltfError> {
    load_bytes(
        &glb(&document.json, document.bin.clone()),
        &GltfLoadOptions::default(),
        None,
    )
}

#[test]
fn glb_triangle_imports_geometry_and_bounds() {
    let document = triangle_document(false);
    let imported = load(&document).expect("triangle GLB imports");
    assert_eq!(imported.statistics.triangles, 1);
    assert_eq!(imported.statistics.primitives, 1);
    assert_eq!(imported.statistics.nodes, 2);
    assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);
    let bounds = imported
        .scene
        .bounds()
        .expect("scene bounds")
        .expect("mesh bounds");
    assert!((bounds.min.x - 0.0).abs() < 1.0e-5);
    assert!((bounds.max.x - 1.0).abs() < 1.0e-5);
    assert!((bounds.max.y - 1.0).abs() < 1.0e-5);
}

#[test]
fn glb_animation_track_samples_between_keys() {
    let document = triangle_document(true);
    let imported = load(&document).expect("animated GLB imports");
    assert_eq!(imported.statistics.animations, 1);
    let clip = imported.animation("spin").expect("named clip");
    assert_eq!(clip.duration, Duration::from_secs(1));
    let sample = imported
        .scene
        .evaluate(&clip.tracks, Duration::from_millis(500))
        .expect("sample animation");
    let rotation = sample
        .local_transform(clip.tracks[0].node())
        .expect("target node transform")
        .rotation;
    let expected = gpui_3d::Quat::from_axis_angle(gpui_3d::Vec3::X, std::f32::consts::FRAC_PI_4)
        .expect("axis angle");
    assert!((rotation.x - expected.x).abs() < 1.0e-3, "{rotation:?}");
    assert!((rotation.w - expected.w).abs() < 1.0e-3, "{rotation:?}");
}

#[test]
fn truncated_glb_is_rejected() {
    let document = triangle_document(false);
    let mut bytes = glb(&document.json, document.bin.clone());
    bytes.truncate(bytes.len() - 8);
    let error = load_bytes(&bytes, &GltfLoadOptions::default(), None).expect_err("truncated");
    assert!(matches!(error, GltfError::Invalid(_)), "{error:?}");
}

#[test]
fn unsupported_required_extension_is_rejected() {
    let document = triangle_document(false);
    let json = document.json.replace(
        r#""asset": { "version": "2.0" },"#,
        r#""asset": { "version": "2.0" }, "extensionsRequired": ["KHR_draco_mesh_compression"],"#,
    );
    let bytes = glb(&json, document.bin);
    let error = load_bytes(&bytes, &GltfLoadOptions::default(), None).expect_err("extension");
    assert!(matches!(error, GltfError::Unsupported(_)), "{error:?}");
}

#[test]
fn out_of_range_index_is_rejected() {
    let document = triangle_document(false);
    let mut bin = document.bin.clone();
    bin[96..100].copy_from_slice(&9_u32.to_le_bytes());
    let bytes = glb(&document.json, bin);
    let error = load_bytes(&bytes, &GltfLoadOptions::default(), None).expect_err("index");
    assert!(matches!(error, GltfError::Invalid(_)), "{error:?}");
}

#[test]
fn node_cycle_is_rejected() {
    let document = triangle_document(false);
    let json = document.json.replace(
        r#""nodes": [{ "mesh": 0 }],"#,
        r#""nodes": [{ "mesh": 0, "children": [1] }, { "children": [0] }],"#,
    );
    let bytes = glb(&json, document.bin);
    let error = load_bytes(&bytes, &GltfLoadOptions::default(), None).expect_err("cycle");
    assert!(matches!(error, GltfError::Invalid(_)), "{error:?}");
}

#[test]
fn external_path_escape_is_rejected() {
    let document = triangle_document(false);
    let json = document.json.replace(
        r#""buffers": [{ "byteLength": "#,
        r#""buffers": [{ "uri": "../escape.bin", "byteLength": "#,
    );
    let bytes = glb(&json, document.bin);
    let base = std::env::temp_dir();
    let error = load_bytes(&bytes, &GltfLoadOptions::default(), Some(base.as_path()))
        .expect_err("path escape");
    assert!(matches!(error, GltfError::Invalid(_)), "{error:?}");
}

#[test]
fn file_limit_is_enforced_before_parsing() {
    let options = GltfLoadOptions {
        max_file_bytes: 8,
        ..GltfLoadOptions::default()
    };
    let error = load_bytes(&[0_u8; 64], &options, None).expect_err("file limit");
    assert!(matches!(error, GltfError::Limit(_)), "{error:?}");
}

#[test]
#[ignore = "requires downloaded test assets (run test_assets/download.ps1)"]
fn cached_damaged_helmet_imports_with_textures() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test_assets/cache/DamagedHelmet.glb");
    let imported = gpui_3d::load_path(&path, &GltfLoadOptions::default()).expect("helmet imports");
    assert_eq!(imported.statistics.primitives, 1);
    assert_eq!(imported.statistics.textures, 4);
    assert!(imported.statistics.triangles > 10_000);
    let bounds = imported
        .scene
        .bounds()
        .expect("bounds")
        .expect("mesh bounds");
    assert!(bounds.max.x > bounds.min.x);
}

#[test]
#[ignore = "requires downloaded test assets (run test_assets/download.ps1)"]
fn cached_animated_cube_imports_external_resources_and_animation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test_assets/cache/AnimatedCube/AnimatedCube.gltf");
    let imported = gpui_3d::load_path(&path, &GltfLoadOptions::default()).expect("cube imports");
    assert_eq!(imported.statistics.textures, 1);
    assert_eq!(imported.animations.len(), 1);
    assert_eq!(imported.animations[0].duration, Duration::from_secs(2));
}
