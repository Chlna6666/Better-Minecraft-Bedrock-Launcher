use bedrock_world::surface::WorldScanOptions;
use bedrock_world::{OpenOptions, World};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: repair_actor_storage <copied-world-path>")?;
    let verify_only = std::env::args_os().any(|argument| argument == "--verify");
    let world = World::open(
        &path,
        OpenOptions {
            read_only: verify_only,
            ..OpenOptions::default()
        },
    )?;
    if verify_only {
        let chunks = world.chunk_positions(WorldScanOptions::default())?;
        let (actors, actor_report) = world.scan_entities(WorldScanOptions::default())?;
        println!(
            "read_only_reopen=true chunks={} actors={} actor_parse_errors={}",
            chunks.len(),
            actors.len(),
            actor_report.parse_errors.len()
        );
        return Ok(());
    }
    let report = world.repair_actor_uids()?;
    world.compact_storage()?;
    println!("{report:?}");
    Ok(())
}
