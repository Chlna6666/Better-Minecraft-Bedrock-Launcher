//! Dump one pool entry's material table, so a family can be checked against the exact texture
//! alias the client bound to each face slot.
//!
//! Usage: `material_table <export-root> <material-ref>`

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
    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    match catalog.debug_material_table(reference) {
        Some(table) => {
            for (slot, alias) in table {
                println!("slot {slot:>3} -> {alias}");
            }
        }
        None => println!("<none>"),
    }
    Ok(())
}
