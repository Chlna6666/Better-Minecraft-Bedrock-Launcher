//! Read-only full block-model/texture-atlas/geometry loading comparison.
use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use bedrock_block_model::BlockModelRepository;

fn checksum(models: &BlockModelRepository) -> Result<u64, serde_json::Error> {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut add = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
        }
    };
    for (name, block) in &models.blocks {
        add(name.as_bytes());
        add(&serde_json::to_vec(&block.raw)?);
    }
    for (name, texture) in &models.terrain_textures.textures {
        add(name.as_bytes());
        add(&serde_json::to_vec(&texture.paths)?);
    }
    for (name, geometry) in &models.geometries.geometries {
        add(name.as_bytes());
        add(&serde_json::to_vec(&geometry.raw)?);
    }
    Ok(hash)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("usage: profile_pack PACK_ROOT [ROUNDS]")?;
    let rounds = std::env::args()
        .nth(2)
        .map(|arg| arg.parse())
        .transpose()?
        .unwrap_or(31usize);
    if !(3..=1_000).contains(&rounds) {
        return Err("rounds must be 3..=1000".into());
    }
    let baseline = BlockModelRepository::load_packs([Path::new(&root)])?;
    let mut samples = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let started = Instant::now();
        let models = black_box(BlockModelRepository::load_packs([Path::new(&root)])?);
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        if models != baseline {
            return Err("model output changed between loads".into());
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "fixture={} rounds={rounds} blocks={} textures={} geometries={} checksum={:016x} median_ms={:.3} p90_ms={:.3} cache=warm errors=0",
        Path::new(&root).display(),
        baseline.blocks.len(),
        baseline.terrain_textures.textures.len(),
        baseline.geometries.geometries.len(),
        checksum(&baseline)?,
        samples[rounds / 2],
        samples[rounds * 9 / 10]
    );
    Ok(())
}
