//! Strict offline loader for the pinned Bedrock BlockState migration corpus.
//!
//! Applications can distribute the CC0 corpus as normal resources instead of duplicating roughly a
//! megabyte of migration data inside every binary. Files are validated against their pinned Git blob
//! identities before parsing, then the raw buffers are released and migration uses owned structures.

use super::{
    AuthoritativeBlockStateCatalog, BlockStateSchemaSource, BlockStateStorageVersion,
    LegacyNumericBlockStateTable, PINNED_BLOCK_STATE_SCHEMA_FILES,
    load_pinned_block_state_catalog, load_pinned_block_state_catalog_for_target,
};
use crate::error::{BedrockWorldError, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// One immutable file expected in the pinned BlockState migration resource bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedCorpusFileSpec {
    /// Path relative to the corpus root.
    pub path: &'static str,
    /// Expected Git blob object id (SHA-1 of `blob <size>\0<contents>`).
    pub git_blob_sha1: &'static str,
    /// Exact byte length of the pinned file.
    pub size: usize,
}

/// Complete file manifest for PocketMine `bedrock-block-upgrade-schema` 5.2.0 at the pinned commit.
pub const PINNED_BLOCK_MIGRATION_CORPUS_FILES: &[PinnedCorpusFileSpec] = &[
    file("LICENSE", "0e259d42c996742e9e3cba14c677129b2c1b6311", 7048),
    file("block_legacy_id_map.json", "e322f7067c39c7b9331093cad1d7d990edaeae1c", 19660),
    file("id_meta_to_nbt/1.9.0.bin", "8a7250d38ce159e758aef104fa2619f5985f273a", 366625),
    file("id_meta_to_nbt/1.12.0.bin", "60fdd271027423fc0cba356230598294b5b1769a", 325472),
    file("nbt_upgrade_schema/0001_1.9.0_to_1.10.0.json", "b7a1c169185403db9a69a8fd305af5dd45eb1531", 52603),
    file("nbt_upgrade_schema/0011_1.10.0_to_1.12.0.json", "652e06bda2cec6cff47615eff302dc383ab23602", 2431),
    file("nbt_upgrade_schema/0021_1.12.0_to_1.13.0.json", "36d79000f98fdf9d8a3934b7759987262cf534bc", 24079),
    file("nbt_upgrade_schema/0031_1.13.0_to_1.14.0.json", "f363e3152811d774e2b2b865d7a0b50ec36b41a5", 213),
    file("nbt_upgrade_schema/0041_1.14.0_to_1.16.0.57_beta.json", "cb6da250890d671cc6cb7a99d29a658dcf0f892d", 736),
    file("nbt_upgrade_schema/0051_1.16.0.57_beta_to_1.16.0.59_beta.json", "fe4917c8c2f8f70cfaab85112ac115242771d9b3", 707),
    file("nbt_upgrade_schema/0061_1.16.0.59_beta_to_1.16.0.68_beta.json", "1b7cc4e2b436fc629b2f2f0d6f82650a82abb426", 1693),
    file("nbt_upgrade_schema/0071_1.16.0_to_1.16.100.json", "2aaa532e389281055b396ed00f40415df4beaac9", 1475),
    file("nbt_upgrade_schema/0081_1.16.200_to_1.16.210.json", "988f4a03ae819a38dc7797def3394a8483a2a2fb", 472),
    file("nbt_upgrade_schema/0091_1.17.10_to_1.17.30.json", "7c9dbbb414d221eac0c3a23de399bcb7a56eaf5f", 385),
    file("nbt_upgrade_schema/0101_1.17.30_to_1.17.40.json", "f6b11c869beb5525a527e8a78bf2ac20cc6dc725", 256),
    file("nbt_upgrade_schema/0111_1.18.0_to_1.18.10.json", "d21428546d69b9949b4d46c6f0566dd8e2860f48", 9015),
    file("nbt_upgrade_schema/0121_1.18.10_to_1.18.20.27_beta.json", "025916a87eb24b86cab30eff279067ea360564b4", 600),
    file("nbt_upgrade_schema/0131_1.18.20.27_beta_to_1.18.30.json", "ccb04ca8e00aa62a7bc37f18b4daf3c2c42d9168", 751),
    file("nbt_upgrade_schema/0141_1.18.30_to_1.19.0.34_beta.json", "55048921dfcf6300e0d2c90e8a2719fa9b0c25a6", 1573),
    file("nbt_upgrade_schema/0151_1.19.0.34_beta_to_1.19.20.json", "73fa3000352d6eaed71f7bd3da584e119fc00361", 271),
    file("nbt_upgrade_schema/0161_1.19.50_to_1.19.60.26_beta.json", "05a59baf2fcf4d6c291087e1cb42fd50be952d66", 233),
    file("nbt_upgrade_schema/0171_1.19.60_to_1.19.70.26_beta.json", "bff8a2c18475cb8506cf6d6842338be2740fa72b", 387),
    file("nbt_upgrade_schema/0181_1.19.70_to_1.19.80.24_beta.json", "6232cd83b140bdf074ecbd945e3f134b37b7535b", 606),
    file("nbt_upgrade_schema/0191_1.19.80.24_beta_to_1.20.0.23_beta.json", "77ae52fd6c6451da76d00ea75bd544f08ee55f3b", 4186),
    file("nbt_upgrade_schema/0201_1.20.0.23_beta_to_1.20.10.24_beta.json", "4afc49d6f35218296bd8cbee022894af6bc09e17", 2098),
    file("nbt_upgrade_schema/0211_1.20.10.24_beta_to_1.20.20.23_beta.json", "1fea99f5c6d2b6b4187ac396a556dfddfbc98f1e", 14819),
    file("nbt_upgrade_schema/0221_1.20.20.23_beta_to_1.20.30.22_beta.json", "24f74fe7ca851543ab6d21e6ac2e67f5c3507eac", 6471),
    file("nbt_upgrade_schema/0231_1.20.30.22_beta_to_1.20.40.24_beta.json", "3c42d33f3318c1603fbf2529208b6459c8e0bda5", 2205),
    file("nbt_upgrade_schema/0241_1.20.40.24_beta_to_1.20.50.23_beta.json", "e79ccbe5205e455ce52b88dd8ebacb7ff216cbaf", 666),
    file("nbt_upgrade_schema/0251_1.20.50.23_beta_to_1.20.60.26_beta.json", "873cf64310b22f94e6dc70b3def226390eb10157", 691),
    file("nbt_upgrade_schema/0261_1.20.60.26_beta_to_1.20.70.24_beta.json", "b6435fb3600e3819d414df06e9c31167ae41fb17", 1919),
    file("nbt_upgrade_schema/0271_1.20.70.24_beta_to_1.20.80.24_beta.json", "b005ed68afa6fd5a3e3264b1f84e13b39f5b51f2", 1699),
    file("nbt_upgrade_schema/0281_1.20.80.24_beta_to_1.21.0.25_beta.json", "a38d54f9e6cf570db7448920559410b35267c209", 2813),
    file("nbt_upgrade_schema/0291_1.21.0.25_beta_to_1.21.20.24_beta.json", "dafe1ee81d2c312ff8733de008e4e50b6a73acf4", 9139),
    file("nbt_upgrade_schema/0301_1.21.20.24_beta_to_1.21.30.24_beta.json", "246712a14ad6c12475f0e705f0f650371fac5841", 2467),
    file("nbt_upgrade_schema/0311_1.21.30.24_beta_to_1.21.40.25_beta.json", "269b5b8f0f559e5243941b5ebadc4aa64ff69709", 2440),
    file("nbt_upgrade_schema/0321_1.21.50.29_beta_to_1.21.60.28_beta.json", "8bb48e0db13bccc7d1a9a63e71ab0f60b2608b06", 9130),
    file("nbt_upgrade_schema/0331_1.21.100.23_beta_to_1.21.110.26_beta.json", "0641aeab0f616128857653149ff3e110756c1ba9", 338),
];

const fn file(path: &'static str, git_blob_sha1: &'static str, size: usize) -> PinnedCorpusFileSpec {
    PinnedCorpusFileSpec {
        path,
        git_blob_sha1,
        size,
    }
}

/// Fully parsed resources needed for authoritative historical BlockState migration.
#[derive(Debug)]
pub struct PinnedBlockMigrationBundle {
    catalog: AuthoritativeBlockStateCatalog,
    legacy_numeric_1_9: LegacyNumericBlockStateTable,
    legacy_numeric_1_12: LegacyNumericBlockStateTable,
}

impl PinnedBlockMigrationBundle {
    /// Returns the parsed versioned BlockState migration catalog.
    #[must_use]
    pub const fn catalog(&self) -> &AuthoritativeBlockStateCatalog {
        &self.catalog
    }

    /// Returns the pinned 1.9 numeric ID/meta mapping table.
    #[must_use]
    pub const fn legacy_numeric_1_9(&self) -> &LegacyNumericBlockStateTable {
        &self.legacy_numeric_1_9
    }

    /// Returns the pinned 1.12 numeric ID/meta mapping table.
    #[must_use]
    pub const fn legacy_numeric_1_12(&self) -> &LegacyNumericBlockStateTable {
        &self.legacy_numeric_1_12
    }

    /// Returns the numeric resolver appropriate for this bundle's selected target schema.
    #[must_use]
    pub fn legacy_numeric_for_target(&self) -> &LegacyNumericBlockStateTable {
        let first_1_12 = BlockStateStorageVersion::from_components(1, 12, 0, 1).raw();
        if self.target_block_state_version() >= first_1_12 {
            &self.legacy_numeric_1_12
        } else {
            &self.legacy_numeric_1_9
        }
    }

    /// Returns the BlockState storage version produced by this bundle.
    #[must_use]
    pub const fn target_block_state_version(&self) -> i32 {
        self.catalog.output_version().raw()
    }
}

/// Verifies every file in a pinned resource directory without parsing the migration data.
pub fn verify_pinned_block_migration_corpus(root: impl AsRef<Path>) -> Result<()> {
    let _ = read_verified_corpus(root.as_ref())?;
    Ok(())
}

/// Loads, verifies and parses a bundle targeting the newest schema represented by the pinned corpus.
pub fn load_pinned_block_migration_bundle_from_dir(
    root: impl AsRef<Path>,
) -> Result<PinnedBlockMigrationBundle> {
    load_bundle(root.as_ref(), None)
}

/// Loads, verifies and parses a bundle bound to one exact historical schema endpoint.
///
/// The target is subject to the same rules as [`load_pinned_block_state_catalog_for_target`]: it must
/// be an authoritative schema endpoint represented by the complete pinned corpus.
pub fn load_pinned_block_migration_bundle_for_target_from_dir(
    root: impl AsRef<Path>,
    target_version: BlockStateStorageVersion,
) -> Result<PinnedBlockMigrationBundle> {
    load_bundle(root.as_ref(), Some(target_version))
}

fn load_bundle(
    root: &Path,
    target_version: Option<BlockStateStorageVersion>,
) -> Result<PinnedBlockMigrationBundle> {
    let mut files = read_verified_corpus(root)?;
    let legacy_id_map = take_utf8(&mut files, "block_legacy_id_map.json")?;
    let numeric_1_9 = take_file(&mut files, "id_meta_to_nbt/1.9.0.bin")?;
    let numeric_1_12 = take_file(&mut files, "id_meta_to_nbt/1.12.0.bin")?;

    let mut schema_documents = Vec::<(&'static str, String)>::with_capacity(
        PINNED_BLOCK_STATE_SCHEMA_FILES.len(),
    );
    for &name in PINNED_BLOCK_STATE_SCHEMA_FILES {
        let path = format!("nbt_upgrade_schema/{name}");
        schema_documents.push((name, take_utf8(&mut files, &path)?));
    }
    let sources = schema_documents
        .iter()
        .map(|(name, json)| BlockStateSchemaSource {
            name: *name,
            json: json.as_str(),
        })
        .collect::<Vec<_>>();

    let catalog = if let Some(target) = target_version {
        load_pinned_block_state_catalog_for_target(&sources, target)?
    } else {
        load_pinned_block_state_catalog(&sources)?
    };
    let legacy_numeric_1_9 = LegacyNumericBlockStateTable::parse(&numeric_1_9, &legacy_id_map)?;
    let legacy_numeric_1_12 = LegacyNumericBlockStateTable::parse(&numeric_1_12, &legacy_id_map)?;

    Ok(PinnedBlockMigrationBundle {
        catalog,
        legacy_numeric_1_9,
        legacy_numeric_1_12,
    })
}

fn read_verified_corpus(root: &Path) -> Result<BTreeMap<&'static str, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for spec in PINNED_BLOCK_MIGRATION_CORPUS_FILES {
        let bytes = fs::read(root.join(spec.path))?;
        if bytes.len() != spec.size {
            return Err(BedrockWorldError::Validation(format!(
                "pinned corpus file {} has {} bytes, expected {}",
                spec.path,
                bytes.len(),
                spec.size
            )));
        }
        let actual = git_blob_sha1_hex(&bytes);
        if actual != spec.git_blob_sha1 {
            return Err(BedrockWorldError::Validation(format!(
                "pinned corpus file {} has Git blob {}, expected {}",
                spec.path, actual, spec.git_blob_sha1
            )));
        }
        files.insert(spec.path, bytes);
    }
    Ok(files)
}

fn take_file(
    files: &mut BTreeMap<&'static str, Vec<u8>>,
    path: &str,
) -> Result<Vec<u8>> {
    files.remove(path).ok_or_else(|| {
        BedrockWorldError::Validation(format!("verified corpus lost required file {path}"))
    })
}

fn take_utf8(
    files: &mut BTreeMap<&'static str, Vec<u8>>,
    path: &str,
) -> Result<String> {
    String::from_utf8(take_file(files, path)?).map_err(|error| {
        BedrockWorldError::Validation(format!("pinned corpus file {path} is not UTF-8: {error}"))
    })
}

fn git_blob_sha1_hex(bytes: &[u8]) -> String {
    let header = format!("blob {}\0", bytes.len());
    let mut sha1 = Sha1::new();
    sha1.update(header.as_bytes());
    sha1.update(bytes);
    let digest = sha1.finalize();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(40);
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[derive(Debug, Clone)]
struct Sha1 {
    state: [u32; 5],
    bytes_seen: u64,
    buffer: [u8; 64],
    buffer_len: usize,
}

impl Sha1 {
    const fn new() -> Self {
        Self {
            state: [
                0x6745_2301,
                0xefcd_ab89,
                0x98ba_dcfe,
                0x1032_5476,
                0xc3d2_e1f0,
            ],
            bytes_seen: 0,
            buffer: [0; 64],
            buffer_len: 0,
        }
    }

    fn update(&mut self, mut input: &[u8]) {
        self.bytes_seen = self.bytes_seen.saturating_add(input.len() as u64);
        if self.buffer_len != 0 {
            let copy_len = (64 - self.buffer_len).min(input.len());
            self.buffer[self.buffer_len..self.buffer_len + copy_len]
                .copy_from_slice(&input[..copy_len]);
            self.buffer_len += copy_len;
            input = &input[copy_len..];
            if self.buffer_len == 64 {
                let block = self.buffer;
                self.process_block(&block);
                self.buffer_len = 0;
            }
        }
        while input.len() >= 64 {
            let block: &[u8; 64] = input[..64]
                .try_into()
                .expect("64-byte SHA-1 block slice");
            self.process_block(block);
            input = &input[64..];
        }
        if !input.is_empty() {
            self.buffer[..input.len()].copy_from_slice(input);
            self.buffer_len = input.len();
        }
    }

    fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.bytes_seen.wrapping_mul(8);
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..].fill(0);
            let block = self.buffer;
            self.process_block(&block);
            self.buffer = [0; 64];
            self.buffer_len = 0;
        }
        self.buffer[self.buffer_len..56].fill(0);
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buffer;
        self.process_block(&block);

        let mut digest = [0_u8; 20];
        for (index, word) in self.state.iter().enumerate() {
            digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    fn process_block(&mut self, block: &[u8; 64]) {
        let mut words = [0_u32; 80];
        for (index, bytes) in block.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().expect("four SHA-1 bytes"));
        }
        for index in 16..80 {
            words[index] = (words[index - 3]
                ^ words[index - 8]
                ^ words[index - 14]
                ^ words[index - 16])
                .rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (index, word) in words.iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn manifest_has_unique_complete_paths() {
        assert_eq!(PINNED_BLOCK_MIGRATION_CORPUS_FILES.len(), 38);
        let paths = PINNED_BLOCK_MIGRATION_CORPUS_FILES
            .iter()
            .map(|file| file.path)
            .collect::<BTreeSet<_>>();
        assert_eq!(paths.len(), PINNED_BLOCK_MIGRATION_CORPUS_FILES.len());
        assert_eq!(
            paths
                .iter()
                .filter(|path| path.starts_with("nbt_upgrade_schema/"))
                .count(),
            PINNED_BLOCK_STATE_SCHEMA_FILES.len()
        );
    }

    #[test]
    fn git_blob_sha1_matches_pinned_real_schema() {
        let bytes = include_bytes!(
            "../../../tests/fixtures/blockstate-schema/0011_1.10.0_to_1.12.0.json"
        );
        assert_eq!(
            git_blob_sha1_hex(bytes),
            "652e06bda2cec6cff47615eff302dc383ab23602"
        );
    }
}
