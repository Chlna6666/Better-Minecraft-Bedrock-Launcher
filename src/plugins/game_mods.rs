//! Plugin-owned game mods and per-version approval of their exact payload.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use serde::Serialize;
use uuid::Uuid;

use serde::Deserialize;

use super::manifest::PluginManifest;

const MAX_MODULE_BYTES: u64 = 32 * 1024 * 1024;
static SETTINGS: Mutex<()> = Mutex::new(());
// Publication is owned by the active GPUI PluginRegistry, not by scanning
// arbitrary plugin directories from the game-launch worker.
static ACTIVE: OnceLock<Mutex<Vec<(GameMod, u64)>>> = OnceLock::new();

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


/// Immutable, per-launch description of a verified DLL already installed under
/// BMCBL/plugins/<plugin_id>/game-mods. This is *not* a staging path.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct LaunchModule {
    pub plugin_id: String,
    pub module_id: String,
    pub plugin_generation: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub platform: &'static str,
    pub phase: &'static str,
    pub relative_path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct LaunchPlan {
    pub plugins_root: PathBuf,
    pub modules: Vec<LaunchModule>,
    /// Host-only runtime generation; never sent over the wire.
    pub runtime_generations: BTreeMap<String, u64>,
}

/// Refresh on registry activation/reload/shutdown. Only enabled, successfully
/// activated WASM plugins may request an associated native DLL.
pub(crate) fn publish_active(mods: Vec<(GameMod, u64)>) {
    *ACTIVE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = mods;
}

pub(crate) fn launch_plan(game_version: &str) -> Result<Option<LaunchPlan>, String> {
    let live = ACTIVE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if live.is_empty() {
        return Ok(None);
    }
    let permissions = approvals(game_version)?;
    let plugins_root = crate::utils::file_ops::bmcbl_subdir("plugins")
        .canonicalize()
        .map_err(|error| format!("cannot resolve installed plugin root: {error}"))?;
    let disabled = super::state::disabled(&plugins_root);
    let mut generations = BTreeMap::<String, Uuid>::new();
    let mut runtime_generations = BTreeMap::<String, u64>::new();
    let mut modules = Vec::new();
    for (game_mod, runtime_generation) in &live {
        if !game_mod.supports(game_version) || !game_mod.enabled(&permissions) {
            continue;
        }
        let Some((plugin_id, module_id)) = game_mod.key.split_once('/') else {
            continue;
        };
        if disabled.contains(plugin_id) || !valid_module_id(plugin_id) || !valid_module_id(module_id) {
            continue;
        }
        let expected_plugin_root = plugins_root.join(plugin_id);
        let path = game_mod.payload.canonicalize();
        let Ok(path) = path else { continue };
        if !path.starts_with(&expected_plugin_root)
            || !path.is_file()
            || !game_mod.declaration.path.starts_with("game-mods/")
        {
            continue;
        }
        let bytes = match verified_payload(&path, &game_mod.sha256) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(plugin_id, module_id, %error, "approved game Mod changed before launch; refusing load");
                continue;
            }
        };
        let generation = generations
            .entry(plugin_id.to_string())
            .or_insert_with(Uuid::new_v4);
        runtime_generations.insert(plugin_id.to_string(), *runtime_generation);
        modules.push(LaunchModule {
            plugin_id: plugin_id.to_string(),
            module_id: module_id.to_string(),
            plugin_generation: generation.to_string(),
            sha256: game_mod.sha256.clone(),
            size_bytes: bytes.len() as u64,
            platform: "windows-x86_64",
            phase: match game_mod.declaration.phase {
                Phase::Preload => "preload",
                Phase::Ready => "ready",
            },
            relative_path: game_mod.declaration.path.clone(),
        });
    }
    if modules.is_empty() {
        return Ok(None);
    }
    // A launch_plan is a single <=64 KiB frame; enforce its limit when serializing.
    modules.sort_by(|a, b| (&a.plugin_id, &a.module_id).cmp(&(&b.plugin_id, &b.module_id)));
    if modules.len() > 256 {
        return Err("virtual Mod launch plan contains more than 256 modules".to_string());
    }
    Ok(Some(LaunchPlan { plugins_root, modules, runtime_generations }))
}

fn valid_module_id(value: &str) -> bool {
    (3..=64).contains(&value.len())
        && value.bytes().next().is_some_and(|ch| ch.is_ascii_lowercase())
        && !value.ends_with('-')
        && !value.contains("--")
        && value.bytes().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-')
}

fn verified_payload(path: &std::path::Path, expected_hash: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|error| format!("open Mod payload: {error}"))?
        .take(MAX_MODULE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read Mod payload: {error}"))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_MODULE_BYTES || !is_x64_dll(&bytes) {
        return Err("invalid or oversized Windows x64 DLL".into());
    }
    if bmcbl_plugin_runtime::engine::wasm_sha256_hex(&bytes) != expected_hash {
        return Err("HASH_MISMATCH: approved DLL bytes changed".into());
    }
    Ok(bytes)
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
        verified_payload(&game_mod.payload, &game_mod.sha256)?;
        approvals.insert(game_mod.key.clone(), game_mod.sha256.clone());
    } else {
        approvals.remove(&game_mod.key);
    }
    crate::core::version::game_info::write_json_atomically(&settings_path(version), &approvals)?;
    Ok(approvals)
}
