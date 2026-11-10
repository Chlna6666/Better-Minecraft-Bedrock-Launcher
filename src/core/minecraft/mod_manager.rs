use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::fs;

/// Only the legacy UWP direct-injection path consumes this list.
/// Modern launches must let BLoader load its bootstrap and its own mods, then
/// let PreLoader/LeviLamina manage the native Mod dependency chain.
/// Never inject LeviLamina.dll or one of its native mods from BMCBL.
pub async fn load_mods_config(mods_dir: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let game_dir = mods_dir.parent().context("Invalid Minecraft mods root")?;
    let bootstrap = game_dir
        .join("BLoader")
        .join("PreLoader")
        .join("PreLoader.dll");
    if !bootstrap.is_file() {
        return Ok(Vec::new());
    }
    let path = fs::canonicalize(&bootstrap)
        .await
        .with_context(|| format!("Failed to resolve PreLoader bootstrap: {}", bootstrap.display()))?;
    let path = if let Some(text) = path.to_str() {
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            path
        }
    } else {
        path
    };
    Ok(vec![(path, 0)])
}
