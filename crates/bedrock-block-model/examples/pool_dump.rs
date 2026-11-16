//! Dump raw capture geometry entries by pool id, for families the catalog skips.
//!
//! Usage: pool_dump <export-root> <id> [<id> ...]
use std::path::Path;
use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else { std::process::exit(2) };
    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    for id in args {
        let id: u32 = id.parse()?;
        match catalog.debug_geometry_vertices(id) {
            Some(vertices) => {
                let mut min = [f32::INFINITY; 3];
                let mut max = [f32::NEG_INFINITY; 3];
                for v in &vertices {
                    for a in 0..3 {
                        min[a] = min[a].min(v[a]);
                        max[a] = max[a].max(v[a]);
                    }
                }
                println!("id={id} vertices={} bounds=({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
                    vertices.len(), min[0], min[1], min[2], max[0], max[1], max[2]);
                for v in vertices.iter().take(24) {
                    println!("   {:.4} {:.4} {:.4}", v[0], v[1], v[2]);
                }
            }
            None => println!("id={id} <none>"),
        }
    }
    Ok(())
}
