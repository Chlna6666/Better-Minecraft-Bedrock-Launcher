//! Versioned Bedrock block appearance colors for image and OBJ voxelization.
//!
//! The source palette is derived from the Minecraft Bedrock 26.40 vanilla resource pack. The
//! build script checks every identifier against the pinned Mojang 1.26.40.5 block manifest,
//! rejects unsupported block families, and compiles the colors into a compact embedded binary.

use std::{collections::BTreeMap, sync::LazyLock};

use bedrock_world::block::BlockState;

use crate::FlatBlockCandidate;

const PALETTE_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bedrock-block-palette.bin"));

static CANDIDATES: LazyLock<Vec<FlatBlockCandidate>> = LazyLock::new(|| {
    decode_candidates(PALETTE_DATA).expect("build script emits a validated Bedrock voxel palette")
});

/// Returns the voxelizer-owned, texture-backed Bedrock 26.40 block palette.
///
/// The source JSON is validated against a pinned Mojang block identifier manifest and compiled
/// into an embedded binary by this crate's build script. The binary is decoded once and shared by
/// Flat, relief, and OBJ conversion. Only explicit namespaced IDs with resolved vanilla texture
/// colors are present; falling blocks, concrete powder, glass panes, and glazed terracotta are
/// excluded. This does not read or modify a Minecraft world.
///
/// # Panics
///
/// Panics if the build-generated binary palette is corrupt or incompatible with this decoder.
#[must_use]
pub fn default_block_candidates() -> &'static [FlatBlockCandidate] {
    &CANDIDATES
}

fn decode_candidates(bytes: &[u8]) -> Result<Vec<FlatBlockCandidate>, String> {
    if bytes.get(..4) != Some(b"BVP1") {
        return Err("invalid Bedrock voxel palette header".to_owned());
    }
    let count_bytes = bytes
        .get(4..6)
        .ok_or_else(|| "truncated Bedrock voxel palette header".to_owned())?;
    let count = usize::from(u16::from_le_bytes([count_bytes[0], count_bytes[1]]));
    let mut offset = 6;
    let mut candidates = Vec::with_capacity(count);

    for _ in 0..count {
        let id_length = usize::from(
            *bytes
                .get(offset)
                .ok_or_else(|| "truncated Bedrock voxel palette block ID".to_owned())?,
        );
        offset += 1;
        let id_end = offset
            .checked_add(id_length)
            .ok_or_else(|| "invalid Bedrock voxel palette block ID length".to_owned())?;
        let id = std::str::from_utf8(
            bytes
                .get(offset..id_end)
                .ok_or_else(|| "truncated Bedrock voxel palette block ID".to_owned())?,
        )
        .map_err(|_| "Bedrock voxel palette contains an invalid block ID".to_owned())?
        .to_owned();
        offset = id_end;
        let top_color = read_rgba(bytes, &mut offset)?;
        let side_color = read_rgba(bytes, &mut offset)?;

        candidates.push(FlatBlockCandidate {
            state: BlockState {
                name: id,
                states: BTreeMap::new(),
                version: None,
            },
            top_color,
            side_color,
        });
    }

    if offset != bytes.len() {
        return Err("Bedrock voxel palette has trailing bytes".to_owned());
    }
    Ok(candidates)
}

fn read_rgba(bytes: &[u8], offset: &mut usize) -> Result<[u8; 4], String> {
    let end = (*offset)
        .checked_add(4)
        .ok_or_else(|| "invalid Bedrock voxel palette color offset".to_owned())?;
    let color = bytes
        .get(*offset..end)
        .ok_or_else(|| "truncated Bedrock voxel palette color".to_owned())?;
    *offset = end;
    Ok([color[0], color[1], color[2], color[3]])
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{PALETTE_DATA, decode_candidates, default_block_candidates};

    const BLOCK_ID_MANIFEST: &str = include_str!("../data/bedrock-block-ids-1.26.40.5.txt");

    #[test]
    fn palette_contains_unique_texture_backed_bedrock_ids() {
        assert!(
            BLOCK_ID_MANIFEST
                .starts_with("# Bedrock 1.26.40.5 namespaced vanilla block identifiers.")
        );
        assert!(BLOCK_ID_MANIFEST.contains("7844835b6baad4c0010f46901a4accf87413a022"));

        let official_ids = BLOCK_ID_MANIFEST
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .collect::<HashSet<_>>();
        assert_eq!(official_ids.len(), 1_415);

        let candidates = default_block_candidates();
        assert_eq!(candidates.len(), 153);
        let names = candidates
            .iter()
            .map(|candidate| candidate.state.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        let bone_block = candidates
            .iter()
            .find(|candidate| candidate.state.name == "minecraft:bone_block")
            .expect("verified Bedrock bone block is in the voxel palette");
        assert_eq!(bone_block.top_color, [209, 206, 179, 255]);
        assert_eq!(bone_block.side_color, [229, 225, 207, 255]);
        for (id, color) in [
            ("minecraft:quartz_bricks", [234, 229, 221, 255]),
            ("minecraft:smooth_quartz", [236, 230, 223, 255]),
            ("minecraft:snow", [249, 254, 254, 255]),
        ] {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.state.name == id)
                .expect("verified Bedrock opaque block is in the voxel palette");
            assert_eq!(candidate.top_color, color, "unexpected top color for {id}");
            assert_eq!(
                candidate.side_color, color,
                "unexpected side color for {id}"
            );
        }
        assert_eq!(
            names
                .iter()
                .filter(|name| name.ends_with("_concrete"))
                .count(),
            16
        );
        assert_eq!(
            names.iter().filter(|name| name.ends_with("_wool")).count(),
            16
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.ends_with("_terracotta"))
                .count(),
            16
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.ends_with("_stained_glass"))
                .count(),
            16
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.ends_with("_planks"))
                .count(),
            12
        );
        for id in [
            "minecraft:bamboo_mosaic",
            "minecraft:bamboo_planks",
            "minecraft:cherry_planks",
            "minecraft:crimson_planks",
            "minecraft:mangrove_planks",
            "minecraft:pale_oak_planks",
            "minecraft:warped_planks",
        ] {
            assert!(names.binary_search(&id).is_ok(), "missing {id}");
        }

        for candidate in candidates {
            let id = candidate.state.name.as_str();
            assert!(id.starts_with("minecraft:"));
            assert!(official_ids.contains(id));
            assert!(!id.ends_with("_concrete_powder"));
            assert!(!id.ends_with("_glass_pane"));
            assert!(!id.ends_with("_glazed_terracotta"));
            assert!(!matches!(
                id,
                "minecraft:sand"
                    | "minecraft:red_sand"
                    | "minecraft:gravel"
                    | "minecraft:suspicious_sand"
                    | "minecraft:suspicious_gravel"
                    | "minecraft:anvil"
                    | "minecraft:chipped_anvil"
                    | "minecraft:damaged_anvil"
                    | "minecraft:dragon_egg"
            ));
            assert!(
                candidate.top_color[3] == 255
                    || id == "minecraft:glass"
                    || id.ends_with("_stained_glass")
            );
        }
    }

    #[test]
    fn binary_palette_decoder_rejects_invalid_or_truncated_data() {
        assert!(decode_candidates(b"bad!").is_err());

        let mut truncated = PALETTE_DATA.to_vec();
        truncated.pop();
        assert!(decode_candidates(&truncated).is_err());
    }
}
