# bedrock-brarchive

Self-maintained, read-only BRArchive v1 parser for Minecraft Bedrock. No
third-party BRArchive implementation, unsafe code, extraction, thread pool,
or GPU dependency is used. Optional CPU acceleration uses the workspace's
exactly pinned `fearless_simd = 1.1.0`; this requires Rust 1.89.

The little-endian v1 layout is an 8-byte magic (`7d 27 25 b1 a0 52 70 26`),
32-bit entry count, 32-bit version, and 256-byte entry records. Each record holds
a one-byte UTF-8 name length, 247 name bytes, a relative 32-bit payload offset,
and 32-bit length. Offsets are relative to `16 + count * 256`. Payloads are
arbitrary bytes, including PNG and compiled MCB. Deduplicated ranges and
unsorted tables are supported; duplicate names use the last record.

```rust,no_run
use std::path::Path;
use bedrock_brarchive::{Acceleration, Archive};

let archive = Archive::open(Path::new("textures.brarchive"), Acceleration::Auto)?;
let mut bytes = Vec::new();
if archive.read_into("terrain_texture.json", &mut bytes)? {
    // Decode only the requested bytes in the consumer.
}
# Ok::<(), bedrock_brarchive::Error>(())
```

`Index::parse` validates a caller-owned buffer; `Entry::bytes` borrows its
payload without copying. `Archive::open` loads only the buffered entry table;
keyed lookups take O(log n). `read_into` reuses a caller buffer, while `copy_to`
streams larger entries. File cursor operations share one per-archive mutex.
Readers reject unknown versions, truncated/out-of-range descriptors, unsafe
relative names and invalid UTF-8. Limits: 65,536 records per container and
256 MiB per allocated payload; streaming has no payload allocation limit.
Empty names are rejected by this resource-oriented reader.

The default `simd` feature accelerates resource-name validation with Fearless
SIMD's safe CPU dispatch. CPU capabilities are detected once; archives with fewer
than 32 records and names shorter than 16 bytes use the original scalar path.
The x86 policy caps at AVX2 to avoid
AVX-512 frequency trade-offs; other supported backends and scalar fallback come
from Fearless SIMD. `Acceleration::Scalar` forces the original portable parser;
`--no-default-features` removes the SIMD dependency entirely. This acceleration
decodes/validates BRArchive metadata, not PNG/TGA or MCB payloads.

Zero-length entries are preserved by the parser. Pack consumers should treat
them as possible external-file registrations, never as valid empty textures.
Archives do not provide snapshot isolation against external modification or
checksums; successful range validation does not validate payload semantics.

BRArchive v1 has no compression step to GPU-accelerate. PNG/TGA decoding and
MCB interpretation remain consumer responsibilities. Hardware-specific paths
should be based on measured workloads, rather than added to the container parser.

Use `profile` for a paired, in-process A/B check with alternating execution order:

```powershell
cargo run -p bedrock-brarchive --example profile -- PATH_TO_PACK/__brarchive 61 --io
```

It verifies identical complete metadata for both policies before measuring small,
medium, large and full-corpus parsing; `--io` also measures opening/indexing files.
The corpus is preloaded, so these are warm-cache checks, not cold-storage results.
On the local Ryzen 7 7840H/AVX2 corpus below, five dev-profile paired runs
improved complete index parsing by 4.32–5.63%; three release-profile runs improved
it by 8.80–9.62%. File-open timings did not establish stable I/O improvement.
These measurements do not guarantee gains on other CPUs or inputs. See the
workspace [parsing audit](../../docs/PARSING_SIMD_AUDIT.md) for the retained and
rejected parsing candidates, fixture identities and validation limits.

Format evidence (cross-checked against local Minecraft 26.40 resources):

- [Minecraft 1.21.40 changelog](https://feedback.minecraft.net/hc/en-us/articles/31222183227149-Minecraft-Bedrock-Edition-1-21-40-Bundles-of-Bravery)
- [Community format analysis](https://gist.github.com/santiago046/ffc40925c5a78eb88f7b597a72aa6f0a)
- [Independent v1 format documentation](https://github.com/bedrock-crustaceans/brarchive/blob/main/FORMAT.md)

Read-only real-pack validation, without copying copyrighted game assets into the repo:

```powershell
cargo run -p bedrock-brarchive --example inspect -- PATH_TO_PACK/__brarchive
cargo run -p bedrock-block-model --example inspect_brarchive -- PATH_TO_PACK
```

The first command checks every index and range without reading payloads. The
second loads the block/atlas/geometry data and decodes an archived stone texture
after dropping the resolver. Debug timings are observations, not performance claims.

Validation on 2026-10-13 used the read-only fixture
`target/debug/BMCBL/versions/26.40/data/resource_packs/vanilla`:
434 archives, 10,273 unique entries, 4,962 empty registrations, zero index/range
errors. Model loading produced 680 block entries, 493 texture keys and 158 geometry
definitions; the embedded `stone.png` was 244 bytes and decoded to 16×16 pixels.
OS cache state was uncontrolled; this was compatibility validation, not a benchmark.
SHA-256 identifiers for reproducibility:

- `__brarchive/textures.brarchive`: `35eb0bad1d0c5edfc23b36252f23cd8ec7e44a806e090188b910c2dfb1a92072`
- `__brarchive/textures/blocks.brarchive`: `dc218292ddbf73f8f078a04b56071658db948245f9b3d31228277a4e8c9e4515`
