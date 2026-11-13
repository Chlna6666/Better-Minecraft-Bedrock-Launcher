use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Resolves the PreLoader bootstrap used by the legacy UWP direct-injection path.
/// Modern launches must let BLoader load its bootstrap and its own mods, then
/// let PreLoader/LeviLamina manage the native Mod dependency chain.
/// Never inject LeviLamina.dll or one of its native mods from BMCBL.
pub(super) async fn resolve_bootstrap(mods_dir: &Path) -> Result<Option<PathBuf>> {
    mods_dir.parent().context("Invalid Minecraft mods root")?;
    let bootstrap = mods_dir.join("PreLoader").join("PreLoader.dll");
    if !bootstrap.is_file() {
        return Ok(None);
    }
    let path = fs::canonicalize(&bootstrap).await.with_context(|| {
        format!(
            "Failed to resolve PreLoader bootstrap: {}",
            bootstrap.display()
        )
    })?;
    let path = if let Some(text) = path.to_str() {
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            path
        }
    } else {
        path
    };
    Ok(Some(path))
}
