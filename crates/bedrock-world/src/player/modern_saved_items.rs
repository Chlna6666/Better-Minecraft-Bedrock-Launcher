//! Explicit player saved-item conversion to one concrete older Modern Bedrock release.

use crate::error::{BedrockWorldError, Result};
use crate::item::{
    ModernSavedItemCheckReport, ModernSavedItemConversionReport, ModernSavedItemTarget,
    check_saved_items_for_modern_target, convert_saved_items_to_modern_target,
};
use crate::player::PlayerData;

impl PlayerData {
    /// Checks this player's saved items against one concrete older Modern target release.
    ///
    /// The player must carry actual `level.dat` version evidence and that version must exactly match
    /// the target's declared source game version. This prevents applying a reverse index built for a
    /// different source release. Call the item-level function directly only when source context is
    /// supplied outside `PlayerData`.
    pub fn check_saved_items_for_modern_target(
        &self,
        target: &ModernSavedItemTarget,
    ) -> Result<ModernSavedItemCheckReport> {
        ensure_source_game_version(self, target)?;
        check_saved_items_for_modern_target(&self.nbt, target)
    }

    /// Explicitly rewrites this player's saved items to one concrete older Modern target release.
    ///
    /// The complete owned NBT tree is preflighted and converted before `self.nbt` changes. A version
    /// mismatch, missing/ambiguous target identity, Classic numeric source, metadata conflict or
    /// blockitem incompatibility leaves the player unchanged.
    pub fn convert_saved_items_to_modern_target(
        &mut self,
        target: &ModernSavedItemTarget,
    ) -> Result<ModernSavedItemConversionReport> {
        ensure_source_game_version(self, target)?;
        let outcome = convert_saved_items_to_modern_target(&self.nbt, target)?;
        self.nbt = outcome.nbt;
        self.finish_edit();
        Ok(outcome.report)
    }
}

fn ensure_source_game_version(
    player: &PlayerData,
    target: &ModernSavedItemTarget,
) -> Result<()> {
    let actual = player.game_version().ok_or_else(|| {
        BedrockWorldError::Validation(
            "Modern player saved-item conversion requires owning level.dat LastOpenedWithVersion evidence"
                .to_string(),
        )
    })?;
    if actual != target.source_game_version() {
        return Err(BedrockWorldError::Validation(format!(
            "Modern player saved-item source version mismatch: player={actual}, target-source={}",
            target.source_game_version()
        )));
    }
    Ok(())
}
