//! Build-time shader array sizes and runtime descriptor budget selection.
pub const SAMPLED_TABLE_TIERS: [u32; 4] = [64, 256, 1024, 4096];

pub const FRAGMENT_GEOMETRY_RESOURCES: u32 = 5;

pub fn sampled_table_capacity(device_limit: u32, max_capacity: u32) -> Option<u32> {
    SAMPLED_TABLE_TIERS.into_iter().rev().find(|&capacity| {
        capacity <= max_capacity
            && capacity.saturating_add(FRAGMENT_GEOMETRY_RESOURCES) <= device_limit
    })
}
