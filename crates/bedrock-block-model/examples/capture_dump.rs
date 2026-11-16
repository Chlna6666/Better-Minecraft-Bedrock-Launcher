//! Dump BedrockSceneCapture geometry for exact block states, next to the built-in family shape.
//!
//! Use it to check a model family against the geometry the game itself produced.
//!
//! Usage: cargo run -p bedrock-block-model --example capture_dump -- <export-root> <name> [k=v ...] -- ...
//!
//! Each spec is a block name followed by any number of `key=value` state pairs; specs are
//! separated by `--`. Integer and boolean literals are parsed as such.

use std::path::Path;

use bedrock_block_model::model_family;
use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;
use bedrock_block_model::{BlockStateQuery, BlockStateValue, ModelShape};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: capture_dump <export-root> <name> [k=v ...] -- ...");
        std::process::exit(2);
    };
    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    println!(
        "states={} version={}",
        catalog.state_count(),
        catalog.game_version()
    );

    let mut spec = Vec::new();
    let mut specs = Vec::new();
    for arg in args {
        if arg == "--" {
            if !spec.is_empty() {
                specs.push(std::mem::take(&mut spec));
            }
        } else {
            spec.push(arg);
        }
    }
    if !spec.is_empty() {
        specs.push(spec);
    }

    // `--list <name>` dumps every captured state of one block, which is how a family with
    // several thickness/facing states gets compared without guessing the exact state values.
    if let Some(list_name) = std::env::var("CAPTURE_LIST").ok() {
        for shape_name in list_name.split(',') {
            println!("
### {shape_name}");
            for state in catalog.states_for(shape_name) {
                let label = format!("{state:?}");
                let captured = catalog.shape(&state);
                print_shape(&label, captured.as_ref());
            }
        }
        return Ok(());
    }

    for spec in specs {
        let Some(name) = spec.first() else { continue };
        let mut query = BlockStateQuery::new(name.clone());
        for pair in &spec[1..] {
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
        let label: Vec<String> = spec.clone();
        let captured = catalog.shape(&query);
        let family = model_family::resolve_shape(&query);
        println!("\n== {label:?}");
        print_shape("capture", captured.as_ref());
        print_shape("family ", family.as_ref());
    }
    Ok(())
}

fn print_shape(tag: &str, shape: Option<&ModelShape>) {
    let Some(shape) = shape else {
        println!("  {tag}: <none>");
        return;
    };
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for cuboid in &shape.cuboids {
        for axis in 0..3 {
            min[axis] = min[axis].min(cuboid.min[axis]);
            max[axis] = max[axis].max(cuboid.max[axis]);
        }
    }
    for plane in &shape.planes {
        for corner in plane.corners {
            for axis in 0..3 {
                min[axis] = min[axis].min(corner[axis]);
                max[axis] = max[axis].max(corner[axis]);
            }
        }
    }
    println!(
        "  {tag}: cuboids={} planes={} bounds=({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
        shape.cuboids.len(),
        shape.planes.len(),
        min[0],
        min[1],
        min[2],
        max[0],
        max[1],
        max[2]
    );
    if shape.cuboids.len() <= 12 {
        for cuboid in &shape.cuboids {
            println!(
                "      box ({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
                cuboid.min[0],
                cuboid.min[1],
                cuboid.min[2],
                cuboid.max[0],
                cuboid.max[1],
                cuboid.max[2]
            );
        }
    }
    if shape.planes.len() <= 24 {
        for plane in &shape.planes {
            println!(
                "      plane n={:?} slot={:?} uv={:?} {:.4?}",
                plane.normal, plane.material_slot, plane.uv, plane.corners
            );
        }
    }
}
