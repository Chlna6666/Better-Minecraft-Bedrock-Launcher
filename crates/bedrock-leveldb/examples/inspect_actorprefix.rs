use bedrock_leveldb::{Db, LevelDbOpenOptions, ReadOptions, VisitorControl};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_path = std::env::args_os()
        .nth(1)
        .ok_or("usage: inspect_actorprefix <world-db-path>")?;
    let database = Db::open(
        database_path,
        LevelDbOpenOptions {
            read_only: true,
            ..LevelDbOpenOptions::default()
        },
    )?;
    database.for_each_prefix(b"actorprefix", ReadOptions::default(), |key, value| {
        println!(
            "key={} value_len={} value={}",
            hex(key),
            value.len(),
            hex(value)
        );
        Ok(VisitorControl::Stop)
    })?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
