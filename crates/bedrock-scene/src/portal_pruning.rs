use super::*;

#[test]
fn hidden_band_prunes_cube_faces_but_preserves_visible_neighbour_and_height_halo() {
    let pos = ChunkPos {
        x: -1,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: pos.x,
        max_chunk_x: pos.x,
        min_chunk_z: pos.z,
        max_chunk_z: pos.z,
    });
    builder.insert_stream_chunk(
        pos,
        ChunkBlocks::test_cubes(&[[-15, 8, 3], [-14, 63, 3], [-13, 70, 3]]),
    );
    let hidden = BTreeSet::from([(pos, 0)]);
    assert!(section_is_hidden_with_face_halo(&hidden, pos, 8));
    assert!(!section_is_hidden_with_face_halo(&hidden, pos, 63));
    assert!(!section_is_hidden_with_face_halo(&hidden, pos, 70));
    assert!(!section_is_hidden_with_face_halo(
        &BTreeSet::from([(pos, -1)]),
        pos,
        -1,
    ));
    let ordinary = builder.build_stream_faces(&[pos], StreamFaceOptions::default());
    let trimmed = builder.build_stream_faces(
        &[pos],
        StreamFaceOptions {
            selections: Some(&FxHashMap::default()),
            gpu_cubes: Some(&FxHashSet::default()),
            hidden_bands: Some(&hidden),
            ..Default::default()
        },
    );
    assert_eq!(ordinary.len(), 1);
    assert_eq!(trimmed.len(), 1);
    assert!(trimmed[0].faces.len() < ordinary[0].faces.len());
    assert!(
        trimmed[0]
            .faces
            .iter()
            .any(|face| face.corners.iter().any(|p| p[1] >= 70.0))
    );
    assert!(
        !trimmed[0]
            .faces
            .iter()
            .any(|face| face.corners.iter().all(|p| p[1] <= 9.0))
    );
}
