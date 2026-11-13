//! Minecraft Bedrock item NBT parsing, editing, and persisted representations.

mod evidence;
mod display;
pub mod classic;
pub mod medieval;
pub mod modern;
mod modern_target;
pub mod history;
pub mod migration;
mod vanilla;
mod version_target;

pub use crate::scan::ItemStack;
pub use evidence::{Evidence, StorageForm, inspect, storage_form};
pub use display::set_item_display;
pub use modern_target::{ModernSavedItemTarget, ModernSavedItemTargetMatch};
pub use migration::{
    AuthoritativeItemMigrationCatalog, BlockItemMigrationContext, ItemIdentity,
    ItemMigrationPolicy, ItemMigrationStatus, ItemNbtMigrationOutcome, ItemNbtMigrationReport,
    ItemSchemaSource, ItemStackMigrationOutcome, LegacyBlockItemResolver,
    PINNED_ITEM_MIGRATION_CORPUS_FILES, PINNED_ITEM_SCHEMA_FILES,
    PINNED_ITEM_UPGRADE_SCHEMA_COMMIT, PINNED_ITEM_UPGRADE_SCHEMA_TREE, PinnedItemCorpusFileSpec,
    migrate_item_stack_nbt, migrate_item_stacks_in_nbt,
};
pub use history::{
    ClassicSavedItemId, ClassicSavedItemMatch, MedievalSavedItemId, MedievalSavedItemMatch,
    NamedSavedItemId, SavedItemBlockStates, SavedItemHistory, SavedItemUpgradeSource,
};
pub use vanilla::{
    VanillaSavedItemBlockMap, VanillaSavedItemBlockMatch, VanillaSavedItemEntry,
    VanillaSavedItemPalette,
};
pub use version_target::{SavedItemVersionMatch, SavedItemVersionTable, SavedItemVersionTarget};

/// Persisted Bedrock item representation generations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Generation {
    /// MCPE <= 1.5: numeric TAG_Short `id` plus TAG_Short `Damage` metadata.
    Classic,
    /// MCPE 1.6-1.8: string `Name` plus TAG_Short `Damage`; blockitems are reconstructed from ID+meta.
    Medieval,
    /// MCPE 1.9+: string `Name` plus metadata and optional persisted `Block` BlockState NBT.
    Modern,
}
