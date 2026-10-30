//! Rename a Bedrock world level.dat LevelName.
//!
//! Usage: cargo run -p bedrock-world --example rename_world -- <world-dir> <new-name>

use bedrock_world::NbtTag;
use bedrock_world::level::{read_level_dat, write_level_dat_atomic};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(world), Some(name)) = (args.next(), args.next()) else {
        eprintln!("usage: rename_world <world-dir> <new-name>");
        std::process::exit(2);
    };
    let path = std::path::Path::new(&world).join("level.dat");
    let mut document = read_level_dat(&path).expect("read level.dat");
    let NbtTag::Compound(root) = &mut document.root else {
        panic!("level.dat root is not a compound");
    };
    root.insert("LevelName".to_string(), NbtTag::String(name.clone()));
    write_level_dat_atomic(&path, &document).expect("write level.dat");
    eprintln!("renamed {world} to {name}");
}
