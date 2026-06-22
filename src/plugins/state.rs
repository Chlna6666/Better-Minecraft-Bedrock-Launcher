use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const PLUGIN_STATE_FILE: &str = ".bmcbl-plugin-state.toml";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct PluginStateFile {
    #[serde(default)]
    plugins: BTreeMap<String, PluginStateEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PluginStateEntry {
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_enabled() -> bool {
    true
}

pub fn state_file_path(plugins_dir: &Path) -> PathBuf {
    plugins_dir.join(PLUGIN_STATE_FILE)
}

pub fn disabled_plugins(plugins_dir: &Path) -> BTreeSet<String> {
    read_state_file(plugins_dir)
        .map(|state| {
            state
                .plugins
                .into_iter()
                .filter_map(|(plugin_id, entry)| (!entry.enabled).then_some(plugin_id))
                .collect()
        })
        .unwrap_or_default()
}

pub fn set_plugin_enabled(plugins_dir: &Path, plugin_id: &str, enabled: bool) -> Result<()> {
    let mut state = read_state_file(plugins_dir)?;
    if enabled {
        state.plugins.remove(plugin_id);
    } else {
        state
            .plugins
            .insert(plugin_id.to_string(), PluginStateEntry { enabled: false });
    }
    write_state_file(plugins_dir, &state)
}

pub fn remove_plugin_state(plugins_dir: &Path, plugin_id: &str) -> Result<()> {
    let mut state = read_state_file(plugins_dir)?;
    state.plugins.remove(plugin_id);
    write_state_file(plugins_dir, &state)
}

fn read_state_file(plugins_dir: &Path) -> Result<PluginStateFile> {
    let path = state_file_path(plugins_dir);
    if !path.exists() {
        return Ok(PluginStateFile::default());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn write_state_file(plugins_dir: &Path, state: &PluginStateFile) -> Result<()> {
    fs::create_dir_all(plugins_dir)
        .with_context(|| format!("create plugin state dir {}", plugins_dir.display()))?;
    let path = state_file_path(plugins_dir);
    let text = toml::to_string_pretty(state).context("serialize plugin state")?;
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, text).with_context(|| format!("write {}", temp_path.display()))?;
    fs::rename(&temp_path, &path)
        .with_context(|| format!("replace {} with {}", path.display(), temp_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_state_roundtrips() {
        let root = unique_temp_dir("bmcbl-plugin-state");
        set_plugin_enabled(&root, "hello-plugin", false).expect("disable should persist");

        let disabled = disabled_plugins(&root);
        assert!(disabled.contains("hello-plugin"));

        set_plugin_enabled(&root, "hello-plugin", true).expect("enable should persist");
        assert!(!disabled_plugins(&root).contains("hello-plugin"));
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }
}
