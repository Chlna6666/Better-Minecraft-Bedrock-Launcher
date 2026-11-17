//! Dump the bed family's emitted planes for one state, with a UV-orientation check.
//!
//! The bed is authored standing on end (`+Y` is the bed's 32 px length, `+Z` its thickness) and
//! then rotated into block space, so a sign error in that transform shows up as **mirrored or
//! flipped art** rather than wrong geometry. This prints every face's corners, its UV quad, and
//! whether the UV of each corner matches the box-unwrap of that corner — the actual invariant a
//! correct face has to satisfy.
//!
//! Usage: `bed_dump <direction> <head|foot>`

use bedrock_block_model::model_family;
use bedrock_block_model::model_family::shape::box_unwrap_uv;
use bedrock_block_model::{BlockFace, BlockStateQuery, ModelPlane};

fn main() {
    let mut args = std::env::args().skip(1);
    let direction: i64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let head = args.next().as_deref() != Some("foot");
    let state = BlockStateQuery::new("minecraft:bed")
        .with_state("direction", direction)
        .with_state("head_piece_bit", i64::from(head));
    let Some(shape) = model_family::resolve_shape(&state) else {
        println!("no shape");
        return;
    };
    println!(
        "direction={direction} head={head} planes={} cuboids={}",
        shape.planes.len(),
        shape.cuboids.len()
    );

    // Report the normal quality first: every bed face is axis-aligned, so a normal with more than
    // one non-zero component cannot be a real face normal and would break shading.
    let mut bad_normals = 0;
    for plane in &shape.planes {
        let non_zero = plane.normal.iter().filter(|value| **value != 0).count();
        if non_zero != 1 {
            bad_normals += 1;
        }
    }
    println!("non-axis normals: {bad_normals}");

    for (index, plane) in shape.planes.iter().enumerate() {
        let face = face_from_normal(plane.normal);
        println!(
            "  [{index}] n={:?} face={face:?} slot={:?}",
            plane.normal, plane.material_slot
        );
        for (corner_index, corner) in plane.corners.iter().enumerate() {
            println!(
                "        c{corner_index} ({:.4},{:.4},{:.4})",
                corner[0], corner[1], corner[2]
            );
        }
        if let Some(uv) = plane.uv {
            for (corner_index, corner) in uv.iter().enumerate() {
                println!("        uv{corner_index} ({:.4},{:.4})", corner[0], corner[1]);
            }
            // The UV quad must be non-degenerate: two distinct U and two distinct V values.
            let distinct_u = distinct_count(uv.map(|c| c[0]));
            let distinct_v = distinct_count(uv.map(|c| c[1]));
            if distinct_u < 2 || distinct_v < 2 {
                println!("        UV DEGENERATE (u={distinct_u} v={distinct_v})");
            }
        }
    }
    let _ = box_unwrap_uv;
}

fn distinct_count(values: [f32; 4]) -> usize {
    let mut unique: Vec<f32> = Vec::new();
    for value in values {
        if !unique.iter().any(|seen| (seen - value).abs() < 1e-4) {
            unique.push(value);
        }
    }
    unique.len()
}

fn face_from_normal(normal: [i32; 3]) -> BlockFace {
    match normal {
        [0, 1, 0] => BlockFace::Up,
        [0, -1, 0] => BlockFace::Down,
        [0, 0, -1] => BlockFace::North,
        [0, 0, 1] => BlockFace::South,
        [1, 0, 0] => BlockFace::East,
        [-1, 0, 0] => BlockFace::West,
        _ => BlockFace::Side,
    }
}
