//! List plant families whose captured geometry varies by state, so the family code can be
//! checked for the same state coverage.
use std::collections::BTreeMap;
use std::path::Path;
use bedrock_block_model::runtime_capture::RuntimeCaptureCatalog;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::args().nth(1) else { std::process::exit(2) };
    let catalog = RuntimeCaptureCatalog::open(Path::new(&root))?;
    let mut by_name: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for name in std::env::args().skip(2) {
        for state in catalog.states_for(&name) {
            let label = format!("{:?}", state);
            let boxes = catalog
                .shape(&state)
                .map(|shape| shape.cuboids.len() + shape.planes.len())
                .unwrap_or(0);
            by_name.entry(name.clone()).or_default().push((label, boxes));
        }
    }
    for (name, states) in by_name {
        let distinct: std::collections::BTreeSet<usize> =
            states.iter().map(|(_, boxes)| *boxes).collect();
        println!(
            "{name}: {} captured states, {} distinct part counts {:?}",
            states.len(),
            distinct.len(),
            distinct
        );
    }
    Ok(())
}
