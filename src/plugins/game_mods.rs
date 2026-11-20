//! Plugin-owned game mods and per-version approval of their exact payload.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Deserialize;

use super::manifest::PluginManifest;

const MAX_MODULE_BYTES: u64 = 32 * 1024 * 1024;
static SETTINGS: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Declaration {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) platform: Platform,
    pub(crate) phase: Phase,
    pub(crate) game_versions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Platform {
    #[serde(rename = "windows-x86_64")]
    WindowsX64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Phase {
    Preload,
    Ready,
}

impl Declaration {
    pub(super) fn valid(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && self.path.starts_with("game-mods/")
            && self.path.to_ascii_lowercase().ends_with(".dll")
            && !self.game_versions.is_empty()
            && self.game_versions.len() <= 32
            && self.game_versions.iter().all(|version| {
                version.len() <= 32
                    && version
                        .split('.')
                        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GameMod {
    pub(crate) key: String,
    pub(crate) plugin: String,
    pub(crate) declaration: Declaration,
    pub(crate) sha256: String,
    pub(crate) payload: PathBuf,
}

impl GameMod {
    pub(crate) fn supports(&self, version: &str) -> bool {
        cfg!(target_arch = "x86_64")
            && self.declaration.game_versions.iter().any(|prefix| {
                version == prefix
                    || version
                        .strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.starts_with('.'))
            })
    }

    pub(crate) fn enabled(&self, approvals: &BTreeMap<String, String>) -> bool {
        approvals.get(&self.key) == Some(&self.sha256)
    }
}

pub(super) fn prepare(manifest: &PluginManifest) -> Vec<GameMod> {
    if manifest.game_mods.is_empty() {
        return Vec::new();
    }
    let Ok(root) = manifest.root_dir.canonicalize() else {
        return Vec::new();
    };
    manifest.game_mods.iter().filter_map(|declaration| {
        let result = (|| {
            let payload = root.join(&declaration.path).canonicalize()?;
            anyhow::ensure!(payload.starts_with(&root) && payload.is_file(), "invalid game mod file");
            let mut bytes = Vec::new();
            fs::File::open(&payload)?.take(MAX_MODULE_BYTES + 1).read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() as u64 <= MAX_MODULE_BYTES && is_x64_dll(&bytes), "expected an x64 PE DLL under 32 MiB");
            Ok::<_, anyhow::Error>(GameMod {
                key: format!("{}/{}", manifest.id, declaration.id),
                plugin: manifest.name.clone(),
                declaration: declaration.clone(),
                sha256: bmcbl_plugin_runtime::engine::wasm_sha256_hex(&bytes),
                payload,
            })
        })();
        match result {
            Ok(game_mod) => Some(game_mod),
            Err(error) => {
                tracing::warn!(plugin = %manifest.id, module = %declaration.id, %error, "invalid plugin game mod");
                None
            }
        }
    }).collect()
}

fn is_x64_dll(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"MZ") {
        return false;
    }
    let Some(offset) = bytes
        .get(60..64)
        .and_then(|value| value.try_into().ok())
        .map(u32::from_le_bytes)
    else {
        return false;
    };
    let start = offset as usize;
    let Some(header) = bytes.get(start..start.saturating_add(24)) else {
        return false;
    };
    header.starts_with(b"PE\0\0\x64\x86") && (header[23] & 0x20) != 0
}

fn settings_path(version: &str) -> PathBuf {
    let key = bmcbl_plugin_runtime::engine::wasm_sha256_hex(version.as_bytes());
    crate::utils::file_ops::state_subdir("plugin-game-mods").join(format!("{key}.json"))
}

pub(crate) fn approvals(version: &str) -> Result<BTreeMap<String, String>, String> {
    match fs::read(settings_path(version)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn set_enabled(
    version: &str,
    game_mod: &GameMod,
    enabled: bool,
) -> Result<BTreeMap<String, String>, String> {
    let _guard = SETTINGS.lock().unwrap_or_else(|error| error.into_inner());
    let mut approvals = approvals(version)?;
    if enabled {
        approvals.insert(game_mod.key.clone(), game_mod.sha256.clone());
    } else {
        approvals.remove(&game_mod.key);
    }
    crate::core::version::game_info::write_json_atomically(&settings_path(version), &approvals)?;
    Ok(approvals)
}
