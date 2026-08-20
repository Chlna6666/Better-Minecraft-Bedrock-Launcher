//! Preservation-first migration for historical Minecraft Bedrock saved items.
//!
//! Saved items differ from BlockStates: there is no universal Mojang item schema version stored in
//! every item NBT compound. Consequently this module follows the authoritative data model used by
//! PocketMine's BedrockItemUpgradeSchema: legacy numeric IDs are first lifted to string IDs, block
//! items are resolved through the block domain, and ordinary string-ID items run every ordered
//! `id_meta_upgrade_schema` document.

use crate::block::{BlockState, BlockStateMigrator};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use indexmap::IndexMap;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Pinned upstream BedrockItemUpgradeSchema commit.
pub const PINNED_ITEM_UPGRADE_SCHEMA_COMMIT: &str = "e19685d2e7e76eb7446115c556df34e5d627d072";
/// Git tree backing [`PINNED_ITEM_UPGRADE_SCHEMA_COMMIT`].
pub const PINNED_ITEM_UPGRADE_SCHEMA_TREE: &str = "33ea604960ce4182c4113dce948603638ce69cee";

/// Complete ordered item ID/meta upgrade schema list at the pinned commit.
pub const PINNED_ITEM_SCHEMA_FILES: &[&str] = &[
    "0001_1.6_beta_to_1.6.0.json",
    "0011_1.11.4_to_1.12.0.json",
    "0021_1.16.0_to_1.16.100.json",
    "0031_1.16.100_to_1.16.200.json",
    "0041_1.16.200_to_1.17.30.json",
    "0051_1.17.40_to_1.18.0.json",
    "0061_1.18.0_to_1.18.10.json",
    "0071_1.18.10_to_1.18.30.json",
    "0081_1.18.30_to_1.19.30.34_beta.json",
    "0091_1.19.60_to_1.19.70.26_beta.json",
    "0101_1.19.70_to_1.19.80.24_beta.json",
    "0111_1.19.80_to_1.20.0.23_beta.json",
    "0121_1.20.0.23_beta_to_1.20.10.24_beta.json",
    "0131_1.20.10.24_beta_to_1.20.20.23_beta.json",
    "0141_1.20.20.23_beta_to_1.20.30.22_beta.json",
    "0151_1.20.30.22_beta_to_1.20.50.23_beta.json",
    "0161_1.20.50.23_beta_to_1.20.60.26_beta.json",
    "0171_1.20.60.26_beta_to_1.20.70.24_beta.json",
    "0181_1.20.70.24_beta_to_1.20.80.24_beta.json",
    "0191_1.20.80.24_beta_to_1.21.0.25_beta.json",
    "0201_1.21.0.25_beta_to_1.21.20.24_beta.json",
    "0211_1.21.20.24_beta_to_1.21.30.24_beta.json",
    "0221_1.21.30.24_beta_to_1.21.40.25.json",
    "0231_1.21.40.25_to_1.21.50.29_beta.json",
    "0241_1.21.50.29_beta_to_1.21.100.23_beta.json",
    "0251_1.21.100.23_beta_to_1.21.110.26_beta.json",
    "0261_1.26.10_to_1.26.20.json",
];

/// One immutable file expected in the pinned item migration corpus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedItemCorpusFileSpec {
    /// Path relative to the item corpus root.
    pub path: &'static str,
    /// Expected Git blob SHA-1.
    pub git_blob_sha1: &'static str,
    /// Exact byte length.
    pub size: usize,
}

const fn file(
    path: &'static str,
    git_blob_sha1: &'static str,
    size: usize,
) -> PinnedItemCorpusFileSpec {
    PinnedItemCorpusFileSpec {
        path,
        git_blob_sha1,
        size,
    }
}

/// Exact file manifest for the pinned CC0 item migration corpus.
pub const PINNED_ITEM_MIGRATION_CORPUS_FILES: &[PinnedItemCorpusFileSpec] = &[
    file("LICENSE", "0e259d42c996742e9e3cba14c677129b2c1b6311", 7048),
    file(
        "item_legacy_id_map.json",
        "f8bf5c2219543ac038d5a440a08a9fca528a5423",
        28615,
    ),
    file(
        "1.12.0_item_id_to_block_id_map.json",
        "7a3ed6efd474c02d67022441b16863c2db1bf53d",
        27382,
    ),
    file(
        "id_meta_upgrade_schema/0001_1.6_beta_to_1.6.0.json",
        "8fdd19e64a1b3b1be1cbdbd0568714776f92f4a5",
        405,
    ),
    file(
        "id_meta_upgrade_schema/0011_1.11.4_to_1.12.0.json",
        "824bddcdf9c61e25752cf75be2f24ec9197b029d",
        1333,
    ),
    file(
        "id_meta_upgrade_schema/0021_1.16.0_to_1.16.100.json",
        "ce78553351d8e3eff77a67a30836e6e8babe0f17",
        7808,
    ),
    file(
        "id_meta_upgrade_schema/0031_1.16.100_to_1.16.200.json",
        "1bdda6e2dd930a8d6b521ac3971dbcf13ac550be",
        95,
    ),
    file(
        "id_meta_upgrade_schema/0041_1.16.200_to_1.17.30.json",
        "84e9a4e9f522d6d0373ca5e65e1157021310cda7",
        248,
    ),
    file(
        "id_meta_upgrade_schema/0051_1.17.40_to_1.18.0.json",
        "3b4ea9b84f7f8c07c0725acfefa51a16bf368441",
        89,
    ),
    file(
        "id_meta_upgrade_schema/0061_1.18.0_to_1.18.10.json",
        "5221d9a2f3c17764e4a7c8d495c8791ef7d4a872",
        359,
    ),
    file(
        "id_meta_upgrade_schema/0071_1.18.10_to_1.18.30.json",
        "3269f83ee5169e2eed4bf66c774edb8965377802",
        756,
    ),
    file(
        "id_meta_upgrade_schema/0081_1.18.30_to_1.19.30.34_beta.json",
        "4c3537a0c6e45fa6e9804c49a153f5b9ff58bdf1",
        1155,
    ),
    file(
        "id_meta_upgrade_schema/0091_1.19.60_to_1.19.70.26_beta.json",
        "b674687f6309fa933f24c7dc30939dd12beb5e38",
        1313,
    ),
    file(
        "id_meta_upgrade_schema/0101_1.19.70_to_1.19.80.24_beta.json",
        "f79b82b3bdcfeaa2e1940350bfbc27eb2296ba86",
        1149,
    ),
    file(
        "id_meta_upgrade_schema/0111_1.19.80_to_1.20.0.23_beta.json",
        "5668a17b8431ff51fb7e6ecd19d08862ba40cab8",
        1346,
    ),
    file(
        "id_meta_upgrade_schema/0121_1.20.0.23_beta_to_1.20.10.24_beta.json",
        "2d7a6db559f59e6fe69c9cef05e5cc1fbed3fe94",
        1634,
    ),
    file(
        "id_meta_upgrade_schema/0131_1.20.10.24_beta_to_1.20.20.23_beta.json",
        "eadfe8e865ad4908274120dd1f280aa3e3b8bafe",
        1839,
    ),
    file(
        "id_meta_upgrade_schema/0141_1.20.20.23_beta_to_1.20.30.22_beta.json",
        "2c3a27dac8a1a1ff2ba6f4889abe27b039a5a215",
        1748,
    ),
    file(
        "id_meta_upgrade_schema/0151_1.20.30.22_beta_to_1.20.50.23_beta.json",
        "30e368f5b59f3d716653121c2f2a74062a838086",
        629,
    ),
    file(
        "id_meta_upgrade_schema/0161_1.20.50.23_beta_to_1.20.60.26_beta.json",
        "31f762c4d0fc4b5d00bcbdd1a4e35122f5ea1067",
        2231,
    ),
    file(
        "id_meta_upgrade_schema/0171_1.20.60.26_beta_to_1.20.70.24_beta.json",
        "453199e25d9a4f4becda79197cb88c5a612e19c6",
        1435,
    ),
    file(
        "id_meta_upgrade_schema/0181_1.20.70.24_beta_to_1.20.80.24_beta.json",
        "fb05350e23a589297d9dcc87eebbae8086e97235",
        1412,
    ),
    file(
        "id_meta_upgrade_schema/0191_1.20.80.24_beta_to_1.21.0.25_beta.json",
        "9372668f99e1816637db4b07973502df0863ab6b",
        1686,
    ),
    file(
        "id_meta_upgrade_schema/0201_1.21.0.25_beta_to_1.21.20.24_beta.json",
        "d160bb5dad4754e5c9b63f28ebdbe0046b0d05e3",
        5783,
    ),
    file(
        "id_meta_upgrade_schema/0211_1.21.20.24_beta_to_1.21.30.24_beta.json",
        "2c81f42f452f6ad4077f65d824b6be2afed31eea",
        2777,
    ),
    file(
        "id_meta_upgrade_schema/0221_1.21.30.24_beta_to_1.21.40.25.json",
        "4e1c0591b0f5b1519fab0ee9478e7184feb48fa8",
        378,
    ),
    file(
        "id_meta_upgrade_schema/0231_1.21.40.25_to_1.21.50.29_beta.json",
        "18674d141cdb508b51513fd2885f56b6635a49e5",
        301,
    ),
    file(
        "id_meta_upgrade_schema/0241_1.21.50.29_beta_to_1.21.100.23_beta.json",
        "1f770197f365def72ab6d36e2d5ad012d915cf7f",
        351,
    ),
    file(
        "id_meta_upgrade_schema/0251_1.21.100.23_beta_to_1.21.110.26_beta.json",
        "e943e53405c3a0ccaa7c677728e8665a30af0b6a",
        79,
    ),
    file(
        "id_meta_upgrade_schema/0261_1.26.10_to_1.26.20.json",
        "7b78d369f1ae0e3d485cae3437affa92baefb230",
        430,
    ),
];

/// Borrowed authoritative item ID/meta schema source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemSchemaSource<'a> {
    /// Schema filename, including its numeric priority prefix.
    pub name: &'a str,
    /// UTF-8 JSON source.
    pub json: &'a str,
}

#[derive(Debug, Clone)]
struct ItemUpgradeSchema {
    id: u32,
    renamed_ids: BTreeMap<String, String>,
    remapped_metas: BTreeMap<String, BTreeMap<i32, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemUpgradeSchemaDocument {
    #[serde(default, rename = "renamedIds")]
    renamed_ids: BTreeMap<String, String>,
    #[serde(default, rename = "remappedMetas")]
    remapped_metas: BTreeMap<String, BTreeMap<String, String>>,
}

/// Canonical string item identifier and auxiliary metadata pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemIdentity {
    /// Namespaced item identifier.
    pub name: String,
    /// Item metadata value remaining after all schema rewrites.
    pub meta: i32,
}

/// Parsed authoritative item migration data.
#[derive(Debug, Clone)]
pub struct AuthoritativeItemMigrationCatalog {
    legacy_numeric_ids: BTreeMap<i32, String>,
    item_to_block_1_12: BTreeMap<String, String>,
    schemas: Vec<ItemUpgradeSchema>,
}

impl AuthoritativeItemMigrationCatalog {
    /// Builds an item migration catalogue from immutable resource documents.
    pub fn from_sources(
        legacy_item_id_map_json: &str,
        item_to_block_1_12_json: &str,
        sources: &[ItemSchemaSource<'_>],
    ) -> Result<Self> {
        let legacy_source: BTreeMap<String, i32> = serde_json::from_str(legacy_item_id_map_json)
            .map_err(|error| validation(format!("invalid legacy item id map: {error}")))?;
        let mut legacy_numeric_ids = BTreeMap::<i32, String>::new();
        for (name, numeric_id) in legacy_source {
            match legacy_numeric_ids.entry(numeric_id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(name);
                }
                std::collections::btree_map::Entry::Occupied(entry) => {
                    if entry.get() != &name {
                        return Err(validation(format!(
                            "legacy item id {numeric_id} maps to both {} and {name}",
                            entry.get()
                        )));
                    }
                }
            }
        }
        let item_to_block_1_12: BTreeMap<String, String> =
            serde_json::from_str(item_to_block_1_12_json)
                .map_err(|error| validation(format!("invalid 1.12 item-to-block map: {error}")))?;

        let mut schemas = Vec::with_capacity(sources.len());
        let mut ids = BTreeSet::new();
        for source in sources {
            let id = schema_id(source.name)?;
            if !ids.insert(id) {
                return Err(validation(format!("duplicate item upgrade schema id {id}")));
            }
            let document: ItemUpgradeSchemaDocument =
                serde_json::from_str(source.json).map_err(|error| {
                    validation(format!(
                        "invalid item upgrade schema {}: {error}",
                        source.name
                    ))
                })?;
            let mut remapped_metas = BTreeMap::new();
            for (name, values) in document.remapped_metas {
                let mut parsed = BTreeMap::new();
                for (raw_meta, target) in values {
                    let meta = raw_meta.parse::<i32>().map_err(|error| {
                        validation(format!(
                            "item schema {} has invalid metadata key {raw_meta:?}: {error}",
                            source.name
                        ))
                    })?;
                    parsed.insert(meta, target);
                }
                remapped_metas.insert(name, parsed);
            }
            schemas.push(ItemUpgradeSchema {
                id,
                renamed_ids: document.renamed_ids,
                remapped_metas,
            });
        }
        schemas.sort_by_key(|schema| schema.id);
        Ok(Self {
            legacy_numeric_ids,
            item_to_block_1_12,
            schemas,
        })
    }

    /// Converts one classic numeric item ID to its historical string identifier.
    #[must_use]
    pub fn legacy_numeric_name(&self, numeric_id: i32) -> Option<&str> {
        self.legacy_numeric_ids.get(&numeric_id).map(String::as_str)
    }

    /// Returns the 1.12-era block identifier represented by an old blockitem ID.
    #[must_use]
    pub fn legacy_block_id(&self, item_id: &str) -> Option<&str> {
        self.item_to_block_1_12.get(item_id).map(String::as_str)
    }

    /// Applies every item ID/meta schema in numeric priority order.
    ///
    /// `remappedMetas` takes precedence over `renamedIds` within one schema. A metadata remap changes
    /// the identifier and resets metadata to zero, after which later schemas continue processing.
    #[must_use]
    pub fn upgrade_id_meta(&self, item_id: &str, meta: i32) -> ItemIdentity {
        let mut name = item_id.to_string();
        let mut meta = meta;
        for schema in &self.schemas {
            if let Some(target) = schema
                .remapped_metas
                .get(&name)
                .and_then(|values| values.get(&meta))
            {
                name = target.clone();
                meta = 0;
            } else if let Some(target) = schema.renamed_ids.get(&name) {
                name = target.clone();
            }
        }
        ItemIdentity { name, meta }
    }

    /// Returns the number of loaded incremental item schemas.
    #[must_use]
    pub fn schema_count(&self) -> usize {
        self.schemas.len()
    }
}

/// Behaviour when an item cannot be authoritatively migrated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ItemMigrationPolicy {
    /// Preserve the complete item NBT unchanged.
    #[default]
    PreserveUnknown,
    /// Reject the migration so a caller can abort a destructive world write.
    RefuseUnknown,
}

/// Classification of one item-stack migration attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemMigrationStatus {
    /// The item was recognised and already matched the selected target representation.
    Unchanged,
    /// One or more authoritative item or block-state fields were rewritten.
    Upgraded,
    /// The item could not be upgraded safely and was preserved unchanged.
    Preserved,
    /// Legacy numeric item ID zero was recognised as an invalid persisted air stack and preserved.
    Air,
}

/// Result of migrating one item-stack NBT compound.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemStackMigrationOutcome {
    /// Migrated or preserved item NBT.
    pub nbt: NbtTag,
    /// Migration action taken.
    pub status: ItemMigrationStatus,
    /// Whether the source was classified as a blockitem.
    pub block_item: bool,
}

/// Resolves a 1.12-era block identifier and metadata into a versioned BlockState.
pub trait LegacyBlockItemResolver: Send + Sync {
    /// Resolves an old block ID/meta pair without guessing.
    fn resolve_legacy_block_item(&self, block_id: &str, meta: i32) -> Result<Option<BlockState>>;
}

impl<F> LegacyBlockItemResolver for F
where
    F: Fn(&str, i32) -> Result<Option<BlockState>> + Send + Sync,
{
    fn resolve_legacy_block_item(&self, block_id: &str, meta: i32) -> Result<Option<BlockState>> {
        self(block_id, meta)
    }
}

/// Block-domain services required to migrate blockitems.
#[derive(Clone, Copy)]
pub struct BlockItemMigrationContext<'a> {
    /// Optional resolver for old string block ID + metadata representations.
    pub legacy_resolver: Option<&'a dyn LegacyBlockItemResolver>,
    /// Semantic BlockState migrator shared with chunk migration.
    pub block_state_migrator: &'a dyn BlockStateMigrator,
    /// Exact BlockState storage version required for the target item.
    pub target_block_state_version: i32,
    /// Authoritative target palette validator.
    pub target_palette_contains: &'a dyn Fn(&BlockState) -> bool,
}

/// Aggregate result of recursively migrating item stacks contained by one NBT tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ItemNbtMigrationReport {
    /// Number of recognised item-stack compounds visited.
    pub items_seen: usize,
    /// Number of item stacks rewritten.
    pub items_upgraded: usize,
    /// Number of recognised item stacks already current.
    pub items_unchanged: usize,
    /// Number of item stacks preserved because migration was not authoritative.
    pub items_preserved: usize,
    /// Number of invalid persisted legacy air stacks observed.
    pub legacy_air_items: usize,
    /// Number of blockitems encountered.
    pub block_items_seen: usize,
}

/// Migrated NBT tree and its item-level report.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemNbtMigrationOutcome {
    /// Migrated NBT root.
    pub nbt: NbtTag,
    /// Aggregate migration counters.
    pub report: ItemNbtMigrationReport,
}

/// Migrates one Bedrock saved-item compound while retaining unrelated stack fields.
pub fn migrate_item_stack_nbt(
    nbt: &NbtTag,
    catalog: &AuthoritativeItemMigrationCatalog,
    block_items: Option<&BlockItemMigrationContext<'_>>,
    policy: ItemMigrationPolicy,
) -> Result<ItemStackMigrationOutcome> {
    let NbtTag::Compound(root) = nbt else {
        return unresolved(nbt, false, policy, "item stack root is not a compound");
    };
    let Some(raw_id) = read_item_id(root)? else {
        return unresolved(
            nbt,
            false,
            policy,
            "item stack has neither Name nor legacy id",
        );
    };
    let (raw_name, source) = match raw_id {
        RawItemId::Name(name) => (name, ItemIdSource::Name),
        RawItemId::StringId(name) => (name, ItemIdSource::LegacyStringId),
        RawItemId::Numeric(0) => {
            return Ok(ItemStackMigrationOutcome {
                nbt: nbt.clone(),
                status: ItemMigrationStatus::Air,
                block_item: false,
            });
        }
        RawItemId::Numeric(id) => {
            let Some(name) = catalog.legacy_numeric_name(id) else {
                return unresolved(
                    nbt,
                    false,
                    policy,
                    format!("unmapped legacy numeric item id {id}"),
                );
            };
            (name.to_string(), ItemIdSource::LegacyNumeric)
        }
    };
    let meta = read_item_meta(root)?;
    let existing_block = root.get("Block");
    let legacy_block_id = catalog.legacy_block_id(&raw_name);
    let is_block_item = existing_block.is_some() || legacy_block_id.is_some();

    let mut migrated = root.clone();
    let mut changed = false;
    if is_block_item {
        let Some(context) = block_items else {
            return unresolved(
                nbt,
                true,
                policy,
                format!("blockitem {raw_name} requires block-state migration context"),
            );
        };
        let source_state = if let Some(block) = existing_block {
            parse_block_state(block)?
        } else {
            let block_id = legacy_block_id.expect("legacy block id classified above");
            let Some(resolver) = context.legacy_resolver else {
                return unresolved(
                    nbt,
                    true,
                    policy,
                    format!("legacy blockitem {raw_name} requires an ID/meta block resolver"),
                );
            };
            let Some(state) = resolver.resolve_legacy_block_item(block_id, meta)? else {
                return unresolved(
                    nbt,
                    true,
                    policy,
                    format!(
                        "legacy blockitem {raw_name}:{meta} has no authoritative block mapping"
                    ),
                );
            };
            state
        };
        let target_state = context
            .block_state_migrator
            .migrate_to(&source_state, context.target_block_state_version)?;
        if target_state.version != Some(context.target_block_state_version) {
            return Err(validation(format!(
                "blockitem migrator returned BlockState version {:?}, expected {}",
                target_state.version, context.target_block_state_version
            )));
        }
        if !(context.target_palette_contains)(&target_state) {
            return Err(validation(format!(
                "blockitem state {} is not registered in the target authoritative palette",
                target_state.name
            )));
        }
        let block_nbt = merge_block_state(existing_block, &target_state);
        changed |= existing_block != Some(&block_nbt);
        migrated.insert("Block".to_string(), block_nbt);
    }

    let identity = catalog.upgrade_id_meta(&raw_name, meta);
    changed |= write_item_identity(&mut migrated, source, &identity, meta)?;
    Ok(ItemStackMigrationOutcome {
        nbt: NbtTag::Compound(migrated),
        status: if changed {
            ItemMigrationStatus::Upgraded
        } else {
            ItemMigrationStatus::Unchanged
        },
        block_item: is_block_item,
    })
}

/// Recursively migrates recognised item-stack compounds inside player, actor or block-entity NBT.
///
/// An item reported as `Preserved` or `Air` is not traversed further, preventing this generic walker
/// from interpreting unknown future item-private data. Recognised current/upgraded items may contain
/// nested saved items in their custom tag payloads, so their child compounds are visited.
pub fn migrate_item_stacks_in_nbt(
    nbt: &NbtTag,
    catalog: &AuthoritativeItemMigrationCatalog,
    block_items: Option<&BlockItemMigrationContext<'_>>,
    policy: ItemMigrationPolicy,
) -> Result<ItemNbtMigrationOutcome> {
    let mut report = ItemNbtMigrationReport::default();
    let nbt = migrate_tree(nbt, catalog, block_items, policy, &mut report)?;
    Ok(ItemNbtMigrationOutcome { nbt, report })
}

fn migrate_tree(
    nbt: &NbtTag,
    catalog: &AuthoritativeItemMigrationCatalog,
    block_items: Option<&BlockItemMigrationContext<'_>>,
    policy: ItemMigrationPolicy,
    report: &mut ItemNbtMigrationReport,
) -> Result<NbtTag> {
    match nbt {
        NbtTag::Compound(root) if looks_like_item_stack(root) => {
            let outcome = migrate_item_stack_nbt(nbt, catalog, block_items, policy)?;
            report.items_seen = report.items_seen.saturating_add(1);
            if outcome.block_item {
                report.block_items_seen = report.block_items_seen.saturating_add(1);
            }
            match outcome.status {
                ItemMigrationStatus::Upgraded => {
                    report.items_upgraded = report.items_upgraded.saturating_add(1)
                }
                ItemMigrationStatus::Unchanged => {
                    report.items_unchanged = report.items_unchanged.saturating_add(1)
                }
                ItemMigrationStatus::Preserved => {
                    report.items_preserved = report.items_preserved.saturating_add(1);
                    return Ok(outcome.nbt);
                }
                ItemMigrationStatus::Air => {
                    report.legacy_air_items = report.legacy_air_items.saturating_add(1);
                    return Ok(outcome.nbt);
                }
            }
            let NbtTag::Compound(mut migrated) = outcome.nbt else {
                unreachable!("item migration always returns a compound for recognised items")
            };
            for (name, child) in &mut migrated {
                if name == "Block" {
                    continue;
                }
                *child = migrate_tree(child, catalog, block_items, policy, report)?;
            }
            Ok(NbtTag::Compound(migrated))
        }
        NbtTag::Compound(root) => {
            let mut migrated = root.clone();
            for child in migrated.values_mut() {
                *child = migrate_tree(child, catalog, block_items, policy, report)?;
            }
            Ok(NbtTag::Compound(migrated))
        }
        NbtTag::List(values) => Ok(NbtTag::List(
            values
                .iter()
                .map(|child| migrate_tree(child, catalog, block_items, policy, report))
                .collect::<Result<Vec<_>>>()?,
        )),
        other => Ok(other.clone()),
    }
}

/// Loads a strict pinned catalogue from already supplied resource strings.
pub fn load_pinned_item_migration_catalog(
    legacy_item_id_map_json: &str,
    item_to_block_1_12_json: &str,
    sources: &[ItemSchemaSource<'_>],
) -> Result<AuthoritativeItemMigrationCatalog> {
    validate_pinned_schema_sources(sources)?;
    AuthoritativeItemMigrationCatalog::from_sources(
        legacy_item_id_map_json,
        item_to_block_1_12_json,
        sources,
    )
}

/// Verifies every file in a pinned item migration resource directory.
pub fn verify_pinned_item_migration_corpus(root: impl AsRef<Path>) -> Result<()> {
    let _ = read_verified_corpus(root.as_ref())?;
    Ok(())
}

/// Verifies and loads the complete pinned item migration corpus from a directory.
pub fn load_pinned_item_migration_catalog_from_dir(
    root: impl AsRef<Path>,
) -> Result<AuthoritativeItemMigrationCatalog> {
    let mut files = read_verified_corpus(root.as_ref())?;
    let legacy = take_utf8(&mut files, "item_legacy_id_map.json")?;
    let blocks = take_utf8(&mut files, "1.12.0_item_id_to_block_id_map.json")?;
    let mut documents =
        Vec::<(&'static str, String)>::with_capacity(PINNED_ITEM_SCHEMA_FILES.len());
    for &name in PINNED_ITEM_SCHEMA_FILES {
        documents.push((
            name,
            take_utf8(&mut files, &format!("id_meta_upgrade_schema/{name}"))?,
        ));
    }
    let sources = documents
        .iter()
        .map(|(name, json)| ItemSchemaSource {
            name: *name,
            json: json.as_str(),
        })
        .collect::<Vec<_>>();
    load_pinned_item_migration_catalog(&legacy, &blocks, &sources)
}

fn validate_pinned_schema_sources(sources: &[ItemSchemaSource<'_>]) -> Result<()> {
    if sources.len() != PINNED_ITEM_SCHEMA_FILES.len() {
        return Err(validation(format!(
            "pinned item corpus requires {} schemas, got {}",
            PINNED_ITEM_SCHEMA_FILES.len(),
            sources.len()
        )));
    }
    let expected = PINNED_ITEM_SCHEMA_FILES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let actual = sources
        .iter()
        .map(|source| source.name)
        .collect::<BTreeSet<_>>();
    if actual.len() != sources.len() {
        return Err(validation(
            "pinned item corpus contains duplicate filenames",
        ));
    }
    if actual != expected {
        let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
        let unexpected = actual.difference(&expected).copied().collect::<Vec<_>>();
        return Err(validation(format!(
            "pinned item corpus mismatch; missing={missing:?}, unexpected={unexpected:?}"
        )));
    }
    Ok(())
}

fn read_verified_corpus(root: &Path) -> Result<BTreeMap<&'static str, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for spec in PINNED_ITEM_MIGRATION_CORPUS_FILES {
        let bytes = fs::read(root.join(spec.path))?;
        if bytes.len() != spec.size {
            return Err(validation(format!(
                "pinned item corpus file {} has {} bytes, expected {}",
                spec.path,
                bytes.len(),
                spec.size
            )));
        }
        let actual = git_blob_sha1_hex(&bytes);
        if actual != spec.git_blob_sha1 {
            return Err(validation(format!(
                "pinned item corpus file {} has Git blob {}, expected {}",
                spec.path, actual, spec.git_blob_sha1
            )));
        }
        files.insert(spec.path, bytes);
    }
    Ok(files)
}

fn take_utf8(files: &mut BTreeMap<&'static str, Vec<u8>>, path: &str) -> Result<String> {
    let bytes = files
        .remove(path)
        .ok_or_else(|| validation(format!("verified item corpus lost required file {path}")))?;
    String::from_utf8(bytes).map_err(|error| {
        validation(format!(
            "pinned item corpus file {path} is not UTF-8: {error}"
        ))
    })
}

fn schema_id(name: &str) -> Result<u32> {
    let prefix = name
        .split_once('_')
        .map(|(prefix, _)| prefix)
        .ok_or_else(|| {
            validation(format!(
                "item schema filename has no numeric prefix: {name}"
            ))
        })?;
    prefix
        .parse::<u32>()
        .map_err(|error| validation(format!("invalid item schema id in {name}: {error}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawItemId {
    Name(String),
    StringId(String),
    Numeric(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemIdSource {
    Name,
    LegacyStringId,
    LegacyNumeric,
}

fn read_item_id(root: &IndexMap<String, NbtTag>) -> Result<Option<RawItemId>> {
    if let Some(value) = root.get("Name") {
        return match value {
            NbtTag::String(name) if !name.is_empty() => Ok(Some(RawItemId::Name(name.clone()))),
            NbtTag::String(_) => Err(validation("item Name is empty")),
            other => Err(validation(format!(
                "item Name has invalid NBT type: {other:?}"
            ))),
        };
    }
    let Some(value) = root.get("id") else {
        return Ok(None);
    };
    match value {
        NbtTag::String(name) if !name.is_empty() => Ok(Some(RawItemId::StringId(name.clone()))),
        NbtTag::String(_) => Err(validation("legacy item id string is empty")),
        NbtTag::Byte(value) => Ok(Some(RawItemId::Numeric(i32::from(*value)))),
        NbtTag::Short(value) => Ok(Some(RawItemId::Numeric(i32::from(*value)))),
        NbtTag::Int(value) => Ok(Some(RawItemId::Numeric(*value))),
        NbtTag::Long(value) => i32::try_from(*value)
            .map(RawItemId::Numeric)
            .map(Some)
            .map_err(|_| validation("legacy item id exceeds i32")),
        other => Err(validation(format!(
            "legacy item id has invalid NBT type: {other:?}"
        ))),
    }
}

fn read_item_meta(root: &IndexMap<String, NbtTag>) -> Result<i32> {
    for key in ["Damage", "Aux"] {
        let Some(value) = root.get(key) else {
            continue;
        };
        return integer_i32(value)
            .ok_or_else(|| validation(format!("item {key} is not an i32-compatible integer")));
    }
    Ok(0)
}

fn integer_i32(value: &NbtTag) -> Option<i32> {
    match value {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn looks_like_item_stack(root: &IndexMap<String, NbtTag>) -> bool {
    let has_id = matches!(root.get("Name"), Some(NbtTag::String(_)))
        || matches!(
            root.get("id"),
            Some(
                NbtTag::String(_)
                    | NbtTag::Byte(_)
                    | NbtTag::Short(_)
                    | NbtTag::Int(_)
                    | NbtTag::Long(_)
            )
        );
    let has_count = matches!(
        root.get("Count"),
        Some(NbtTag::Byte(_) | NbtTag::Short(_) | NbtTag::Int(_) | NbtTag::Long(_))
    );
    has_id && has_count
}

fn write_item_identity(
    root: &mut IndexMap<String, NbtTag>,
    source: ItemIdSource,
    identity: &ItemIdentity,
    old_meta: i32,
) -> Result<bool> {
    let mut changed = false;
    if root.get("Name") != Some(&NbtTag::String(identity.name.clone())) {
        root.insert("Name".to_string(), NbtTag::String(identity.name.clone()));
        changed = true;
    }
    if !matches!(source, ItemIdSource::Name) && root.shift_remove("id").is_some() {
        changed = true;
    }
    if identity.meta != old_meta || root.contains_key("Aux") {
        let meta = i16::try_from(identity.meta).map_err(|_| {
            validation(format!("item metadata {} exceeds TAG_Short", identity.meta))
        })?;
        if root.get("Damage") != Some(&NbtTag::Short(meta)) {
            root.insert("Damage".to_string(), NbtTag::Short(meta));
            changed = true;
        }
        if root.shift_remove("Aux").is_some() {
            changed = true;
        }
    }
    Ok(changed)
}

fn parse_block_state(tag: &NbtTag) -> Result<BlockState> {
    let NbtTag::Compound(root) = tag else {
        return Err(validation("item Block payload is not a compound"));
    };
    let name = match root.get("name") {
        Some(NbtTag::String(name)) if !name.is_empty() => name.clone(),
        Some(other) => {
            return Err(validation(format!(
                "item Block name has invalid type: {other:?}"
            )));
        }
        None => return Err(validation("item Block payload has no name")),
    };
    let states = match root.get("states") {
        Some(NbtTag::Compound(states)) => {
            states.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }
        Some(other) => {
            return Err(validation(format!(
                "item Block states has invalid type: {other:?}"
            )));
        }
        None => BTreeMap::new(),
    };
    let version = match root.get("version") {
        Some(NbtTag::Int(version)) => Some(*version),
        Some(other) => {
            return Err(validation(format!(
                "item Block version has invalid type: {other:?}"
            )));
        }
        None => None,
    };
    Ok(BlockState {
        name,
        states,
        version,
    })
}

fn merge_block_state(original: Option<&NbtTag>, state: &BlockState) -> NbtTag {
    let mut root = match original {
        Some(NbtTag::Compound(root)) => root.clone(),
        _ => IndexMap::new(),
    };
    root.insert("name".to_string(), NbtTag::String(state.name.clone()));
    root.insert(
        "states".to_string(),
        NbtTag::Compound(
            state
                .states
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        ),
    );
    if let Some(version) = state.version {
        root.insert("version".to_string(), NbtTag::Int(version));
    }
    NbtTag::Compound(root)
}

fn unresolved(
    original: &NbtTag,
    block_item: bool,
    policy: ItemMigrationPolicy,
    reason: impl Into<String>,
) -> Result<ItemStackMigrationOutcome> {
    let reason = reason.into();
    match policy {
        ItemMigrationPolicy::PreserveUnknown => Ok(ItemStackMigrationOutcome {
            nbt: original.clone(),
            status: ItemMigrationStatus::Preserved,
            block_item,
        }),
        ItemMigrationPolicy::RefuseUnknown => {
            Err(BedrockWorldError::UnsupportedChunkFormat(reason))
        }
    }
}

fn validation(message: impl Into<String>) -> BedrockWorldError {
    BedrockWorldError::Validation(message.into())
}

fn git_blob_sha1_hex(bytes: &[u8]) -> String {
    let header = format!("blob {}\0", bytes.len());
    let mut sha1 = Sha1::new();
    sha1.update(header.as_bytes());
    sha1.update(bytes);
    sha1.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
            let needed = 64 - self.buffer_len;
            let take = needed.min(input.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&input[..take]);
            self.buffer_len += take;
            input = &input[take..];
            if self.buffer_len == 64 {
                let block = self.buffer;
                self.process_block(&block);
                self.buffer_len = 0;
            }
        }
        for block in input.chunks_exact(64) {
            let block: &[u8; 64] = block.try_into().expect("exact SHA-1 block");
            self.process_block(block);
        }
        let remainder = input.len() % 64;
        if remainder != 0 {
            let start = input.len() - remainder;
            self.buffer[..remainder].copy_from_slice(&input[start..]);
            self.buffer_len = remainder;
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

        let mut output = [0_u8; 20];
        for (index, value) in self.state.iter().enumerate() {
            output[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
        }
        output
    }

    fn process_block(&mut self, block: &[u8; 64]) {
        let mut words = [0_u32; 80];
        for (index, bytes) in block.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().expect("SHA-1 word"));
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];
        let mut e = self.state[4];
        for (index, word) in words.iter().copied().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
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
    use crate::block::{BlockStateMigrationGraph, BlockStateMigrationStep};

    fn catalog() -> AuthoritativeItemMigrationCatalog {
        AuthoritativeItemMigrationCatalog::from_sources(
            r#"{"minecraft:coal":263,"minecraft:stone":1}"#,
            r#"{"minecraft:stone":"minecraft:stone"}"#,
            &[
                ItemSchemaSource {
                    name: "0011_test.json",
                    json: r#"{"renamedIds":{"minecraft:old_bucket":"minecraft:bucket"}}"#,
                },
                ItemSchemaSource {
                    name: "0021_test.json",
                    json: r#"{"remappedMetas":{"minecraft:bucket":{"8":"minecraft:water_bucket"},"minecraft:coal":{"1":"minecraft:charcoal"}}}"#,
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn schema_order_and_meta_precedence_match_reference_upgrader() {
        let catalog = catalog();
        assert_eq!(
            catalog.upgrade_id_meta("minecraft:old_bucket", 8),
            ItemIdentity {
                name: "minecraft:water_bucket".to_string(),
                meta: 0,
            }
        );
    }

    #[test]
    fn classic_numeric_item_is_lifted_without_dropping_stack_fields() {
        let catalog = catalog();
        let source = NbtTag::Compound(IndexMap::from([
            ("id".to_string(), NbtTag::Short(263)),
            ("Damage".to_string(), NbtTag::Short(1)),
            ("Count".to_string(), NbtTag::Byte(3)),
            ("Slot".to_string(), NbtTag::Byte(7)),
            (
                "tag".to_string(),
                NbtTag::Compound(IndexMap::from([("custom".to_string(), NbtTag::Int(42))])),
            ),
        ]));
        let outcome = migrate_item_stack_nbt(
            &source,
            &catalog,
            None,
            ItemMigrationPolicy::PreserveUnknown,
        )
        .unwrap();
        assert_eq!(outcome.status, ItemMigrationStatus::Upgraded);
        let NbtTag::Compound(root) = outcome.nbt else {
            panic!("compound")
        };
        assert_eq!(
            root.get("Name"),
            Some(&NbtTag::String("minecraft:charcoal".to_string()))
        );
        assert_eq!(root.get("Damage"), Some(&NbtTag::Short(0)));
        assert_eq!(root.get("Count"), Some(&NbtTag::Byte(3)));
        assert_eq!(root.get("Slot"), Some(&NbtTag::Byte(7)));
        assert!(!root.contains_key("id"));
        assert!(root.contains_key("tag"));
    }

    #[test]
    fn blockitem_is_preserved_without_block_context() {
        let catalog = catalog();
        let source = NbtTag::Compound(IndexMap::from([
            (
                "Name".to_string(),
                NbtTag::String("minecraft:stone".to_string()),
            ),
            ("Damage".to_string(), NbtTag::Short(0)),
            ("Count".to_string(), NbtTag::Byte(1)),
        ]));
        let outcome = migrate_item_stack_nbt(
            &source,
            &catalog,
            None,
            ItemMigrationPolicy::PreserveUnknown,
        )
        .unwrap();
        assert_eq!(outcome.status, ItemMigrationStatus::Preserved);
        assert_eq!(outcome.nbt, source);
        assert!(
            migrate_item_stack_nbt(&source, &catalog, None, ItemMigrationPolicy::RefuseUnknown,)
                .is_err()
        );
    }

    #[test]
    fn existing_blockitem_uses_shared_blockstate_migrator() {
        let catalog = AuthoritativeItemMigrationCatalog::from_sources("{}", "{}", &[]).unwrap();
        let mut graph = BlockStateMigrationGraph::new();
        graph
            .add_step(BlockStateMigrationStep::identity(10, 20))
            .unwrap();
        let validator = |state: &BlockState| state.name == "minecraft:test";
        let context = BlockItemMigrationContext {
            legacy_resolver: None,
            block_state_migrator: &graph,
            target_block_state_version: 20,
            target_palette_contains: &validator,
        };
        let source = NbtTag::Compound(IndexMap::from([
            (
                "Name".to_string(),
                NbtTag::String("minecraft:test_item".to_string()),
            ),
            ("Damage".to_string(), NbtTag::Short(0)),
            ("Count".to_string(), NbtTag::Byte(1)),
            (
                "Block".to_string(),
                NbtTag::Compound(IndexMap::from([
                    (
                        "name".to_string(),
                        NbtTag::String("minecraft:test".to_string()),
                    ),
                    ("states".to_string(), NbtTag::Compound(IndexMap::new())),
                    ("version".to_string(), NbtTag::Int(10)),
                    ("future_extra".to_string(), NbtTag::Long(9)),
                ])),
            ),
        ]));
        let outcome = migrate_item_stack_nbt(
            &source,
            &catalog,
            Some(&context),
            ItemMigrationPolicy::RefuseUnknown,
        )
        .unwrap();
        assert_eq!(outcome.status, ItemMigrationStatus::Upgraded);
        let NbtTag::Compound(item) = outcome.nbt else {
            panic!("item")
        };
        let Some(NbtTag::Compound(block)) = item.get("Block") else {
            panic!("block")
        };
        assert_eq!(block.get("version"), Some(&NbtTag::Int(20)));
        assert_eq!(block.get("future_extra"), Some(&NbtTag::Long(9)));
    }

    #[test]
    fn recursive_walker_handles_inventory_lists() {
        let catalog = catalog();
        let source = NbtTag::Compound(IndexMap::from([(
            "Inventory".to_string(),
            NbtTag::List(vec![NbtTag::Compound(IndexMap::from([
                ("id".to_string(), NbtTag::Short(263)),
                ("Damage".to_string(), NbtTag::Short(1)),
                ("Count".to_string(), NbtTag::Byte(1)),
            ]))]),
        )]));
        let outcome = migrate_item_stacks_in_nbt(
            &source,
            &catalog,
            None,
            ItemMigrationPolicy::PreserveUnknown,
        )
        .unwrap();
        assert_eq!(outcome.report.items_seen, 1);
        assert_eq!(outcome.report.items_upgraded, 1);
    }

    #[test]
    fn git_blob_sha1_matches_known_vectors() {
        assert_eq!(
            git_blob_sha1_hex(b""),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
        assert_eq!(
            git_blob_sha1_hex(b"abc"),
            "f2ba8f84ab5c1bce84a7b441cb1959cfc7093b7f"
        );
    }

    #[test]
    fn pinned_manifest_is_complete_and_ordered() {
        assert_eq!(PINNED_ITEM_SCHEMA_FILES.len(), 27);
        let ids = PINNED_ITEM_SCHEMA_FILES
            .iter()
            .map(|name| schema_id(name).unwrap())
            .collect::<Vec<_>>();
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(PINNED_ITEM_MIGRATION_CORPUS_FILES.len(), 30);
    }
}
