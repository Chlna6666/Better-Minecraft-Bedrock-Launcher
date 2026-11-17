//! Print the resolved material and per-face UV rects (in texture pixels) for one block state.
//!
//! This is the check that catches "the geometry is right but the art is on the wrong part of the
//! sheet" bugs: it shows, per face, the exact texture rectangle the family samples, so a flipped
//! V axis or a wrong strip is visible as a rectangle in the wrong place.
//!
//! Usage: `uv_probe <block> [k=v ...]` — e.g. `uv_probe white_bed direction=0 head_piece_bit=1`

use bedrock_block_model::model_family;
use bedrock_block_model::{BlockFace, BlockStateQuery, BlockStateValue};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(name) = args.next() else {
        std::process::exit(2);
    };
    let mut query = BlockStateQuery::new(format!("minecraft:{name}"));
    for pair in args {
        let Some((key, raw)) = pair.split_once('=') else {
            continue;
        };
        let value = if let Ok(number) = raw.parse::<i64>() {
            BlockStateValue::Int(number)
        } else if raw == "true" || raw == "false" {
            BlockStateValue::Bool(raw == "true")
        } else {
            BlockStateValue::String(raw.to_owned())
        };
        query = query.with_state(key.to_owned(), value);
    }
    let Some(shape) = model_family::resolve_shape(&query) else {
        println!("no shape for {query:?}");
        return;
    };
    println!(
        "{name} {query:?}\n  cuboids={} planes={}",
        shape.cuboids.len(),
        shape.planes.len()
    );
    // The sprite the bed family reads is 64x64; every other family here is a 16x16 tile.
    let span = if name == "bed" || name.ends_with("_bed") {
        64.0
    } else {
        16.0
    };
    for (index, cuboid) in shape.cuboids.iter().enumerate() {
        println!(
            "  cuboid {index} ({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4}) slot={:?}",
            cuboid.min[0],
            cuboid.min[1],
            cuboid.min[2],
            cuboid.max[0],
            cuboid.max[1],
            cuboid.max[2],
            cuboid.material_slot
        );
        for face in [
            BlockFace::Up,
            BlockFace::Down,
            BlockFace::North,
            BlockFace::South,
            BlockFace::East,
            BlockFace::West,
        ] {
            let Some(uv) = cuboid.face_uvs.get(&face) else {
                continue;
            };
            let slot = cuboid
                .face_material_slots
                .get(&face)
                .cloned()
                .or_else(|| cuboid.material_slot.clone());
            println!(
                "      {:?} u {:5.1}..{:5.1}  v {:5.1}..{:5.1}   slot={:?}",
                face,
                uv.iter().map(|c| c[0] * span).fold(f32::INFINITY, f32::min),
                uv.iter().map(|c| c[0] * span).fold(f32::NEG_INFINITY, f32::max),
                uv.iter().map(|c| c[1] * span).fold(f32::INFINITY, f32::min),
                uv.iter().map(|c| c[1] * span).fold(f32::NEG_INFINITY, f32::max),
                slot,
            );
        }
    }
    for (index, plane) in shape.planes.iter().enumerate() {
        println!(
            "  plane {index} n={:?} slot={:?} uv_px={:?}",
            plane.normal,
            plane.material_slot,
            plane.uv.map(|uv| uv.map(|c| [(c[0] * span).round(), (c[1] * span).round()]))
        );
    }
}
