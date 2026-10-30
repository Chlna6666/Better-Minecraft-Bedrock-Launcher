//! Move every stored player of a Bedrock world to a position.
//!
//! Usage: cargo run -p bedrock-world --example set_player_pos -- <world-dir> <x> <y> <z>

use bedrock_world::NbtTag;
use bedrock_world::world::{OpenOptions, World};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(world_path), Some(x), Some(y), Some(z)) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        eprintln!("usage: set_player_pos <world-dir> <x> <y> <z>");
        std::process::exit(2);
    };
    let position = [
        x.parse::<f64>().expect("x"),
        y.parse::<f64>().expect("y"),
        z.parse::<f64>().expect("z"),
    ];
    let mut options = OpenOptions::default();
    options.read_only = false;
    let world = World::open(&world_path, options).expect("open world");
    let ids = world.players().expect("list players");
    eprintln!("players: {ids:?}");
    for id in ids {
        let Some(mut player) = world.player(&id).expect("read player") else {
            continue;
        };
        let NbtTag::Compound(root) = &mut player.nbt else {
            eprintln!("player {id:?} root is not a compound");
            continue;
        };
        eprintln!("player {id:?} keys: {:?}", root.keys().collect::<Vec<_>>());
        root.insert(
            "Pos".to_string(),
            NbtTag::List(
                position
                    .iter()
                    .map(|value| NbtTag::Double(*value))
                    .collect(),
            ),
        );
        if root.contains_key("DimensionId") {
            root.insert("DimensionId".to_string(), NbtTag::Int(0));
        }
        world.save_player(&player).expect("save player");
        eprintln!("player {id:?} moved to {position:?}");
    }
}
