//! Multi-version Minecraft Bedrock saved-item data and explicit historical conversion.
//!
//! Item reads retain the representation found in NBT. Historical ID/meta and BlockState changes are
//! only applied when a caller explicitly invokes [`conversion`].

/// Explicit cross-version saved-item conversion and pinned authoritative version data.
pub mod conversion;

pub use crate::parsed::ItemStack;
pub use conversion::{
    AuthoritativeItemMigrationCatalog, BlockItemMigrationContext, ItemIdentity,
    ItemMigrationPolicy, ItemMigrationStatus, ItemNbtMigrationOutcome, ItemNbtMigrationReport,
    ItemSchemaSource, ItemStackMigrationOutcome, LegacyBlockItemResolver,
    PINNED_ITEM_MIGRATION_CORPUS_FILES, PINNED_ITEM_SCHEMA_FILES,
    PINNED_ITEM_UPGRADE_SCHEMA_COMMIT, PINNED_ITEM_UPGRADE_SCHEMA_TREE,
    PinnedItemCorpusFileSpec, load_pinned_item_migration_catalog,
    load_pinned_item_migration_catalog_from_dir, migrate_item_stack_nbt,
    migrate_item_stacks_in_nbt, verify_pinned_item_migration_corpus,
};
