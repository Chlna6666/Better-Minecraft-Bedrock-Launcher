//! Paired scalar/automatic measurements of the same read-only archive corpus.
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bedrock_brarchive::{Acceleration, Archive, Index};

type Corpus = Vec<(PathBuf, Vec<u8>)>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn available_backend() -> String {
    #[cfg(feature = "simd")]
    {
        let level = fearless_simd::Level::new();
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        let level = level.as_avx2().map_or(level, fearless_simd::Level::Avx2);
        format!("{level:?}")
    }
    #[cfg(not(feature = "simd"))]
    {
        "Scalar".to_owned()
    }
}

fn load(path: &Path, corpus: &mut Corpus) -> Result<()> {
    if path.is_dir() {
        let mut paths = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            load(&path, corpus)?;
        }
    } else if path.extension().is_some_and(|ext| ext == "brarchive") {
        corpus.push((path.to_owned(), std::fs::read(path)?));
    }
    Ok(())
}

fn measure(
    rounds: usize,
    mut run: impl FnMut(Acceleration) -> Result<()>,
) -> Result<([f64; 2], [f64; 2])> {
    let mut samples: [Vec<f64>; 2] = std::array::from_fn(|_| Vec::with_capacity(rounds));
    let policies = [Acceleration::Scalar, Acceleration::Auto];
    for round in 0..=rounds {
        let order = if round % 2 == 0 { [0, 1] } else { [1, 0] };
        for index in order {
            let started = Instant::now();
            run(policies[index])?;
            if round > 0 {
                samples[index].push(started.elapsed().as_secs_f64() * 1_000_000.0);
            }
        }
    }
    for sample in &mut samples {
        sample.sort_by(f64::total_cmp);
    }
    Ok((
        samples.each_ref().map(|sample| sample[rounds / 2]),
        samples
            .each_ref()
            .map(|sample| sample[(rounds * 9 / 10).min(rounds - 1)]),
    ))
}

fn report(
    group: &str,
    count: usize,
    rounds: usize,
    run: impl FnMut(Acceleration) -> Result<()>,
) -> Result<()> {
    let (median, p90) = measure(rounds, run)?;
    println!(
        "group={group} archives={count} scalar_us={:.3} auto_us={:.3} delta_percent={:.2} scalar_p90_us={:.3} auto_p90_us={:.3}",
        median[0],
        median[1],
        (median[1] / median[0] - 1.0) * 100.0,
        p90[0],
        p90[1]
    );
    Ok(())
}

fn groups(corpus: &Corpus) -> Result<[Vec<&Vec<u8>>; 4]> {
    let mut groups: [Vec<&Vec<u8>>; 4] = std::array::from_fn(|_| Vec::new());
    let mut entries = 0;
    let mut checksum = 0xcbf2_9ce4_8422_2325u64;
    for (_, bytes) in corpus {
        let index = Index::parse(bytes, Acceleration::Scalar)?;
        if index != Index::parse(bytes, Acceleration::Auto)? {
            return Err("scalar/automatic metadata mismatch".into());
        }
        let count = index.entries().len();
        entries += count;
        let group = if count <= 16 {
            0
        } else if count <= 256 {
            1
        } else {
            2
        };
        groups[group].push(bytes);
        groups[3].push(bytes);
        for entry in index.entries() {
            for byte in entry
                .name()
                .bytes()
                .chain(entry.offset().to_le_bytes())
                .chain(entry.len().to_le_bytes())
            {
                checksum = (checksum ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            }
        }
    }
    println!(
        "archives={} entries={entries} metadata_checksum={checksum:016x} simd_feature={} cache=preloaded errors=0",
        corpus.len(),
        cfg!(feature = "simd")
    );
    Ok(groups)
}

fn main() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("usage: profile ARCHIVE_DIRECTORY [ROUNDS] [--io]")?;
    let rounds = std::env::args()
        .nth(2)
        .map(|arg| arg.parse())
        .transpose()?
        .unwrap_or(31usize);
    if !(3..=1_000).contains(&rounds) {
        return Err("rounds must be 3..=1000".into());
    }
    let mut corpus = Corpus::new();
    load(Path::new(&root), &mut corpus)?;
    println!("available_backend={}", available_backend());
    println!(
        "fixture={} rounds={rounds} order=alternating",
        Path::new(&root).display()
    );
    for (name, group) in ["small", "medium", "large", "all"]
        .into_iter()
        .zip(groups(&corpus)?)
    {
        if !group.is_empty() {
            report(name, group.len(), rounds, |policy| {
                for bytes in &group {
                    black_box(Index::parse(black_box(bytes), policy)?);
                }
                Ok(())
            })?;
        }
    }
    if std::env::args().nth(3).as_deref() == Some("--io") {
        report("open", corpus.len(), rounds, |policy| {
            for (path, _) in &corpus {
                black_box(Archive::open(black_box(path), policy)?);
            }
            Ok(())
        })?;
    }
    Ok(())
}
