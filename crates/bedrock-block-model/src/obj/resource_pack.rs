use super::{
    HashMap, ObjResolvedTexture, PackResources, Path, PathBuf, ResourceFile, Result, Value,
    block_definition, obj_normalize_texture_key, obj_texture_key_from_value, path_extension_eq,
    path_starts_with_directory, prefixed_relative_path, read_texture_data_maps,
    relative_path_string, replace_path_extension,
};
use crate::json::read_resource_json;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ObjResourcePack {
    root: PathBuf,
    texture_root: PathBuf,
    /// True for the unversioned base `vanilla` pack; texture and definition
    /// lookups always give it priority over versioned `vanilla_*` overlays.
    is_base_vanilla: bool,
    blocks: Value,
    texture_data: HashMap<String, Vec<String>>,
    item_texture_data: HashMap<String, String>,
    flipbook_textures: BTreeSet<String>,
    resources: PackResources,
}

impl ObjResourcePack {
    pub(super) fn load(root: PathBuf, resources: PackResources) -> Result<Option<Self>> {
        let texture_root = root.join("textures");
        if !texture_root.is_dir()
            && !root.join("__brarchive/textures.brarchive").is_file()
            && !root.join("__brarchive/textures").is_dir()
        {
            return Ok(None);
        }
        let blocks = resources
            .get(Path::new("blocks.json"))
            .map(|file| read_resource_json(&file))
            .transpose()?
            .unwrap_or(Value::Null);
        let mut texture_data = read_texture_data_maps(&root, &texture_root);
        // Archive entries take precedence over matching loose atlas files.
        for path in ["terrain_texture.json", "textures/terrain_texture.json"] {
            if let Some(file) = resources
                .get(Path::new(path))
                .filter(ResourceFile::is_archived)
            {
                let value = read_resource_json(&file)?;
                if let Some(entries) = value.get("texture_data").and_then(Value::as_object) {
                    for (key, value) in entries {
                        let paths: Vec<String> = super::terrain_texture_paths(value)
                            .into_iter()
                            .map(|path| obj_normalize_texture_key(&path))
                            .collect();
                        if !paths.is_empty() {
                            texture_data.insert(key.clone(), paths);
                        }
                    }
                }
            }
        }
        let mut item_texture_data = HashMap::new();
        for path in ["item_texture.json", "textures/item_texture.json"] {
            if let Some(file) = resources.get(Path::new(path)) {
                let value = read_resource_json(&file)?;
                if let Some(entries) = value.get("texture_data").and_then(Value::as_object) {
                    for (key, value) in entries {
                        if let Some(path) = super::terrain_texture_path(value) {
                            item_texture_data.insert(key.clone(), path);
                        }
                    }
                }
            }
        }
        let mut flipbook_textures = BTreeSet::new();
        for path in ["flipbook_textures.json", "textures/flipbook_textures.json"] {
            let Some(file) = resources.get(Path::new(path)) else {
                continue;
            };
            let value = read_resource_json(&file)?;
            let Some(entries) = value.as_array() else {
                continue;
            };
            for entry in entries {
                let Some(texture) = entry.get("flipbook_texture").and_then(Value::as_str) else {
                    continue;
                };
                flipbook_textures.insert(obj_normalize_texture_key(texture));
            }
        }
        Ok(Some(Self {
            is_base_vanilla: super::is_base_vanilla_root(&root),
            root,
            texture_root,
            blocks,
            texture_data,
            item_texture_data,
            flipbook_textures,
            resources,
        }))
    }

    /// Whether this pack is the unversioned base `vanilla` pack.
    pub(super) fn is_base_vanilla(&self) -> bool {
        self.is_base_vanilla
    }

    /// Whether the given texture path is declared as an animated flipbook strip.
    pub(super) fn is_flipbook_texture(&self, relative_path: &str) -> bool {
        self.flipbook_textures
            .contains(&obj_normalize_texture_key(relative_path))
    }

    pub(super) fn block_texture_key_for(&self, block: &str, normal: [i32; 3]) -> Option<String> {
        let object = block_definition(&self.blocks, block)?;
        let textures = object
            .get("textures")
            .or_else(|| object.get("texture"))
            .or_else(|| object.get("carried_textures"))?;
        obj_texture_key_from_value(textures, normal)
    }

    /// Returns the terrain-texture path registered for one key in this pack.
    ///
    /// This is the documented default variant (the first declared entry); the
    /// full variant list is available through [`Self::texture_key_paths`].
    pub(super) fn texture_key_path(&self, texture_key: &str) -> Option<&str> {
        self.texture_key_paths(texture_key)
            .and_then(|paths| paths.first())
            .map(String::as_str)
    }

    /// Returns every terrain-texture path registered for one key, in
    /// declaration order. Arrays in `terrain_texture.json` are variants
    /// selected by legacy data value or block state at runtime.
    pub(super) fn texture_key_paths(&self, texture_key: &str) -> Option<&[String]> {
        self.texture_data.get(texture_key).map(Vec::as_slice)
    }

    /// Returns the item-texture path registered for one item in this pack.
    pub(super) fn item_texture_key_path(&self, item: &str) -> Option<&str> {
        self.item_texture_data.get(item).map(String::as_str)
    }

    /// Looks up one texture file by its logical path inside this pack.
    pub(super) fn texture_file_for_path(&self, path: &Path) -> Option<ResourceFile> {
        self.texture_file(path)
    }

    fn texture_file(&self, path: &Path) -> Option<ResourceFile> {
        if let Some(file) = self.resources.get(path) {
            return Some(file);
        }
        for extension in ["png", "tga", "jpg", "jpeg"] {
            let mut file_name = path.as_os_str().to_owned();
            file_name.push(format!(".{extension}"));
            if let Some(file) = self.resources.get(Path::new(&file_name)) {
                return Some(file);
            }
        }
        None
    }

    fn packaged_texture_path(&self, texture_directory_name: &str, source_path: &Path) -> String {
        if let Ok(relative) = source_path.strip_prefix(&self.texture_root) {
            return prefixed_relative_path(texture_directory_name, relative);
        }
        if let Ok(relative) = source_path.strip_prefix(&self.root)
            && let Some(path) = relative_path_string(relative)
        {
            if path_starts_with_directory(&path, texture_directory_name) {
                return path;
            }
            return format!("{texture_directory_name}/{path}");
        }
        source_path
            .file_name()
            .and_then(|name| name.to_str())
            .map_or_else(
                || format!("{texture_directory_name}/texture.png"),
                |name| format!("{texture_directory_name}/{name}"),
            )
    }

    pub(super) fn resolved_texture(
        &self,
        directory: &str,
        source_path: ResourceFile,
    ) -> ObjResolvedTexture {
        let relative = self.packaged_texture_path(directory, source_path.path());
        let relative_path = if path_extension_eq(source_path.path(), "tga") {
            replace_path_extension(&relative, "png")
        } else {
            relative
        };
        ObjResolvedTexture {
            source_path,
            relative_path,
            pack_root: self.root.clone(),
        }
    }
}
