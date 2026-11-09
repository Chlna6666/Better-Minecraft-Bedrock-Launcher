//! Device-limit-aware Froxel volume planning.
//!
//! This module only plans a *true 3D* GPU texture layout. It deliberately does
//! not allocate a 2D atlas, silently substitute a buffer, or dispatch a shader:
//! gfx-core's current TextureDimension ABI exposes D2 and D2Array only.
//! Native Texture3D/UAV support must be proven on every enabled backend before
//! allocation and the new ordered Graphics -> Compute -> Graphics path are used.

use gfx_core::{ComputeCapabilities, ComputeKernelProfile, Extent2d};

pub(super) const FROXEL_DEPTH_SLICES: u32 = 64;
pub(super) const MAX_FROXEL_BYTES: u64 = 64 * 1024 * 1024;
const INITIAL_TILE_PIXELS: u32 = 16;
const MAX_TILE_PIXELS: u32 = 256;
/// Two RGBA16F volumes: scattering/extinction and integrated radiance.
const VOLUME_COUNT: u64 = 2;
const BYTES_PER_RGBA16F: u64 = 8;

/// This must match the local workgroup size of the eventual compiled WGSL
/// entry points. It is a legality check, *not* a claim of optimal occupancy.
pub(super) const FROXEL_KERNEL: ComputeKernelProfile = ComputeKernelProfile {
    workgroup_size: [8, 8, 4],
    shared_memory_bytes: 0,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FroxelGrid {
    pub tile_pixels: u32,
    /// Physical texture width, height and depth; unlike a 2D atlas these
    /// dimensions correspond to x/y/z coordinates in texture_3d.
    pub extent: [u32; 3],
    pub workgroups: [u32; 3],
    /// Upper bound on uncompressed texels for the two logical volumes.
    /// Does not include native image padding, metadata, or history textures.
    pub texture_bytes: u64,
}

impl FroxelGrid {
    /// Plan a froxel grid from actual framebuffer size and compute limits.
    /// None retains the existing fragment-integrated fog without an unsafe
    /// backend override. High-resolution windows increase tile size before
    /// allocating unbounded 3D images.
    pub fn plan(extent: Extent2d, caps: ComputeCapabilities) -> Option<Self> {
        if !caps.supports_kernel(FROXEL_KERNEL) {
            return None;
        }
        let mut tile_pixels = INITIAL_TILE_PIXELS;
        loop {
            let x = extent.width().div_ceil(tile_pixels);
            let y = extent.height().div_ceil(tile_pixels);
            let z = FROXEL_DEPTH_SLICES;
            let voxels = u64::from(x)
                .checked_mul(u64::from(y))?
                .checked_mul(u64::from(z))?;
            let texture_bytes = voxels
                .checked_mul(BYTES_PER_RGBA16F)?
                .checked_mul(VOLUME_COUNT)?;
            let workgroups = [
                x.div_ceil(FROXEL_KERNEL.workgroup_size[0]),
                y.div_ceil(FROXEL_KERNEL.workgroup_size[1]),
                z.div_ceil(FROXEL_KERNEL.workgroup_size[2]),
            ];
            if texture_bytes <= MAX_FROXEL_BYTES
                && workgroups.iter().zip(caps.max_workgroups)
                    .all(|(&count, limit)| count > 0 && count <= limit)
            {
                return Some(Self {
                    tile_pixels,
                    extent: [x, y, z],
                    workgroups,
                    texture_bytes,
                });
            }
            if tile_pixels >= MAX_TILE_PIXELS {
                return None;
            }
            tile_pixels = tile_pixels.checked_mul(2)?;
        }
    }

    /// Center of a logarithmically distributed depth slice (in world units).
    /// The matching shader must use the same mapping for light injection,
    /// integration and full-resolution composite sampling.
    pub fn depth_at_slice(self, index: u32, near: f32, far: f32) -> Option<f32> {
        if index >= self.extent[2] || !near.is_finite() || !far.is_finite()
            || near <= 0.0 || far <= near
        {
            return None;
        }
        let t = (index as f32 + 0.5) / self.extent[2] as f32;
        // Log interpolation avoids overflowing far/near for otherwise
        // finite, valid clipping distances.
        Some((near.ln() * (1.0 - t) + far.ln() * t).exp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capable() -> ComputeCapabilities {
        ComputeCapabilities {
            max_workgroups: [65535, 65535, 65535],
            max_invocations_per_workgroup: 1024,
            max_workgroup_size: [1024, 1024, 64],
            max_workgroup_shared_memory_bytes: 32768,
            indirect_draw: true,
        }
    }

    #[test]
    fn full_hd_grid_uses_3d_dispatch_and_bounded_storage() {
        let grid = FroxelGrid::plan(Extent2d::new(1920, 1080).unwrap(), capable()).unwrap();
        assert_eq!(grid.extent, [120, 68, FROXEL_DEPTH_SLICES]);
        assert_eq!(grid.workgroups, [15, 9, 16]);
        assert_eq!(grid.tile_pixels, 16);
        assert!(grid.texture_bytes < MAX_FROXEL_BYTES);
    }

    #[test]
    fn ultra_high_resolution_reduces_grid_density_before_exceeding_budget() {
        let grid = FroxelGrid::plan(Extent2d::new(7680, 4320).unwrap(), capable()).unwrap();
        assert!(grid.tile_pixels > INITIAL_TILE_PIXELS);
        assert!(grid.texture_bytes <= MAX_FROXEL_BYTES);
        assert_eq!(grid.extent[2], FROXEL_DEPTH_SLICES);
    }

    #[test]
    fn unavailable_compute_limits_keep_existing_fog_path() {
        assert!(FroxelGrid::plan(
            Extent2d::new(1920, 1080).unwrap(),
            ComputeCapabilities::default()
        ).is_none());
        let mut limits = capable();
        limits.max_workgroups[2] = 1;
        assert!(FroxelGrid::plan(Extent2d::new(1920, 1080).unwrap(), limits).is_none());
    }

    #[test]
    fn logarithmic_depth_is_monotonic_and_stays_in_frustum() {
        let grid = FroxelGrid::plan(Extent2d::new(800, 600).unwrap(), capable()).unwrap();
        let mut previous = 0.0;
        for z in 0..grid.extent[2] {
            let current = grid.depth_at_slice(z, 0.1, 512.0).unwrap();
            assert!(current > previous && current < 512.0);
            previous = current;
        }
        assert!(grid.depth_at_slice(grid.extent[2], 0.1, 512.0).is_none());
        assert!(grid.depth_at_slice(0, 0.0, 512.0).is_none());
    }
}
