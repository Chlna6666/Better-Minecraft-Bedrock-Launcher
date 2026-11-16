//! List or extract entries from a `.brarchive`.
//! extract_entry <archive> --list <substring> | <entry> <out> [<entry> <out> ...]

use std::path::Path;
use bedrock_brarchive::{Acceleration, Archive};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(archive_path) = args.next() else { std::process::exit(2) };
    let archive = Archive::open(Path::new(&archive_path), Acceleration::Auto)?;
    if let Some(flag) = args.next() {
        if flag == "--list" {
            let needle = args.next().unwrap_or_default();
            for entry in archive.index().entries() {
                if entry.name().contains(&needle) {
                    println!("{}", entry.name());
                }
            }
            return Ok(());
        }
        let mut name = flag;
        loop {
            let Some(out) = args.next() else { break };
            let mut buffer = Vec::new();
            let found = archive.read_into(&name, &mut buffer)?;
            println!("{name}: found={found} bytes={}", buffer.len());
            if found { std::fs::write(&out, &buffer)?; }
            match args.next() { Some(next) => name = next, None => break }
        }
    }
    Ok(())
}
