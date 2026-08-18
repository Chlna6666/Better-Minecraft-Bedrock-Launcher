//! Bedrock saved-item data and preservation-first historical item migration.

/// Historical saved-item migration and pinned authoritative schema loading.
pub mod migration;

pub use crate::parsed::ItemStack;
pub use migration::{
    AuthoritativeItemMigrationCatalog, BlockItemMigrationContext, ItemIdentity,
    ItemMigrationPolicy, ItemMigrationStatus, ItemNbtMigrationOutcome, ItemNbtMigrationReport,
    ItemSchemaSource, ItemStackMigrationOutcome, LegacyBlockItemResolver,
    PINNED_ITEM_MIGRATION_CORPUS_FILES, PINNED_ITEM_SCHEMA_FILES,
    PINNED_ITEM_UPGRADE_SCHEMA_COMMIT, PINNED_ITEM_UPGRADE_SCHEMA_TREE,
    PinnedItemCorpusFileSpec, load_pinned_item_migration_catalog,
    load_pinned_item_migration_catalog_from_dir, migrate_item_stack_nbt,
    migrate_item_stacks_in_nbt, verify_pinned_item_migration_corpus,
};
