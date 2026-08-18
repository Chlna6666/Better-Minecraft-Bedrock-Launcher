//! Minecraft Bedrock saved-item NBT, including historical persisted representations.

mod classic_saved_item;
mod format;
mod format_evidence;
mod legacy_saved_item;
mod legacy_saved_item_check;
mod legacy_saved_item_conversion;
mod medieval_saved_item;
mod saved_item;
mod vanilla_saved_item_palette;
mod version_target;

pub use crate::parsed::ItemStack;
pub use classic_saved_item::{
    ClassicSavedItemCheckReport, ClassicSavedItemConversionOutcome, ClassicSavedItemConversionReport,
    ClassicSavedItemIssue, ClassicSavedItemIssueKind, check_saved_items_for_classic,
    check_saved_items_for_classic_with_blocks, convert_saved_items_to_classic,
    convert_saved_items_to_classic_with_blocks,
};
pub use format::SavedItemFormat;
pub use format_evidence::{
    SavedItemFormatEvidence, SavedItemStorageForm, inspect_saved_item_formats,
    saved_item_storage_form,
};
pub use legacy_saved_item::{
    LegacySavedItemId, LegacySavedItemIdTable, LegacySavedItemMatch, MedievalSavedItemId,
    MedievalSavedItemMatch, NamedSavedItemId, SavedItemUpgradeSource,
    load_pinned_legacy_saved_item_id_table_from_dir,
};
pub use legacy_saved_item_check::{
    LegacySavedItemBlockStateTables, LegacySavedItemCheckReport, LegacySavedItemIssue,
    LegacySavedItemIssueKind, check_legacy_numeric_saved_items,
    check_legacy_numeric_saved_items_with_blocks,
};
pub use legacy_saved_item_conversion::{
    LegacySavedItemConversionOutcome, LegacySavedItemConversionReport,
    convert_saved_items_to_legacy_numeric, convert_saved_items_to_legacy_numeric_with_blocks,
};
pub use medieval_saved_item::{
    MedievalSavedItemCheckReport, MedievalSavedItemConversionOutcome,
    MedievalSavedItemConversionReport, MedievalSavedItemIssue, MedievalSavedItemIssueKind,
    check_saved_items_for_medieval, check_saved_items_for_medieval_with_blocks,
    convert_saved_items_to_medieval, convert_saved_items_to_medieval_with_blocks,
};
pub use vanilla_saved_item_palette::{VanillaSavedItemEntry, VanillaSavedItemPalette};
pub use version_target::{SavedItemVersionMatch, SavedItemVersionTable, SavedItemVersionTarget};

// The historical forward rule executor remains crate-private while public APIs are expressed through
// concrete saved-item representations and explicit upgrade/reverse checks.
pub(crate) use saved_item::{
    AuthoritativeItemMigrationCatalog, BlockItemMigrationContext, ItemIdentity,
    ItemMigrationPolicy, ItemMigrationStatus, ItemNbtMigrationOutcome, ItemNbtMigrationReport,
    ItemSchemaSource, ItemStackMigrationOutcome, LegacyBlockItemResolver,
    PINNED_ITEM_MIGRATION_CORPUS_FILES, PINNED_ITEM_SCHEMA_FILES,
    PINNED_ITEM_UPGRADE_SCHEMA_COMMIT, PINNED_ITEM_UPGRADE_SCHEMA_TREE,
    PinnedItemCorpusFileSpec, load_pinned_item_migration_catalog,
    load_pinned_item_migration_catalog_from_dir, migrate_item_stack_nbt,
    migrate_item_stacks_in_nbt, verify_pinned_item_migration_corpus,
};
