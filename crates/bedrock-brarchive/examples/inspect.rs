//! Read-only corpus validation: inspect metadata without loading payloads.
use std::path::Path;
use std::time::Instant;

use bedrock_brarchive::{Acceleration, Archive};

fn inspect(path: &Path, counts: &mut [u64; 4]) -> Result<(), Box<dyn std::error::Error>> {
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            inspect(&entry?.path(), counts)?;
        }
    } else if path
        .extension()
        .is_some_and(|extension| extension == "brarchive")
    {
        let archive = Archive::open(path, Acceleration::Auto)?;
        counts[0] += 1;
        for entry in archive.index().entries() {
            counts[1] += 1;
            counts[2] += u64::from(entry.len());
            counts[3] += u64::from(entry.is_empty());
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: inspect ARCHIVE_OR_DIRECTORY")?;
    let started = Instant::now();
    let mut counts = [0; 4];
    inspect(Path::new(&path), &mut counts)?;
    println!(
        "fixture={} archives={} entries={} payload_bytes={} stubs={} index_ms={:.3}",
        Path::new(&path).display(),
        counts[0],
        counts[1],
        counts[2],
        counts[3],
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}
