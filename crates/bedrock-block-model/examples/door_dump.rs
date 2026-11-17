//! Compare the door family's slab against the client capture for every state.
//!
//! Doors are the one two-block family whose geometry the client varies per state, and the state
//! has four axes (direction, hinge, open, upper). This prints, per state, the capture's thin axis
//! and which side of the cell the slab sits on, next to what the family produces, so a mismatch in
//! the direction mapping shows up directly.
//!
//! Usage: `door_dump <export-root> [block-name]`

use std::collections::BTreeMap;
use std::path::Path;

use bedrock_block_model::model_family;
use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;
use bedrock_block_model::{BlockStateQuery, BlockStateValue};
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        std::process::exit(2);
    };
    let block = args.next().unwrap_or_else(|| "wooden_door".to_owned());
    let block = block.strip_prefix("minecraft:").unwrap_or(&block).to_owned();

    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    let index: Value =
        serde_json::from_slice(&std::fs::read(Path::new(&root).join("blocks.json"))?)?;
    let entries = index["blocks"].as_array().ok_or("blocks.json: no blocks array")?;

    let mut rows = BTreeMap::new();
    for entry in entries {
        let name = entry["name"].as_str().unwrap_or_default();
        if name.strip_prefix("minecraft:").unwrap_or(name) != block {
            continue;
        }
        let Some(hex) = entry["state_nbt_hex"].as_str() else {
            continue;
        };
        let Some(states) = decode_states(hex) else {
            continue;
        };
        let mut captured: Option<([f32; 3], [f32; 3])> = None;
        for variant in entry["variants"].as_array().into_iter().flatten() {
            for id in variant["shared_geometry_refs"].as_array().into_iter().flatten() {
                let Some(id) = id.as_u64() else { continue };
                let Some(vertices) = catalog.debug_geometry_vertices(id as u32) else {
                    continue;
                };
                for vertex in vertices {
                    captured = Some(match captured {
                        None => (vertex, vertex),
                        Some((mut min, mut max)) => {
                            for axis in 0..3 {
                                min[axis] = min[axis].min(vertex[axis]);
                                max[axis] = max[axis].max(vertex[axis]);
                            }
                            (min, max)
                        }
                    });
                }
            }
        }
        let mut query = BlockStateQuery::new(format!("minecraft:{block}"));
        for (key, value) in &states {
            query = query.with_state(key.clone(), value.clone());
        }
        let family = model_family::resolve_shape(&query).and_then(|shape| {
            let mut bounds: Option<([f32; 3], [f32; 3])> = None;
            for cuboid in &shape.cuboids {
                for point in [
                    [cuboid.min[0], cuboid.min[1], cuboid.min[2]],
                    [cuboid.max[0], cuboid.max[1], cuboid.max[2]],
                ] {
                    bounds = Some(match bounds {
                        None => (point, point),
                        Some((mut min, mut max)) => {
                            for axis in 0..3 {
                                min[axis] = min[axis].min(point[axis]);
                                max[axis] = max[axis].max(point[axis]);
                            }
                            (min, max)
                        }
                    });
                }
            }
            for plane in &shape.planes {
                for corner in plane.corners {
                    bounds = Some(match bounds {
                        None => (corner, corner),
                        Some((mut min, mut max)) => {
                            for axis in 0..3 {
                                min[axis] = min[axis].min(corner[axis]);
                                max[axis] = max[axis].max(corner[axis]);
                            }
                            (min, max)
                        }
                    });
                }
            }
            bounds
        });
        let label = states
            .iter()
            .map(|(key, value)| format!("{key}={value:?}"))
            .collect::<Vec<_>>()
            .join(" ");
        rows.insert(label, (captured, family));
    }

    let mut mismatches = 0;
    for (label, (captured, family)) in &rows {
        let capture_desc = slab_desc(*captured);
        let family_desc = slab_desc(*family);
        let verdict = if capture_desc == family_desc {
            "ok"
        } else {
            mismatches += 1;
            "MISMATCH"
        };
        println!("{label:<78} capture {capture_desc:<14} family {family_desc:<14} {verdict}");
    }
    println!("{} states, {mismatches} mismatched", rows.len());
    Ok(())
}

/// Describe a slab as its thin axis plus which side of the cell it occupies.
fn slab_desc(bounds: Option<([f32; 3], [f32; 3])>) -> String {
    let Some((min, max)) = bounds else {
        return "<none>".to_owned();
    };
    let span_x = max[0] - min[0];
    let span_z = max[2] - min[2];
    if (span_x - span_z).abs() < 1e-3 {
        return "full".to_owned();
    }
    if span_x < span_z {
        if min[0] < 0.1 {
            "X low".to_owned()
        } else {
            "X high".to_owned()
        }
    } else if min[2] < 0.1 {
        "Z low".to_owned()
    } else {
        "Z high".to_owned()
    }
}

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
