use super::*;
use std::{hint::black_box, time::Instant};

fn fixture(count: u32) -> RasterBoundsCache {
    let mut cache = RasterBoundsCache::default();
    for glyph_id in 0..count {
        cache.entries.insert(
            test_glyph_params(glyph_id),
            RasterBoundsEntry::new(test_raster_bounds(), u64::from(glyph_id)),
        );
    }
    cache
}

fn original_eviction(cache: &mut RasterBoundsCache, target: usize) {
    let mut candidates = cache
        .entries
        .iter()
        .map(|(params, entry)| (params.clone(), entry.last_used_epoch()))
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|(_, epoch)| *epoch);
    let remove_count = cache.entries.len() - target;
    for (params, _) in candidates.into_iter().take(remove_count) {
        cache.entries.remove(&params);
    }
}

#[test]
#[ignore = "manual release-profile A/B measurement; run without other profiling workloads"]
fn raster_bounds_eviction_profile() {
    for count in [4096u32, 16384] {
        let target = count as usize / 2;
        let mut before = Vec::new();
        let mut after = Vec::new();
        for round in 0..31 {
            for original in [round % 2 == 0, round % 2 != 0] {
                let mut cache = fixture(count);
                let start = Instant::now();
                if original {
                    original_eviction(black_box(&mut cache), black_box(target));
                } else {
                    cache.evict_lru_to_len(black_box(target));
                }
                let elapsed = start.elapsed().as_nanos();
                assert_eq!(cache.len(), target);
                for glyph_id in (count / 2)..count {
                    assert!(cache.entries.contains_key(&test_glyph_params(glyph_id)));
                }
                if original {
                    before.push(elapsed);
                } else {
                    after.push(elapsed);
                }
            }
        }
        before.sort_unstable();
        after.sort_unstable();
        println!(
            "RASTER_PRESSURE_PROFILE entries={count} rounds=31 before_ns={} after_ns={} before_scratch_bytes={} after_scratch_bytes={} shaped_or_rasterized_glyphs=0",
            before[15],
            after[15],
            count as usize * std::mem::size_of::<(RenderGlyphParams, u64)>(),
            count as usize * std::mem::size_of::<u64>()
        );
    }
}
