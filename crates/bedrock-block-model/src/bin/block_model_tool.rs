use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

use bedrock_block_model::{
    BlockStateQuery, BlockStateValue, JavaBakedModel, JavaModelRepository,
    bake_java_model_database, java_block_id_for_bedrock_state,
    java_properties_for_bedrock_state,
};
use serde_json::json;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(usage("missing command"));
    };

    match command.as_str() {
        "map" => {
            let Some(block) = args.next() else {
                return Err(usage("map requires a Bedrock block id"));
            };
            let (state, json_output) = parse_state(block, args)?;
            print_mapping(&state, json_output);
            Ok(())
        }
        "java" => {
            let Some(root) = args.next() else {
                return Err(usage("java requires an extracted Java assets root"));
            };
            let Some(block) = args.next() else {
                return Err(usage("java requires a Bedrock block id"));
            };
            let (state, json_output) = parse_state(block, args)?;
            let repository = JavaModelRepository::from_root(PathBuf::from(root))
                .map_err(|error| error.to_string())?;
            let Some(model) = repository
                .resolve_bedrock_state(&state)
                .map_err(|error| error.to_string())?
            else {
                return Err(format!(
                    "no Java blockstate/model matched {} with {:?}",
                    state.name, state.states
                ));
            };
            print_model(&model, json_output);
            Ok(())
        }
        "bake" => {
            let Some(root) = args.next() else {
                return Err(usage("bake requires an extracted Java assets root"));
            };
            let Some(output) = args.next() else {
                return Err(usage("bake requires an output .bin path"));
            };
            let mut json_output = false;
            for arg in args {
                match arg.as_str() {
                    "--json" => json_output = true,
                    _ => return Err(usage(&format!("unknown bake option: {arg}"))),
                }
            }
            let stats = bake_java_model_database(PathBuf::from(root), PathBuf::from(&output))
                .map_err(|error| error.to_string())?;
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&stats)
                        .map_err(|error| format!("failed to serialize bake stats: {error}"))?
                );
            } else {
                println!("Java release: {}", stats.source_version);
                println!(
                    "Baked {} blocks, {} variants, {} multipart parts, {} applies",
                    stats.blocks, stats.variants, stats.multipart_parts, stats.applies
                );
                println!(
                    "Models: {} referenced ids -> {} unique geometries",
                    stats.referenced_model_ids, stats.unique_models
                );
                println!(
                    "Database: {} bytes (geometry {}, strings {}), warnings {}",
                    stats.database_bytes,
                    stats.model_data_bytes,
                    stats.string_data_bytes,
                    stats.warnings
                );
                println!("Output: {output}");
            }
            Ok(())
        }
        "-h" | "--help" | "help" => {
            println!("{}", usage(""));
            Ok(())
        }
        _ => Err(usage(&format!("unknown command: {command}"))),
    }
}

fn usage(message: &str) -> String {
    let prefix = if message.is_empty() {
        String::new()
    } else {
        format!("{message}\n\n")
    };
    format!(
        "{prefix}usage:\n  cargo run -p bedrock-block-model --bin block_model_tool -- map <bedrock-id> [state=value ...] [--json]\n  cargo run -p bedrock-block-model --bin block_model_tool -- java <extracted-java-root> <bedrock-id> [state=value ...] [--json]\n  cargo run -p bedrock-block-model --bin block_model_tool -- bake <extracted-java-root> <output.bin> [--json]\n\nThe Java root may be the directory containing assets/, assets/ itself, or assets/minecraft/."
    )
}

fn parse_state(
    block: String,
    args: impl IntoIterator<Item = String>,
) -> Result<(BlockStateQuery, bool), String> {
    let mut state = BlockStateQuery::new(block);
    let mut json_output = false;
    for arg in args {
        if arg == "--json" {
            json_output = true;
            continue;
        }
        if arg.starts_with('-') {
            return Err(usage(&format!("unknown option: {arg}")));
        }
        let Some((key, value)) = arg.split_once('=') else {
            return Err(usage(&format!("state must use key=value syntax: {arg}")));
        };
        state.states.insert(key.to_owned(), parse_state_value(value));
    }
    Ok((state, json_output))
}

fn parse_state_value(value: &str) -> BlockStateValue {
    if value.eq_ignore_ascii_case("true") {
        return BlockStateValue::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return BlockStateValue::Bool(false);
    }
    if let Ok(value) = value.parse::<i64>() {
        return BlockStateValue::Int(value);
    }
    BlockStateValue::String(value.to_owned())
}

fn print_mapping(state: &BlockStateQuery, json_output: bool) {
    let java_block_id = java_block_id_for_bedrock_state(state);
    let properties = java_properties_for_bedrock_state(state);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "bedrock_block": state.name,
                "bedrock_states": state_map_json(&state.states),
                "java_block": java_block_id,
                "java_properties": properties,
            }))
            .expect("mapping report is JSON serializable")
        );
    } else {
        println!("Bedrock: {} {:?}", state.name, state.states);
        println!("Java: {java_block_id} {properties:?}");
    }
}

fn print_model(model: &JavaBakedModel, json_output: bool) {
    if json_output {
        let cuboids = model
            .shape
            .cuboids
            .iter()
            .map(|cuboid| {
                let slots = cuboid
                    .face_material_slots
                    .iter()
                    .map(|(face, slot)| (format!("{face:?}").to_ascii_lowercase(), slot.clone()))
                    .collect::<BTreeMap<_, _>>();
                json!({
                    "min": cuboid.min,
                    "max": cuboid.max,
                    "material_slot": cuboid.material_slot,
                    "face_material_slots": slots,
                })
            })
            .collect::<Vec<_>>();
        let planes = model
            .shape
            .planes
            .iter()
            .map(|plane| {
                json!({
                    "corners": plane.corners,
                    "normal": plane.normal,
                    "material_slot": plane.material_slot,
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "java_block": model.java_block_id,
                "java_properties": model.properties,
                "source_models": model.source_models,
                "warnings": model.warnings,
                "shape": {
                    "cuboids": cuboids,
                    "planes": planes,
                }
            }))
            .expect("model report is JSON serializable")
        );
    } else {
        println!("Java block: {}", model.java_block_id);
        println!("Properties: {:?}", model.properties);
        println!("Models: {}", model.source_models.join(", "));
        println!(
            "Shape: {} cuboids, {} planes",
            model.shape.cuboids.len(),
            model.shape.planes.len()
        );
        for warning in &model.warnings {
            println!("warning: {warning}");
        }
    }
}

fn state_map_json(states: &BTreeMap<String, BlockStateValue>) -> serde_json::Value {
    let states = states
        .iter()
        .map(|(key, value)| {
            let value = match value {
                BlockStateValue::Bool(value) => json!(value),
                BlockStateValue::Int(value) => json!(value),
                BlockStateValue::String(value) => json!(value),
            };
            (key.clone(), value)
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::Value::Object(states)
}
