//! Bounded two-channel voxel light propagation; independent of world storage and GPU APIs.

/// Validated emission and attenuation packed into one byte.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightCell(u8);

impl LightCell {
    /// Non-emitting, fully transparent cell.
    pub const AIR: Self = Self(0);
    /// Non-emitting cell blocking all incoming light.
    pub const OPAQUE: Self = Self(0xf0);
    /// Block-emitted light on the 0..=15 scale.
    #[must_use]
    pub const fn emission(self) -> u8 {
        self.0 & 15
    }
    /// Attenuation on the 0..=15 scale; 15 blocks incoming light.
    #[must_use]
    pub const fn attenuation(self) -> u8 {
        self.0 >> 4
    }
    /// Creates a cell on the 0..=15 emission/attenuation scale.
    #[must_use]
    pub fn new(emission: u8, attenuation: u8) -> Option<Self> {
        (emission <= 15 && attenuation <= 15).then_some(Self((attenuation << 4) | emission))
    }
}

/// Invalid light volume dimensions or input length.
#[derive(Debug, thiserror::Error)]
#[error("light volume must contain 1..=4194304 cells with matching input length")]
pub struct LightError;

/// Immutable sky (high nibble) and block (low nibble) light in a finite voxel volume.
///
/// Index order is X + size.X * (Y + size.Y * Z). The caller must include the complete
/// vertical columns and at least 15 cells of horizontal halo for an authoritative
/// interior result. Unknown world cells should be opaque, rather than inventing sky.
#[derive(Clone, Debug)]
pub struct LightVolume {
    size: [u32; 3],
    levels: Box<[u8]>,
}

impl LightVolume {
    /// Propagates light with six-neighbor attenuation. Only straight downward full
    /// skylight can remain at 15 through transparent cells. No world data is written.
    /// Re-solving a changed bounded region removes stale light as well as adding it.
    ///
    /// # Errors
    /// Returns [`LightError`] for empty, overflowing, oversized or mismatched input.
    pub fn solve(size: [u32; 3], cells: &[LightCell], has_sky: bool) -> Result<Self, LightError> {
        let count = size
            .into_iter()
            .try_fold(1usize, |n, s| n.checked_mul(s as usize))
            .ok_or(LightError)?;
        if size.contains(&0) || count > 4_194_304 || count != cells.len() {
            return Err(LightError);
        }
        let mut levels = cells.iter().map(|cell| cell.0 & 15).collect::<Vec<_>>();
        if has_sky {
            seed_sky(size, cells, &mut levels);
            spread(size, cells, &mut levels, 4);
        }
        spread(size, cells, &mut levels, 0);
        Ok(Self {
            size,
            levels: levels.into_boxed_slice(),
        })
    }

    /// Packed sky/block level for a cell, or `None` outside the volume.
    #[must_use]
    pub fn packed_at(&self, position: [u32; 3]) -> Option<u8> {
        if position.iter().zip(self.size).any(|(&p, n)| p >= n) {
            return None;
        }
        Some(self.levels[index(self.size, position)])
    }

    /// Packed light bytes in X/Y/Z order described on [`LightVolume`].
    #[must_use]
    pub fn levels(&self) -> &[u8] {
        &self.levels
    }
}

fn index(size: [u32; 3], p: [u32; 3]) -> usize {
    ((p[2] * size[1] + p[1]) * size[0] + p[0]) as usize
}

fn seed_sky(size: [u32; 3], cells: &[LightCell], levels: &mut [u8]) {
    for z in 0..size[2] {
        for x in 0..size[0] {
            let mut sky = 15u8;
            for y in (0..size[1]).rev() {
                let i = index(size, [x, y, z]);
                let attenuation = cells[i].0 >> 4;
                if sky != 15 || attenuation != 0 {
                    sky = sky.saturating_sub(attenuation.max(1));
                }
                levels[i] |= sky << 4;
            }
        }
    }
}

fn spread(size: [u32; 3], cells: &[LightCell], levels: &mut [u8], shift: u8) {
    let mut buckets: [Vec<usize>; 16] = std::array::from_fn(|_| Vec::new());
    for (i, value) in levels.iter().enumerate() {
        let level = (value >> shift) & 15;
        if level > 1 {
            buckets[level as usize].push(i);
        }
    }
    for level in (2u8..=15).rev() {
        while let Some(i) = buckets[level as usize].pop() {
            if (levels[i] >> shift) & 15 != level {
                continue;
            }
            let p = [
                i as u32 % size[0],
                i as u32 / size[0] % size[1],
                i as u32 / size[0] / size[1],
            ];
            for (axis, delta) in [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)] {
                let value = i64::from(p[axis]) + delta;
                if value < 0 || value >= i64::from(size[axis]) {
                    continue;
                }
                let mut next = p;
                next[axis] = value as u32;
                let n = index(size, next);
                let attenuation = cells[n].0 >> 4;
                let candidate =
                    if shift == 4 && level == 15 && axis == 1 && delta == -1 && attenuation == 0 {
                        15
                    } else {
                        level.saturating_sub(attenuation.max(1))
                    };
                if candidate > (levels[n] >> shift) & 15 {
                    levels[n] = (levels[n] & !(15 << shift)) | (candidate << shift);
                    if candidate > 1 {
                        buckets[candidate as usize].push(n);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "light_tests.rs"]
mod tests;
