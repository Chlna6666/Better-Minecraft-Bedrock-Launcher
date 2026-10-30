//! Move the single-player character to a position by editing level.dat.
//!
//! Usage: cargo run -p bedrock-world --example set_level_spawn -- <world-dir> <x> <y> <z>

use bedrock_world::NbtTag;
use bedrock_world::level::{read_level_dat, write_level_dat_atomic};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(world), Some(x), Some(y), Some(z)) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        eprintln!("usage: set_level_spawn <world-dir> <x> <y> <z>");
        std::process::exit(2);
    };
    let position = [
        x.parse::<f64>().expect("x"),
        y.parse::<f64>().expect("y"),
        z.parse::<f64>().expect("z"),
    ];
    let path = std::path::Path::new(&world).join("level.dat");
    let mut document = read_level_dat(&path).expect("read level.dat");
    let NbtTag::Compound(root) = &mut document.root else {
        panic!("level.dat root is not a compound");
    };
    let Some(NbtTag::Compound(player)) = root.get_mut("Player") else {
        eprintln!(
            "no Player compound; root keys: {:?}",
            root.keys().collect::<Vec<_>>()
        );
        std::process::exit(1);
    };
    player.insert(
        "Pos".to_string(),
        NbtTag::List(
            position
                .iter()
                .map(|value| NbtTag::Double(*value))
                .collect(),
        ),
    );
    player.insert("DimensionId".to_string(), NbtTag::Int(0));
    write_level_dat_atomic(&path, &document).expect("write level.dat");
    eprintln!("player moved to {position:?}");
}
