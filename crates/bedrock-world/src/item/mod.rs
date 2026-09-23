//! Minecraft Bedrock saved-item NBT, including historical persisted representations.

mod classic_saved_item;
mod format;
mod format_evidence;
mod medieval_saved_item;
mod modern_saved_item;
mod modern_target;
mod saved_item;
mod saved_item_history;
mod vanilla_saved_item_block_map;
mod vanilla_saved_item_palette;
mod version_target;

pub use crate::scan::ItemStack;
pub use classic_saved_item::{
    ClassicSavedItemCheckReport, ClassicSavedItemConversionOutcome,
    ClassicSavedItemConversionReport, ClassicSavedItemIssue, ClassicSavedItemIssueKind,
    check_saved_items_for_classic, convert_saved_items_to_classic,
};
pub use format::SavedItemFormat;
pub use format_evidence::{
    SavedItemFormatEvidence, SavedItemStorageForm, inspect_saved_item_formats,
    saved_item_storage_form,
};
pub use medieval_saved_item::{
    MedievalSavedItemCheckReport, MedievalSavedItemConversionOutcome,
    MedievalSavedItemConversionReport, MedievalSavedItemIssue, MedievalSavedItemIssueKind,
    check_saved_items_for_medieval, convert_saved_items_to_medieval,
};
pub use modern_saved_item::{
    ModernSavedItemCheckReport, ModernSavedItemConversionOutcome, ModernSavedItemConversionReport,
    ModernSavedItemIssue, ModernSavedItemIssueKind, check_saved_items_for_modern_target,
    convert_saved_items_to_modern_target,
};
pub use modern_target::{ModernSavedItemTarget, ModernSavedItemTargetMatch};
pub use saved_item_history::{
    ClassicSavedItemId, ClassicSavedItemMatch, MedievalSavedItemId, MedievalSavedItemMatch,
    NamedSavedItemId, SavedItemBlockStates, SavedItemHistory, SavedItemUpgradeSource,
    load_pinned_saved_item_history_from_dir,
};
pub use vanilla_saved_item_block_map::{VanillaSavedItemBlockMap, VanillaSavedItemBlockMatch};
pub use vanilla_saved_item_palette::{VanillaSavedItemEntry, VanillaSavedItemPalette};
pub use version_target::{SavedItemVersionMatch, SavedItemVersionTable, SavedItemVersionTarget};

// The historical forward rule executor remains crate-private while public APIs are expressed through
// concrete saved-item representations and explicit upgrade/reverse checks.
