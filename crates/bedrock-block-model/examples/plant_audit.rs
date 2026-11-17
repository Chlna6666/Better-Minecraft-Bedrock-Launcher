//! Compare every captured plant state against the procedural family shape.
//!
//! The catalog skips `dynamic`/`partial` entries, so plant families are never checked by
//! `capture_dump`. This example reads `blocks.json` directly, decodes each entry's state NBT,
//! unions the raw pool geometry of its variants, and prints the captured bounds next to the
//! family's. A plant whose family shape ignores a state axis shows up as one family row repeated
//! across several distinct capture rows.
//!
//! Usage: `plant_audit <export-root> <name> [<name> ...]`

use std::collections::BTreeMap;
use std::path::Path;

use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;
use bedrock_block_model::{BlockStateQuery, BlockStateValue};
use serde_json::Value;

type Bounds = ([f32; 3], [f32; 3]);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        std::process::exit(2);
    };
    let wanted: Vec<String> = args.map(|name| normalize(&name)).collect();
    if wanted.is_empty() {
        std::process::exit(2);
    }

    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    let index: Value =
        serde_json::from_slice(&std::fs::read(Path::new(&root).join("blocks.json"))?)?;
    let entries = index["blocks"].as_array().ok_or("blocks.json: no blocks array")?;

    let mut rows: BTreeMap<String, Vec<(String, Option<Bounds>, Option<Bounds>)>> = BTreeMap::new();
    for entry in entries {
        let name = normalize(entry["name"].as_str().unwrap_or_default());
        if !wanted.contains(&name) {
            continue;
        }
        let Some(hex) = entry["state_nbt_hex"].as_str() else {
            continue;
        };
        let Some(states) = decode_states(hex) else {
            continue;
        };
        let mut captured: Option<Bounds> = None;
        for variant in entry["variants"].as_array().into_iter().flatten() {
            // `registry_visual_bounds` is the placeholder full cube the game reports for a plant
            // whose real mesh is built per position; only native scene samples carry the art.
            if variant["neighborhood"].as_str() != Some("native_scene") {
                continue;
            }
            for id in variant["shared_geometry_refs"].as_array().into_iter().flatten() {
                let Some(id) = id.as_u64() else { continue };
                let Some(vertices) = catalog.debug_geometry_vertices(id as u32) else {
                    continue;
                };
                for vertex in vertices {
                    captured = Some(union(captured, vertex));
                }
            }
        }
        let mut query = BlockStateQuery::new(name.clone());
        for (key, value) in &states {
            query = query.with_state(key.clone(), value.clone());
        }
        let family = bedrock_block_model::model_family::resolve_shape(&query)
            .and_then(|shape| shape_bounds(&shape));
        let label = states
            .iter()
            .map(|(key, value)| format!("{key}={value:?}"))
            .collect::<Vec<_>>()
            .join(" ");
        rows.entry(name).or_default().push((label, captured, family));
    }

    for (name, states) in rows {
        println!("\n=== {name} ({} captured states)", states.len());
        for (label, captured, family) in states {
            println!(
                "  {label:<48} capture {:<30} family {}",
                fmt_bounds(captured),
                fmt_bounds(family)
            );
        }
    }
    Ok(())
}

fn normalize(name: &str) -> String {
    name.strip_prefix("minecraft:").unwrap_or(name).to_owned()
}

fn union(current: Option<Bounds>, point: [f32; 3]) -> Bounds {
    match current {
        None => (point, point),
        Some((mut min, mut max)) => {
            for axis in 0..3 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
            (min, max)
        }
    }
}

fn shape_bounds(shape: &bedrock_block_model::ModelShape) -> Option<Bounds> {
    let mut bounds: Option<Bounds> = None;
    for cuboid in &shape.cuboids {
        for point in [
            [cuboid.min[0], cuboid.min[1], cuboid.min[2]],
            [cuboid.max[0], cuboid.max[1], cuboid.max[2]],
        ] {
            bounds = Some(union(bounds, point));
        }
    }
    for plane in &shape.planes {
        for corner in plane.corners {
            bounds = Some(union(bounds, corner));
        }
    }
    bounds
}

fn fmt_bounds(bounds: Option<Bounds>) -> String {
    match bounds {
        Some((min, max)) => format!(
            "({:.3},{:.3},{:.3})..({:.3},{:.3},{:.3})",
            min[0], min[1], min[2], max[0], max[1], max[2]
        ),
        None => "<none>".to_owned(),
    }
}

/// Decode the `states` compound of one capture entry.
///
/// The export stores a Bedrock little-endian NBT root whose only interesting member is `states`;
/// every value is a byte, short, int or string, which is all a block-state map can hold.
fn decode_states(hex: &str) -> Option<Vec<(String, BlockStateValue)>> {
    let bytes = decode_hex(hex)?;
    let mut at = 0;
    let tag = read_u8(&bytes, &mut at)?;
    if tag != 10 {
        return None;
    }
    read_string(&bytes, &mut at)?;
    let root = read_compound(&bytes, &mut at)?;
    let states = root.into_iter().find(|(key, _)| key == "states")?.1;
    let Nbt::Compound(map) = states else {
        return None;
    };
    map.into_iter()
        .map(|(key, value)| {
            let value = match value {
                Nbt::Byte(v) => BlockStateValue::Int(i64::from(v)),
                Nbt::Short(v) => BlockStateValue::Int(i64::from(v)),
                Nbt::Int(v) => BlockStateValue::Int(i64::from(v)),
                Nbt::String(v) => BlockStateValue::String(v),
                _ => return None,
            };
            Some((key, value))
        })
        .collect()
}

enum Nbt {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    List(Vec<Nbt>),
    Compound(Vec<(String, Nbt)>),
    ByteArray,
    IntArray,
}

fn read_compound(bytes: &[u8], at: &mut usize) -> Option<Vec<(String, Nbt)>> {
    let mut out = Vec::new();
    loop {
        let tag = read_u8(bytes, at)?;
        if tag == 0 {
            return Some(out);
        }
        let name = read_string(bytes, at)?;
        out.push((name, read_payload(bytes, at, tag)?));
    }
}

fn read_payload(bytes: &[u8], at: &mut usize, tag: u8) -> Option<Nbt> {
    Some(match tag {
        1 => Nbt::Byte(read_u8(bytes, at)? as i8),
        2 => Nbt::Short(i16::from_le_bytes(read_array(bytes, at)?)),
        3 => Nbt::Int(i32::from_le_bytes(read_array(bytes, at)?)),
        4 => Nbt::Long(i64::from_le_bytes(read_array(bytes, at)?)),
        5 => Nbt::Float(f32::from_le_bytes(read_array(bytes, at)?)),
        6 => Nbt::Double(f64::from_le_bytes(read_array(bytes, at)?)),
        7 => {
            let len = i32::from_le_bytes(read_array(bytes, at)?).max(0) as usize;
            *at += len;
            Nbt::ByteArray
        }
        8 => Nbt::String(read_string(bytes, at)?),
        9 => {
            let element = read_u8(bytes, at)?;
            let len = i32::from_le_bytes(read_array(bytes, at)?).max(0) as usize;
            let mut items = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                items.push(read_payload(bytes, at, element)?);
            }
            Nbt::List(items)
        }
        10 => Nbt::Compound(read_compound(bytes, at)?),
        11 => {
            let len = i32::from_le_bytes(read_array(bytes, at)?).max(0) as usize;
            *at += len * 4;
            Nbt::IntArray
        }
        _ => return None,
    })
}

fn read_u8(bytes: &[u8], at: &mut usize) -> Option<u8> {
    let value = *bytes.get(*at)?;
    *at += 1;
    Some(value)
}

fn read_array<const N: usize>(bytes: &[u8], at: &mut usize) -> Option<[u8; N]> {
    let slice = bytes.get(*at..*at + N)?;
    *at += N;
    slice.try_into().ok()
}

fn read_string(bytes: &[u8], at: &mut usize) -> Option<String> {
    let len = u16::from_le_bytes(read_array(bytes, at)?) as usize;
    let slice = bytes.get(*at..*at + len)?;
    *at += len;
    Some(String::from_utf8_lossy(slice).into_owned())
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}
