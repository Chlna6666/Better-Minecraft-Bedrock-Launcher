use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use tracing::warn;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InstalledModLoader {
    pub(crate) id: Arc<str>,
    pub(crate) name: Arc<str>,
    pub(crate) version: Arc<str>,
}

struct ModLoaderProbe {
    id: &'static str,
    name: &'static str,
    manifest_path: &'static str,
}

const MOD_LOADER_PROBES: &[ModLoaderProbe] = &[ModLoaderProbe {
    id: "levilamina",
    name: "LeviLamina",
    manifest_path: "mods/LeviLamina/manifest.json",
}];

#[derive(Deserialize)]
struct ModLoaderManifest {
    version: String,
}

pub(crate) fn detect_installed_mod_loaders(game_directory: &Path) -> Vec<InstalledModLoader> {
    MOD_LOADER_PROBES
        .iter()
        .filter_map(
            |probe| match read_installed_mod_loader(game_directory, probe) {
                Ok(loader) => loader,
                Err(error) => {
                    warn!(
                        loader = probe.id,
                        path = %game_directory.display(),
                        %error,
                        "读取已安装 Mod 加载器信息失败"
                    );
                    None
                }
            },
        )
        .collect()
}

fn read_installed_mod_loader(
    game_directory: &Path,
    probe: &ModLoaderProbe,
) -> Result<Option<InstalledModLoader>, String> {
    let manifest_path = game_directory.join(probe.manifest_path);
    if !manifest_path.is_file() {
        return Ok(None);
    }

    let bytes = std::fs::read(&manifest_path)
        .map_err(|error| format!("读取 {} 失败: {error}", manifest_path.display()))?;
    let manifest = serde_json::from_slice::<ModLoaderManifest>(&bytes)
        .map_err(|error| format!("解析 {} 失败: {error}", manifest_path.display()))?;
    let version = manifest.version.trim();
    if version.is_empty() {
        return Err(format!("{} 缺少有效版本号", manifest_path.display()));
    }

    Ok(Some(InstalledModLoader {
        id: Arc::from(probe.id),
        name: Arc::from(probe.name),
        version: Arc::from(version.to_owned()),
    }))
}
