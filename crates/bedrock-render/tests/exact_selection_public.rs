use bedrock_render::{ChunkPos, Dimension, ExactChunkRenderPlan, ExactChunkSelection};

fn chunk(x: i32, z: i32) -> ChunkPos {
    ChunkPos {
        x,
        z,
        dimension: Dimension::Overworld,
    }
}

#[test]
fn exact_selection_api_is_public_and_preserves_holes() {
    let selection = ExactChunkSelection::new([chunk(0, 0), chunk(1, 0), chunk(0, 1), chunk(6, 2)])
        .expect("public exact selection");
    let plan = ExactChunkRenderPlan::new(selection);

    assert_eq!(plan.chunk_count(), 4);
    assert!(plan.contains(chunk(0, 1)));
    assert!(!plan.contains(chunk(1, 1)));
    assert!(plan.bounds().chunk_count() > plan.chunk_count());
    assert_eq!(
        plan.rectangle_cover()
            .iter()
            .map(|bounds| bounds.chunk_count())
            .sum::<usize>(),
        plan.chunk_count()
    );
}
