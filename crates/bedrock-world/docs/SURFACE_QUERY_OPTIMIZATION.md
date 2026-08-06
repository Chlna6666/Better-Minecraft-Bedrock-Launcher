# Surface Query Optimization

## Baseline

The 2026-07-13 large-fixture run queried a 16x16 chunk window (256 chunks).
Before `SurfaceProjection`, exact-surface warm reads took 364 ms: 46 ms DB
time and 314 ms total decode time, including 231 ms in surface traversal.
After projection, the comparable warm pass took 205 ms: 30 ms DB time, 172 ms
total decode time, and 93 ms in surface traversal. Deferring palette counts
for surface-only requests reduced the latest warm pass to 193 ms: 43 ms DB,
147 ms decode, 28 ms subchunk parse, and 91 ms surface traversal. Fixed-layer
queries took 8-10 ms after cache reuse, with about 4-5 ms each in DB and decode.

Stage counters include microsecond totals so sub-millisecond chunk work is
preserved during aggregation.

## Current Work Shape

For every surface chunk, the loader:

1. Parses every requested paletted subchunk and every palette-entry NBT.
2. Scans all 4096 packed palette entries to build `BlockPalette::counts`.
3. Scans 256 X/Z columns from maximum Y downward. Each candidate block performs
   a BTreeMap subchunk lookup, Y conversion, storage iteration, packed-index
   extraction, and surface-role lookup.
4. Builds cloned `TerrainColumnSample` values for overlay, water, relief, and
   biome context.

This is correct, but it mixes the general chunk representation with a
surface-specific traversal.

## Optimization Sequence

### 1. Measure Before Changing Semantics

Microsecond counters are present for biome parse, subchunk parse,
surface-column traversal, and block-entity parse. The benchmark prints those
totals while the existing millisecond API fields remain coarse compatibility
diagnostics. Packed-index validation needs its own counter before counts are
made lazy.

Acceptance: the sum of fine-grained phases accounts for at least 95% of
`decode_ms` on the 256-chunk fixture.

### 2. Surface Projection Decoder

An internal `SurfaceProjection` lookup is now used only when a
`ChunkDataRequest` contains `SurfaceColumns` and no layer/cave/full-3D
requirement. It should:

- precompute `TerrainSurfaceRole` once per palette entry;
- process loaded subchunks top-down and unresolved columns in a 16x16 array;
- decode packed palette values directly in the traversal order;
- retain only the states needed for the selected surface, overlay, water, and
  relief sample;
- avoid BTreeMap lookup for each candidate block.

The public `ChunkData` surface result must remain byte-for-byte equivalent to
the current `TerrainColumnSamples` behavior, including secondary storage,
water depth, thin overlays, legacy fallback, and biome precedence.

The first projection stage replaces per-block BTreeMap lookups with compact
section slots and preserved exact-surface equivalence fixtures. It reduced
surface traversal by roughly 60% in the large fixture. The next stage is
column-batched packed-index traversal and palette-role precomputation.

### 3. Defer Counts for Surface-Only Queries

`BlockPalette::counts` requires a complete 4096-value pass for every palette
storage, but surface rendering does not consume it. Counts are now optional:
surface-only requests leave them absent, while Full 3D, CountsOnly, and
structure paths retain exact values.

Acceptance: surface-only parsing performs no separate full-volume count pass;
queries that expose counts retain their current output.

### 4. Bounded Parallelism After Projection

The existing chunk-level Rayon pool is the correct outer parallelism. Do not
parallelize individual 16x16 chunks by default: that creates small jobs and
competes with DB work. After projection reduces per-chunk overhead, tune only
the existing bounded chunk worker count and keep output order stable.

Acceptance: no nested Rayon pools; cancellation and output ordering tests pass.
