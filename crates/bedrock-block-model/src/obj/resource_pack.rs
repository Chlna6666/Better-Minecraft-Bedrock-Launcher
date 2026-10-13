use super::{
    HashMap, ObjResolvedTexture, PackResources, Path, PathBuf, ResourceFile, Result, Value,
    block_definition, obj_normalize_texture_key, obj_texture_key_from_value, path_extension_eq,
    path_starts_with_directory, prefixed_relative_path, read_texture_data_maps,
    relative_path_string, replace_path_extension, texture_candidate_relatives,
};
use crate::json::read_resource_json;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ObjResourcePack {
    root: PathBuf,
    texture_root: PathBuf,
    blocks: Value,
    texture_data: HashMap<String, String>,
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
                        if let Some(path) = super::terrain_texture_path(value) {
                            texture_data.insert(key.clone(), path);
                        }
                    }
                }
            }
        }
        Ok(Some(Self {
            root,
            texture_root,
            blocks,
            texture_data,
            resources,
        }))
    }

    pub(super) fn block_texture_key_for(&self, block: &str, normal: [i32; 3]) -> Option<String> {
        let object = block_definition(&self.blocks, block)?;
        let textures = object
            .get("textures")
            .or_else(|| object.get("texture"))
            .or_else(|| object.get("carried_textures"))?;
        obj_texture_key_from_value(textures, normal)
    }

    pub(super) fn texture_path_for_key(&self, texture_key: &str) -> Option<ResourceFile> {
        let normalized = self
            .texture_data
            .get(texture_key)
            .map(String::as_str)
            .unwrap_or(texture_key);
        let normalized = obj_normalize_texture_key(normalized);
        for relative in texture_candidate_relatives(&normalized) {
            for path in [
                PathBuf::from("textures").join(&relative),
                PathBuf::from(&relative),
            ] {
                if let Some(file) = self.texture_file(&path) {
                    return Some(file);
                }
            }
        }
        None
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
        }
    }
}
