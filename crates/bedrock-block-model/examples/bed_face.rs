//! Render one bed side face by sampling the shipped sprite through the family's own UVs.
//!
//! A headless world render needs a source world, which a model-only check should not depend on.
//! This instead takes the exact UV quad the bed family emits for one face, samples the real
//! 64x64 `textures/entity/bed/white.png` at those coordinates, and prints the result as a coarse
//! colour map. If the face's UVs are right the printout shows the sheet over the frame; if they
//! are rotated or off by a band it shows the frame running the wrong way.
//!
//! Usage: `bed_face <texture.png> <direction> <head|foot> <face>`

use std::collections::BTreeMap;

use bedrock_block_model::model_family;
use bedrock_block_model::{BlockFace, BlockStateQuery};
use image::GenericImageView;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(texture_path) = args.next() else {
        std::process::exit(2);
    };
    let direction: i64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let head = args.next().as_deref() != Some("foot");
    let face_name = args.next().unwrap_or_else(|| "North".to_owned());
    let face = match face_name.to_ascii_lowercase().as_str() {
        "up" => BlockFace::Up,
        "down" => BlockFace::Down,
        "north" => BlockFace::North,
        "south" => BlockFace::South,
        "east" => BlockFace::East,
        _ => BlockFace::West,
    };

    let sprite = image::open(&texture_path)?;
    let (width, height) = sprite.dimensions();
    println!("sprite {width}x{height}");

    let state = BlockStateQuery::new("minecraft:bed")
        .with_state("direction", direction)
        .with_state("head_piece_bit", head);
    let shape = model_family::resolve_shape(&state).ok_or("no bed shape")?;

    // Collect the planes of the requested face, in block space, and sample each corner's UV.
    let mut rows: BTreeMap<i32, Vec<(f32, String)>> = BTreeMap::new();
    for plane in &shape.planes {
        let normal = match plane.normal {
            [0, 1, 0] => BlockFace::Up,
            [0, -1, 0] => BlockFace::Down,
            [0, 0, -1] => BlockFace::North,
            [0, 0, 1] => BlockFace::South,
            [1, 0, 0] => BlockFace::East,
            [-1, 0, 0] => BlockFace::West,
            _ => continue,
        };
        if normal != face {
            continue;
        }
        let Some(uv) = plane.uv else { continue };
        // Sample a 6x6 grid across the quad in its own corner order.
        for gy in 0..6 {
            for gx in 0..6 {
                let s = (gx as f32 + 0.5) / 6.0;
                let t = (gy as f32 + 0.5) / 6.0;
                let u = bilinear(uv[0][0], uv[1][0], uv[2][0], uv[3][0], s, t);
                let v = bilinear(uv[0][1], uv[1][1], uv[2][1], uv[3][1], s, t);
                let px = ((u * width as f32) as u32).min(width - 1);
                let py = ((v * height as f32) as u32).min(height - 1);
                let pixel = sprite.to_rgba8().get_pixel(px, py).0;
                let cell = classify(pixel);
                // Vertical position in the quad, from the block-space corners.
                let y = bilinear(
                    plane.corners[0][1],
                    plane.corners[1][1],
                    plane.corners[2][1],
                    plane.corners[3][1],
                    s,
                    t,
                );
                let band = (y * 24.0) as i32;
                rows.entry(band).or_default().push((s, cell));
            }
        }
        println!("  plane {face:?} slot={:?}", plane.material_slot);
        for corner in uv {
            println!("      uv ({:.4},{:.4}) -> px ({:.1},{:.1})", corner[0], corner[1], corner[0] * width as f32, corner[1] * height as f32);
        }
    }

    println!("\n  sampled {face:?} (rows = block-space Y, top of printout = top of face):");
    for (band, cells) in rows.iter().rev() {
        let mut cells = cells.clone();
        cells.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let line: String = cells.into_iter().map(|(_, cell)| cell).collect();
        println!("   y{:>5.2}  {line}", *band as f32 / 24.0);
    }
    Ok(())
}

fn bilinear(a: f32, b: f32, c: f32, d: f32, s: f32, t: f32) -> f32 {
    (1.0 - s) * (1.0 - t) * a + s * (1.0 - t) * b + s * t * c + (1.0 - s) * t * d
}

fn classify(pixel: [u8; 4]) -> String {
    let [r, g, b, a] = pixel;
    if a < 10 {
        return ".".to_owned();
    }
    let brown = r > 120 && g > 85 && b < 120 && r >= g && g > b;
    let white = r > 225 && g > 225 && b > 225;
    let light = r > 195 && g > 195 && b > 195;
    let grey = (r as i32 - g as i32).abs() < 30 && (g as i32 - b as i32).abs() < 30;
    if brown {
        "B".to_owned()
    } else if white {
        "W".to_owned()
    } else if light {
        "w".to_owned()
    } else if grey {
        "G".to_owned()
    } else {
        "?".to_owned()
    }
}
