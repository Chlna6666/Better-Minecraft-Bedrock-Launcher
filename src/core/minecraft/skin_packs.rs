use anyhow::{Context as _, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{debug, warn};

use crate::core::minecraft::paths::{GamePathOptions, GameTargetDir, game_target_dirs};
use crate::core::minecraft::resource_packs::{Header, Module, load_lang_map_for_pack};
use crate::core::minecraft::skin_pack_preview::generate_skin_preview;

const PARALLEL_SKIN_PREVIEW_THRESHOLD: usize = 12;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkinsJson {
    pub serialize_name: Option<String>,
    pub localization_name: Option<String>,
    #[serde(default)]
    pub skins: Vec<SkinJsonEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkinJsonEntry {
    pub localization_name: Option<String>,
    pub geometry: Option<String>,
    pub texture: Option<String>,
    #[serde(rename = "type")]
    pub skin_type: Option<String>,
    #[serde(default)]
    pub cape: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SkinPackManifest {
    pub format_version: Option<u32>,
    pub header: Option<Header>,
    pub modules: Option<Vec<Module>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct McSkinPackSkinInfo {
    pub display_name: String,
    pub localization_name: Option<String>,
    pub full_texture_path: Option<String>,
    pub preview_path: Option<String>,
    pub model_label: String,
}

impl McSkinPackSkinInfo {
    pub fn full_texture_path(&self) -> Option<&str> {
        self.full_texture_path.as_deref()
    }
}

#[derive(Debug, Clone)]
pub struct McSkinPackInfo {
    pub folder_name: String,
    pub folder_path: String,
    pub display_name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub icon_path: Option<String>,
    pub preview_path: Option<String>,
    pub first_full_skin_texture_path: Option<String>,
    pub skin_count: usize,
    pub slim_skin_count: usize,
    pub source: Option<String>,
    pub edition: Option<String>,
    pub source_root: Option<String>,
    pub gdk_user: Option<String>,
    pub skins: Vec<McSkinPackSkinInfo>,
}

impl McSkinPackInfo {
    pub fn first_full_skin_texture_path(&self) -> Option<&str> {
        self.first_full_skin_texture_path.as_deref().or_else(|| {
            self.skins
                .iter()
                .find_map(McSkinPackSkinInfo::full_texture_path)
        })
    }
}

pub(crate) fn read_skin_packs_standard(
    lang: &str,
    options: &GamePathOptions,
) -> Result<Vec<McSkinPackInfo>> {
    let pack_roots = game_target_dirs(options, GameTargetDir::SkinPacks);
    if pack_roots.is_empty() {
        return Ok(Vec::new());
    }

    let mut pack_folders = Vec::new();
    for root in pack_roots {
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pack_folders.push((path, root.clone()));
            }
        }
    }

    let results = pack_folders
        .into_par_iter()
        .filter_map(|(folder_path, base_path)| {
            match read_skin_pack_folder(&folder_path, &base_path, lang, options) {
                Ok(pack) => Some(pack),
                Err(error) => {
                    warn!("read skin pack failed {}: {error:?}", folder_path.display());
                    None
                }
            }
        })
        .collect::<Vec<_>>();

    debug!("Finished reading skin_packs ({})", results.len());
    Ok(results)
}

fn read_skin_pack_folder(
    folder_path: &Path,
    base_path: &Path,
    lang: &str,
    options: &GamePathOptions,
) -> Result<McSkinPackInfo> {
    let folder_name = folder_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("skin_pack")
        .to_string();
    let manifest_path = folder_path.join("manifest.json");
    let skins_path = folder_path.join("skins.json");

    let manifest_raw = read_lossy_text(&manifest_path, "皮肤包 manifest")?;
    let skins_raw = read_lossy_text(&skins_path, "skins.json")?;

    let clean_manifest = strip_json_comments(manifest_raw.trim_start_matches('\u{feff}'));
    let mut manifest_value: Value = serde_json::from_str(&clean_manifest)
        .with_context(|| format!("解析皮肤包 manifest 失败: {}", manifest_path.display()))?;
    let manifest_parsed = serde_json::from_value::<SkinPackManifest>(manifest_value.clone()).ok();
    let skins_json: SkinsJson = serde_json::from_str(&strip_json_comments(
        skins_raw.trim_start_matches('\u{feff}'),
    ))
    .with_context(|| format!("解析 skins.json 失败: {}", skins_path.display()))?;

    let lang_map = load_lang_map_for_pack(&folder_path.to_path_buf(), lang).unwrap_or_default();
    localize_manifest_header(&mut manifest_value, &lang_map);

    let manifest_header = manifest_parsed
        .as_ref()
        .and_then(|manifest| manifest.header.as_ref());
    let display_name = pack_display_name(
        &skins_json,
        manifest_header,
        &manifest_value,
        &lang_map,
        &folder_name,
    );
    let description = manifest_header.and_then(|_| {
        manifest_value
            .get("header")?
            .get("description")?
            .as_str()
            .map(ToString::to_string)
    });
    let version = manifest_header.and_then(|header| version_label(header.version.as_deref()));
    let icon_path = first_existing_file(
        folder_path,
        &["pack_icon.png", "pack_icon.jpg", "pack_icon.jpeg"],
    );
    let skins = skin_infos_from_json(folder_path, &skins_json, &lang_map);
    let first_skin = skins.iter().find(|skin| skin.full_texture_path.is_some());
    let first_full_skin_texture_path = first_skin.and_then(|skin| skin.full_texture_path.clone());
    let preview_path = first_skin.and_then(|skin| skin.preview_path.clone());
    let slim_skin_count = skins
        .iter()
        .filter(|skin| skin.model_label.eq_ignore_ascii_case("Alex"))
        .count();

    Ok(McSkinPackInfo {
        folder_name,
        folder_path: folder_path.to_string_lossy().to_string(),
        display_name,
        description,
        version,
        icon_path,
        preview_path,
        first_full_skin_texture_path,
        skin_count: skins.len(),
        slim_skin_count,
        source: Some(format!("{:?}", options.build_type)),
        edition: Some(format!("{:?}", options.edition)),
        source_root: Some(base_path.to_string_lossy().to_string()),
        gdk_user: gdk_user_from_base_path(base_path),
        skins,
    })
}

fn skin_infos_from_json(
    folder_path: &Path,
    skins_json: &SkinsJson,
    lang_map: &HashMap<String, String>,
) -> Vec<McSkinPackSkinInfo> {
    if skins_json.skins.len() < PARALLEL_SKIN_PREVIEW_THRESHOLD {
        return skins_json
            .skins
            .iter()
            .map(|skin| skin_info_from_json(folder_path, skins_json, skin, lang_map))
            .collect();
    }

    skins_json
        .skins
        .par_iter()
        .map(|skin| skin_info_from_json(folder_path, skins_json, skin, lang_map))
        .collect()
}

fn skin_info_from_json(
    folder_path: &Path,
    skins_json: &SkinsJson,
    skin: &SkinJsonEntry,
    lang_map: &HashMap<String, String>,
) -> McSkinPackSkinInfo {
    let display_name = skin_display_name(skins_json, skin, lang_map);
    let texture_path = skin
        .texture
        .as_ref()
        .map(|texture| folder_path.join(texture))
        .filter(|path| path.is_file());
    let model_label = skin
        .geometry
        .as_deref()
        .map(model_label_from_geometry)
        .unwrap_or_else(|| "Steve".to_string());
    let preview_path = texture_path
        .as_ref()
        .and_then(|path| match generate_skin_preview(path) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(error) => {
                warn!("generate skin preview failed {}: {error:?}", path.display());
                None
            }
        });

    McSkinPackSkinInfo {
        display_name,
        localization_name: skin.localization_name.clone(),
        full_texture_path: texture_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        preview_path,
        model_label,
    }
}

fn pack_display_name(
    skins_json: &SkinsJson,
    manifest_header: Option<&Header>,
    manifest_value: &Value,
    lang_map: &HashMap<String, String>,
    folder_name: &str,
) -> String {
    manifest_value
        .get("header")
        .and_then(|header| header.get("name"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            skins_json
                .localization_name
                .as_deref()
                .and_then(|name| lang_map.get(&format!("skinpack.{name}")).cloned())
        })
        .or_else(|| manifest_header.and_then(|header| header.name.clone()))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| folder_name.to_string())
}

fn skin_display_name(
    skins_json: &SkinsJson,
    skin: &SkinJsonEntry,
    lang_map: &HashMap<String, String>,
) -> String {
    let pack_name = skins_json.localization_name.as_deref();
    let skin_name = skin.localization_name.as_deref();

    pack_name
        .zip(skin_name)
        .and_then(|(pack, skin)| lang_map.get(&format!("skin.{pack}.{skin}")).cloned())
        .or_else(|| skin_name.and_then(|skin| lang_map.get(&format!("skin.{skin}")).cloned()))
        .or_else(|| skin_name.map(ToString::to_string))
        .unwrap_or_else(|| "Skin".to_string())
}

fn model_label_from_geometry(geometry: &str) -> String {
    if geometry.to_ascii_lowercase().contains("slim") {
        "Alex".to_string()
    } else {
        "Steve".to_string()
    }
}

fn version_label(version: Option<&[u32]>) -> Option<String> {
    let version = version?;
    (!version.is_empty()).then(|| {
        version
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(".")
    })
}

fn localize_manifest_header(manifest_value: &mut Value, lang_map: &HashMap<String, String>) {
    let Some(header) = manifest_value
        .get_mut("header")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };

    for key in ["name", "description"] {
        let Some(value) = header.get_mut(key) else {
            continue;
        };
        let Some(raw) = value.as_str() else {
            continue;
        };
        if let Some(translated) = lang_map.get(raw) {
            *value = Value::String(translated.clone());
        }
    }
}

fn first_existing_file(folder_path: &Path, file_names: &[&str]) -> Option<String> {
    file_names
        .iter()
        .map(|file_name| folder_path.join(file_name))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
}

fn read_lossy_text(path: &Path, label: &str) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("读取 {label} 失败: {}", path.display()))?;
    Ok(lossy_text(&bytes))
}

fn lossy_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn gdk_user_from_base_path(base_path: &Path) -> Option<String> {
    base_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|value| value.to_string_lossy().to_string())
}

fn strip_json_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while let Some(character) = chars.next() {
        if in_line_comment {
            if character == '\n' {
                in_line_comment = false;
                output.push(character);
            }
            continue;
        }
        if in_block_comment {
            if character == '*' && chars.peek().is_some_and(|next| *next == '/') {
                chars.next();
                in_block_comment = false;
            }
            continue;
        }
        if in_string {
            output.push(character);
            if character == '\\' {
                if let Some(next) = chars.next() {
                    output.push(next);
                }
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        if character == '"' {
            in_string = true;
            output.push(character);
            continue;
        }
        if character == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    in_line_comment = true;
                    continue;
                }
                Some('*') => {
                    chars.next();
                    in_block_comment = true;
                    continue;
                }
                _ => {}
            }
        }
        output.push(character);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_display_name_uses_pack_scoped_localization() {
        let skins_json = SkinsJson {
            serialize_name: None,
            localization_name: Some("alleis".to_string()),
            skins: Vec::new(),
        };
        let skin = SkinJsonEntry {
            localization_name: Some("birthday".to_string()),
            geometry: Some("geometry.humanoid.custom".to_string()),
            texture: Some("birthday.png".to_string()),
            skin_type: Some("free".to_string()),
            cape: None,
            extra: serde_json::Map::new(),
        };
        let lang_map = HashMap::from([(
            "skin.alleis.birthday".to_string(),
            "Birthday Skin".to_string(),
        )]);

        assert_eq!(
            skin_display_name(&skins_json, &skin, &lang_map),
            "Birthday Skin"
        );
    }

    #[test]
    fn full_texture_path_is_separate_from_head_preview_path() {
        let skin = McSkinPackSkinInfo {
            display_name: "Alex".to_string(),
            localization_name: None,
            full_texture_path: Some("packs/alex.png".to_string()),
            preview_path: Some("cache/skin_previews/head.png".to_string()),
            model_label: "Alex".to_string(),
        };
        let pack = McSkinPackInfo {
            folder_name: "pack".to_string(),
            folder_path: "packs".to_string(),
            display_name: "Pack".to_string(),
            description: None,
            version: None,
            icon_path: None,
            preview_path: skin.preview_path.clone(),
            first_full_skin_texture_path: None,
            skin_count: 1,
            slim_skin_count: 1,
            source: None,
            edition: None,
            source_root: None,
            gdk_user: None,
            skins: vec![skin],
        };

        assert_eq!(pack.first_full_skin_texture_path(), Some("packs/alex.png"));
        assert_ne!(
            pack.first_full_skin_texture_path(),
            pack.preview_path.as_deref()
        );
    }

    #[test]
    fn skins_json_accepts_lossy_utf8_in_strings() {
        let raw = b"{\"skins\":[{\"localization_name\":\"bad\xffname\",\"texture\":\"a.png\"}]}";
        let parsed: SkinsJson = serde_json::from_str(&lossy_text(raw))
            .unwrap_or_else(|error| panic!("lossy skins json should parse: {error}"));

        assert_eq!(parsed.skins.len(), 1);
        assert_eq!(
            parsed.skins[0].localization_name.as_deref(),
            Some("bad\u{fffd}name")
        );
    }
}
