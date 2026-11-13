# bedrock-nbt

Bedrock NBT and classic SNBT codecs without world, LevelDB, UI or GPU dependencies.
bedrock-world depends on this library; world schemas and level.dat headers remain
in bedrock-world.

~~~rust
use bedrock_nbt::{parse_root, serialize_root, snbt};

let tag = snbt::parse(r#"{LevelName:"Old world",RandomSeed:-42L}"#)?;
let bytes = serialize_root(&tag)?;
assert_eq!(parse_root(&bytes)?, tag);
# Ok::<(), bedrock_nbt::Error>(())
~~~

## Encoding and compatibility

- parse_root / serialize_root: unnamed little-endian compound roots. Parsing reads
  the first root; parse_root_with_consumed exposes its boundary and
  parse_consecutive_roots reads all concatenated roots.
- binary::parse / binary::serialize: named roots of any type, including the
  one-byte End marker, with explicit LittleEndian or NetworkLittleEndian.
  Network Int/Long and container lengths use ZigZag VarInts; string lengths use
  unsigned VarInts; Short/Float/Double remain fixed-width little-endian.
- Pocket Edition and current Bedrock disk NBT use little-endian encoding.
  Network VarInt NBT, used since MCPE 0.16.0, is a separate variant and is never
  inferred from a file. A gameplay version is not a wire encoding.
- Preserves non-UTF-8 string payloads as ByteString and the existing project
  ShortArray extension (tag 100). Names must be UTF-8. Unknown tag IDs fail
  explicitly; unknown field names are retained.
- Empty lists accept any element tag and non-positive counts. Their value model
  stores only elements: writing normalizes empty lists to End/zero. Compound order
  is retained; duplicate names use the last value. Compound-root helpers discard
  root names; binary::NamedTag retains them.
- NbtReader / NbtView provide borrowed strings and events for disk LE only.
  An event visitor can stop without validating the remaining input.
- No Java big-endian/modified UTF-8, compression or schema conversion.
  Pre-NBT Pocket Edition raw binary files require a separate historical reader.
  Codec coverage does not imply that every historical world has been tested.

## SNBT

Supports compounds, homogeneous lists, B/I/L arrays, booleans, numeric suffixes,
and single/double quoted UTF-8 strings. Escape the delimiter and backslash with
a backslash. Namespace strings containing a colon must be quoted.
Unquoted numeric overflow remains a string; typed arrays reject mismatched types.
parse requires complete input; errors carry a byte offset.

to_string rejects End, ShortArray and raw ByteString rather than inventing
nonstandard representations. Non-finite floats use Rust's NaN/inf spelling with
a type suffix; text does not preserve NaN payload bits. This implements classic
SNBT, not the newer Java command expression language.

Depth is limited to 128, strings to 65535 bytes, ByteArray and SNBT input to 32 MiB.
Binary readers verify available bytes before allocating declared arrays/lists.
Callers must enforce their own total file/batch memory budgets.

## Optional CPU acceleration

Default features:

- simd: fearless_simd runtime dispatch scans long quoted string runs. A 64-byte prefix stays scalar; dispatch requires at least 256 remaining bytes
  and no early string delimiter. No unsafe casts, native-only build flags or
  mandatory CPU extensions; unsupported targets retain the fallback.
- parallel: independent document batches use Rayon's shared pool, retaining input
  order and per-document errors. The caller provides boundaries. No arbitrary
  splitting of recursive trees or per-document thread spawning.

Disable both with default-features = false. Small work should use sequential
calls. This library owns no GPU, world mutation or global thread pool configuration.

~~~powershell
cargo test -p bedrock-nbt --all-features
cargo test -p bedrock-nbt --no-default-features
cargo bench -p bedrock-nbt --bench snbt_scan --no-default-features
cargo bench -p bedrock-nbt --bench snbt_scan --no-default-features --features simd
~~~

## References

- [Bedrock Wiki NBT](https://wiki.bedrock.dev/nbt/nbt-in-depth.html)
- [Cloudburst NBT wire variants](https://github.com/CloudburstMC/NBT)
- [nbtx binary and SNBT reference](https://github.com/bedrock-crustaceans/nbtx)
- [Fearless SIMD](https://github.com/linebender/fearless_simd)

The disk codec and value model were extracted from this workspace's bedrock-world.
External libraries were consulted as format/API references, without depending on
their value models.
