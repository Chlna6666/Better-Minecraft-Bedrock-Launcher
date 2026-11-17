//! Print one appearance entry's geometry id / source kind, then dump that geometry's quads.
//! Usage: appearance_info <export-root> <appearance-ref>

use std::path::Path;

use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else { std::process::exit(2) };
    let Some(reference) = args.next() else { std::process::exit(2) };
    let reference: u32 = reference.parse()?;
    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    let Some((geometry_id, source_kind, uv_complete)) = catalog.debug_appearance_info(reference) else {
        println!("<no appearance>");
        return Ok(());
    };
    println!("appearance {reference}: geometry={geometry_id} source_kind={source_kind} uv_complete={uv_complete}");
    if let Some(vertices) = catalog.debug_geometry_vertices(geometry_id) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in &vertices {
            for (i, value) in v.iter().enumerate() {
                min[i] = min[i].min(*value);
                max[i] = max[i].max(*value);
            }
        }
        println!("geometry {geometry_id}: verts={} bounds={min:?}..{max:?}", vertices.len());
        for (i, v) in vertices.iter().enumerate() {
            let px = v.map(|x| x * 16.0);
            println!("  v{i:>2} {px:?}");
        }
    }
    Ok(())
}
