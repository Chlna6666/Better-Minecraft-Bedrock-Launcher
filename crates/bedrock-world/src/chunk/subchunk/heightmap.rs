//! Bedrock SubChunk height-map contribution helpers.
//!
//! Request-mode chunk streaming sends only part of a vertical chunk column at a time. The client
//! therefore cannot derive the complete column height map from received blocks alone. This module
//! converts the authoritative Bedrock column height map into the contribution of one absolute
//! SubChunk without allocation.

/// One SubChunk's contribution to a complete 16x16 Bedrock column height map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubChunkHeightMapContribution {
    /// The source column height map is incomplete, so no safe contribution can be produced.
    NoData,
    /// Per-column local heights in `[z][x]` order.
    ///
    /// Values follow Bedrock request-mode semantics: `-1` is below this SubChunk, `16` is above it,
    /// and `0..=15` is the local Y inside this SubChunk.
    HasData([[i8; 16]; 16]),
    /// Every column height lies above this SubChunk.
    AllTooHigh,
    /// Every column height lies below this SubChunk.
    AllTooLow,
}

impl SubChunkHeightMapContribution {
    /// Returns the 256-byte `[z][x]` contribution only for [`Self::HasData`].
    #[must_use]
    pub const fn data(&self) -> Option<&[[i8; 16]; 16]> {
        match self {
            Self::HasData(data) => Some(data),
            Self::NoData | Self::AllTooHigh | Self::AllTooLow => None,
        }
    }
}

/// Computes the height-map contribution of one absolute SubChunk Y index.
///
/// `height_map` must contain absolute world Y values in `[z][x]` order, which is the public form
/// returned by `bedrock-world` chunk-data queries. Any unknown column makes the whole contribution
/// [`SubChunkHeightMapContribution::NoData`] instead of guessing and corrupting client lighting.
#[must_use]
pub fn subchunk_height_map_contribution(
    height_map: &[[Option<i16>; 16]; 16],
    subchunk_y: i32,
) -> SubChunkHeightMapContribution {
    let section_min_y = subchunk_y.saturating_mul(16);
    let section_max_y = section_min_y.saturating_add(15);
    let mut output = [[0_i8; 16]; 16];
    let mut all_high = true;
    let mut all_low = true;

    for z in 0..16 {
        for x in 0..16 {
            let Some(height) = height_map[z][x].map(i32::from) else {
                return SubChunkHeightMapContribution::NoData;
            };
            let local = if height < section_min_y {
                -1
            } else if height > section_max_y {
                16
            } else {
                (height - section_min_y) as i8
            };
            output[z][x] = local;
            all_high &= local == 16;
            all_low &= local == -1;
        }
    }

    if all_high {
        SubChunkHeightMapContribution::AllTooHigh
    } else if all_low {
        SubChunkHeightMapContribution::AllTooLow
    } else {
        SubChunkHeightMapContribution::HasData(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_above_and_all_below_use_compact_variants() {
        let above = [[Some(80_i16); 16]; 16];
        assert_eq!(
            subchunk_height_map_contribution(&above, 0),
            SubChunkHeightMapContribution::AllTooHigh
        );

        let below = [[Some(-64_i16); 16]; 16];
        assert_eq!(
            subchunk_height_map_contribution(&below, 4),
            SubChunkHeightMapContribution::AllTooLow
        );
    }

    #[test]
    fn mixed_columns_are_relative_to_absolute_subchunk_y() {
        let mut height_map = [[Some(64_i16); 16]; 16];
        height_map[0][0] = Some(63);
        height_map[0][1] = Some(64);
        height_map[0][2] = Some(79);
        height_map[0][3] = Some(80);
        let contribution = subchunk_height_map_contribution(&height_map, 4);
        let SubChunkHeightMapContribution::HasData(data) = contribution else {
            panic!("expected mixed height-map data")
        };
        assert_eq!(data[0][0], -1);
        assert_eq!(data[0][1], 0);
        assert_eq!(data[0][2], 15);
        assert_eq!(data[0][3], 16);
    }

    #[test]
    fn unknown_column_never_guesses_lighting_height() {
        let mut height_map = [[Some(64_i16); 16]; 16];
        height_map[7][9] = None;
        assert_eq!(
            subchunk_height_map_contribution(&height_map, 4),
            SubChunkHeightMapContribution::NoData
        );
    }
}
