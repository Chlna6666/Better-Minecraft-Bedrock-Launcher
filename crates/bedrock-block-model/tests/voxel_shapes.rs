use bedrock_block_model::{BlockFace, VoxelShapeLibrary, vanilla_voxel_shapes};
use serde_json::json;

fn library(boxes: serde_json::Value) -> VoxelShapeLibrary {
    VoxelShapeLibrary::from_json(
        &serde_json::to_vec(&json!([{
            "format_version": "1.21.110",
            "root": "minecraft:voxel_shape",
            "description": { "identifier": "test:volume" },
            "minecraft:voxel_shape": { "boxes": boxes }
        }]))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn embedded_shapes_preserve_version_count_and_normalized_boxes() {
    let shapes = vanilla_voxel_shapes();
    assert_eq!(shapes.len(), 218);
    assert_eq!(
        shapes
            .iter()
            .map(|(_, shape)| shape.boxes().len())
            .sum::<usize>(),
        684
    );
    assert!(
        shapes
            .iter()
            .all(|(_, shape)| shape.format_version() == "1.21.110")
    );
    let slab = shapes.get("minecraft:slab_type_top").unwrap();
    assert_eq!(slab.boxes()[0].min(), [0.0, 0.5, 0.0]);
    assert_eq!(slab.boxes()[0].max(), [1.0; 3]);
    assert!(slab.boundary_coverage(BlockFace::Up).unwrap().is_full());
    assert!(slab.boundary_coverage(BlockFace::Down).unwrap().is_empty());
    assert!(shapes.get("minecraft:stone").is_none());
    assert!(shapes.get("slab_type_top").is_none());
}

#[test]
fn coverage_preserves_holes_and_distinguishes_axes() {
    let shapes = library(json!([
        { "min": [0, 0, 0], "max": [8, 16, 16] },
        { "min": [9, 0, 0], "max": [16, 16, 16] }
    ]));
    let shape = shapes.get("test:volume").unwrap();
    let top = shape.boundary_coverage(BlockFace::Up).unwrap();
    assert!(!top.is_full());
    assert!(top.contains(7, 8));
    assert!(!top.contains(8, 8));
    assert!(top.contains(9, 8));
    assert!(!top.contains(16, 8));
    assert!(shape.boundary_coverage(BlockFace::West).unwrap().is_full());
    assert!(shape.boundary_coverage(BlockFace::All).is_none());
}

#[test]
fn coverage_does_not_round_fractional_holes_or_external_touching_boxes() {
    let shapes = library(json!([
        { "min": [0, 0, 0], "max": [7.99, 16, 16] },
        { "min": [8.01, 0, 0], "max": [16, 16, 16] },
        { "min": [0, 16, 0], "max": [16, 20, 16] }
    ]));
    let top = shapes
        .get("test:volume")
        .unwrap()
        .boundary_coverage(BlockFace::Up)
        .unwrap();
    assert!(!top.contains(7, 8));
    assert!(!top.contains(8, 8));
    assert!(top.contains(6, 8));
}

#[test]
fn empty_shapes_are_valid_but_invalid_boxes_and_duplicates_are_rejected() {
    let shapes = library(json!([]));
    assert!(
        shapes
            .get("test:volume")
            .unwrap()
            .boundary_coverage(BlockFace::Up)
            .unwrap()
            .is_empty()
    );
    for bounds in [
        json!({"min": [0, 0, 0], "max": [0, 16, 16]}),
        json!({"min": [8, 0, 0], "max": [7, 16, 16]}),
    ] {
        let document = json!({
            "format_version": "1", "root": "minecraft:voxel_shape",
            "description": {"identifier": "test:volume"},
            "minecraft:voxel_shape": {"boxes": [bounds]}
        });
        assert!(
            VoxelShapeLibrary::from_json(&serde_json::to_vec(&json!([document])).unwrap()).is_err()
        );
    }
    let document = json!({
        "format_version": "1", "root": "minecraft:voxel_shape",
        "description": {"identifier": "test:volume"},
        "minecraft:voxel_shape": {"boxes": []}
    });
    assert!(
        VoxelShapeLibrary::from_json(&serde_json::to_vec(&json!([document, document])).unwrap())
            .is_err()
    );
    assert!(VoxelShapeLibrary::from_json(b"not json").is_err());
}
