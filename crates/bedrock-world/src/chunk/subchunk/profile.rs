//! Opt-in, read-only measurement against the locally verified world snapshot.
use super::{SubChunkDecodeMode, SubChunkFormat, decode, decode_indices, indices};
use crate::chunk::key::{BedrockDbKey, ChunkRecordTag};
use crate::storage::{
    BedrockLevelDbStorage, StorageEntryView, StorageReadOptions, StorageScanMode,
    StorageThreadingOptions, StorageVisitorControl, WorldStorage,
};
use bytes::Bytes;
use std::collections::{BTreeMap, HashSet};
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use xxhash_rust::xxh3::Xxh3;

struct PaletteSample {
    words: Bytes,
    bits: u8,
    palette_len: usize,
}
#[derive(Default)]
struct Corpus {
    records: usize,
    chunks: HashSet<crate::ChunkPos>,
    subchunks: usize,
    errors: usize,
    versions: BTreeMap<u8, usize>,
    widths: BTreeMap<u8, usize>,
    samples: BTreeMap<u8, Vec<PaletteSample>>,
    payloads: Vec<(i8, Bytes)>,
}
impl Corpus {
    fn visit(&mut self, entry: StorageEntryView<'_>) -> crate::Result<StorageVisitorControl> {
        self.records += 1;
        let BedrockDbKey::Chunk(key) = BedrockDbKey::decode(entry.key) else {
            return Ok(StorageVisitorControl::Continue);
        };
        self.chunks.insert(key.pos);
        if key.tag != ChunkRecordTag::SubChunkPrefix {
            return Ok(StorageVisitorControl::Continue);
        }
        self.subchunks += 1;
        let value = Bytes::copy_from_slice(entry.value);
        if let Some(version) = value.first() {
            *self.versions.entry(*version).or_default() += 1;
        }
        let y = key.subchunk_y.expect("decoded SubChunk key has Y");
        match decode(y, value.clone(), SubChunkDecodeMode::PackedIndices) {
            Ok(decoded) => self.observe(decoded.format),
            Err(error) => {
                self.errors += 1;
                if self.errors <= 3 {
                    eprintln!("subchunk error key={key:?}: {error}");
                }
            }
        }
        if self.subchunks % 64 == 1 {
            self.payloads.push((y, value));
        }
        Ok(StorageVisitorControl::Continue)
    }
    fn observe(&mut self, format: SubChunkFormat) {
        let SubChunkFormat::Paletted { storages, .. } = format else {
            return;
        };
        for palette in storages {
            let packed = palette.packed_indices.expect("packed mode retains words");
            *self.widths.entry(packed.bits_per_block).or_default() += 1;
            let scalar = indices::scalar(&packed.bytes, packed.bits_per_block, packed.palette_len);
            if scalar.is_err() {
                self.errors += 1;
            }
            // Every palette must have identical values or identical first errors.
            let canonical =
                |result: crate::Result<Vec<u16>>| result.map_err(|error| error.to_string());
            assert_eq!(
                canonical(scalar),
                canonical(indices::unpack(
                    &packed.bytes,
                    packed.bits_per_block,
                    packed.palette_len
                ))
            );
            let group = self.samples.entry(packed.bits_per_block).or_default();
            if group.len() < 256 {
                group.push(PaletteSample {
                    words: packed.bytes,
                    bits: packed.bits_per_block,
                    palette_len: packed.palette_len,
                });
            }
        }
    }
}

#[test]
#[ignore = "requires the verified local world snapshot; performs a complete read-only scan"]
fn profile_world_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/resource-parse-check/fixtures/bedrock-level-20261013");
    let storage = BedrockLevelDbStorage::open_read_only(snapshot.join("db"))?;
    let mut corpus = Corpus::default();
    let scan_start = Instant::now();
    storage.for_each_prefix_ref(
        b"",
        StorageReadOptions {
            threading: StorageThreadingOptions::Single,
            scan_mode: StorageScanMode::Sequential,
            ..StorageReadOptions::default()
        },
        &mut |entry| corpus.visit(entry),
    )?;
    let mut checksum = Xxh3::new();
    for (y, bytes) in &corpus.payloads {
        checksum.update(&y.to_ne_bytes());
        checksum.update(bytes);
    }
    println!(
        "snapshot={} records={} chunks={} subchunks={} errors={} versions={:?} widths={:?} scan_ms={} payload_checksum={:032x} cache=preloaded simd_feature={}",
        snapshot.display(),
        corpus.records,
        corpus.chunks.len(),
        corpus.subchunks,
        corpus.errors,
        corpus.versions,
        corpus.widths,
        scan_start.elapsed().as_millis(),
        checksum.digest128(),
        cfg!(feature = "simd")
    );
    assert_eq!(
        corpus.errors, 0,
        "corrupt fixture is unsuitable for optimization measurements"
    );
    profile_palettes(&corpus.samples);
    profile_decode(&corpus.payloads);
    Ok(())
}

fn profile_palettes(samples: &BTreeMap<u8, Vec<PaletteSample>>) {
    for (bits, group) in samples {
        let medians = paired(31, |auto| {
            for sample in group {
                let result = if auto {
                    indices::unpack(black_box(&sample.words), sample.bits, sample.palette_len)
                } else {
                    indices::scalar(black_box(&sample.words), sample.bits, sample.palette_len)
                };
                black_box(result.expect("already validated"));
            }
        });
        println!(
            "unpack bits={bits} samples={} scalar_us={:.3} auto_us={:.3}",
            group.len(),
            medians[0].as_secs_f64() * 1e6,
            medians[1].as_secs_f64() * 1e6
        );
    }
}

fn profile_decode(payloads: &[(i8, Bytes)]) {
    for mode in [
        SubChunkDecodeMode::FullIndices,
        SubChunkDecodeMode::CountsOnly,
        SubChunkDecodeMode::PackedIndices,
    ] {
        for (y, bytes) in payloads {
            assert_eq!(
                decode_indices::<false>(*y, bytes.clone(), mode).unwrap(),
                decode(*y, bytes.clone(), mode).unwrap()
            );
        }
        let medians = paired(17, |auto| {
            for (y, bytes) in payloads {
                let result = if auto {
                    decode(*y, black_box(bytes.clone()), mode)
                } else {
                    decode_indices::<false>(*y, black_box(bytes.clone()), mode)
                };
                black_box(result.expect("already validated"));
            }
        });
        println!(
            "decode mode={mode:?} samples={} scalar_ms={:.3} auto_ms={:.3}",
            payloads.len(),
            medians[0].as_secs_f64() * 1e3,
            medians[1].as_secs_f64() * 1e3
        );
    }
}

fn paired(rounds: usize, mut operation: impl FnMut(bool)) -> [Duration; 2] {
    let mut timings: [Vec<Duration>; 2] = std::array::from_fn(|_| Vec::with_capacity(rounds));
    for round in 0..=rounds {
        for variant in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
            let start = Instant::now();
            operation(variant == 1);
            if round > 0 {
                timings[variant].push(start.elapsed());
            }
        }
    }
    timings.map(|mut samples| {
        samples.sort_unstable();
        samples[rounds / 2]
    })
}
