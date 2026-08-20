use bedrock_world::{BedrockWorld, BedrockWorldOpenOptions, WorldScanOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: repair_actor_snapshot <copied-world-path>")?;
    let verify_only = std::env::args_os().any(|argument| argument == "--verify");
    let world = BedrockWorld::open_blocking(
        &path,
        BedrockWorldOpenOptions {
            read_only: verify_only,
            ..BedrockWorldOpenOptions::default()
        },
    )?;
    if verify_only {
        let chunks = world.list_chunk_positions_blocking(WorldScanOptions::default())?;
        let (actors, actor_report) = world.scan_entities_blocking(WorldScanOptions::default())?;
        println!(
            "read_only_reopen=true chunks={} actors={} actor_parse_errors={}",
            chunks.len(),
            actors.len(),
            actor_report.parse_errors.len()
        );
        return Ok(());
    }
    let report = world.repair_actor_uids_blocking()?;
    world.compact_storage_blocking()?;
    println!("{report:?}");
    Ok(())
}
