//! Dump one appearance entry's per-triangle UVs and texture slots, so a family porting captured
//! geometry can bind the same art rectangles the client used.
//!
//! Usage: `appearance_uv <export-root> <appearance-ref> [<material-ref>]`

use std::path::Path;

use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        std::process::exit(2);
    };
    let Some(reference) = args.next() else {
        std::process::exit(2);
    };
    let reference: u32 = reference.parse()?;
    let material_ref: Option<u32> = args.next().and_then(|value| value.parse().ok());
    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    let Some(rows) = catalog.debug_appearance_uv(reference) else {
        println!("<none>");
        return Ok(());
    };
    let table = material_ref.and_then(|reference| catalog.debug_material_table(reference));
    for (index, corners) in rows.iter().enumerate() {
        let slot = corners[0].1;
        let alias = slot
            .checked_sub(1)
            .and_then(|index| table.as_ref().and_then(|table| table.get(index as usize)))
            .map(|(_slot, alias)| alias.as_str())
            .unwrap_or("?");
        let uvs: Vec<String> = corners
            .iter()
            .map(|(uv, _)| format!("({:.4},{:.4})", uv[0], uv[1]))
            .collect();
        println!("tri {index:>2} slot {slot} [{alias}] {}", uvs.join(" "));
    }
    Ok(())
}
