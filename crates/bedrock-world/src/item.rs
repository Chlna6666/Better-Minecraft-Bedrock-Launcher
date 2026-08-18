//! Minecraft Bedrock saved-item NBT, including historical numeric ID/meta representations.

mod saved_item;

pub use crate::parsed::ItemStack;
pub use saved_item::{
    AuthoritativeItemMigrationCatalog, BlockItemMigrationContext, ItemIdentity,
    ItemMigrationPolicy, ItemMigrationStatus, ItemNbtMigrationOutcome, ItemNbtMigrationReport,
    ItemSchemaSource, ItemStackMigrationOutcome, LegacyBlockItemResolver,
    PINNED_ITEM_MIGRATION_CORPUS_FILES, PINNED_ITEM_SCHEMA_FILES,
    PINNED_ITEM_UPGRADE_SCHEMA_COMMIT, PINNED_ITEM_UPGRADE_SCHEMA_TREE,
    PinnedItemCorpusFileSpec, load_pinned_item_migration_catalog,
    load_pinned_item_migration_catalog_from_dir, migrate_item_stack_nbt,
    migrate_item_stacks_in_nbt, verify_pinned_item_migration_corpus,
};
