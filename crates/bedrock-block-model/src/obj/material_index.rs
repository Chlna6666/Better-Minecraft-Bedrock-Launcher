use super::{HashMap, ObjResolvedTexture, ObjTextureResolver, Path, PathBuf};
use std::hash::{Hash, Hasher};

/// One resolved-or-known-missing material lookup, persisted per pack stamp.
type MaterialIndexEntry = Option<[String; 3]>;

/// Disk-backed `(material, normal) -> resolved texture` index.
///
/// The stamp binds the file to one ordered pack-root set and lookup revision;
/// a different stamp reads and writes a different file, so game-version or
/// pack-list changes can never serve stale entries.
#[derive(Debug, Default)]
pub(super) struct MaterialIndexFile {
    stamp: u64,
    directory: PathBuf,
    entries: HashMap<String, MaterialIndexEntry>,
    dirty: bool,
}

impl MaterialIndexFile {
    fn path(&self) -> PathBuf {
        self.directory
            .join(format!("material_index_{:016x}.json", self.stamp))
    }

    fn load(&mut self) {
        let Ok(text) = std::fs::read_to_string(self.path()) else {
            return;
        };
        #[derive(serde::Deserialize)]
        struct File {
            entries: HashMap<String, MaterialIndexEntry>,
        }
        if let Ok(file) = serde_json::from_str::<File>(&text) {
            self.entries = file.entries;
        }
    }

    fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        #[derive(serde::Serialize)]
        struct File<'a> {
            stamp: u64,
            entries: &'a HashMap<String, MaterialIndexEntry>,
        }
        let Ok(text) = serde_json::to_string(&File {
            stamp: self.stamp,
            entries: &self.entries,
        }) else {
            return;
        };
        if std::fs::create_dir_all(&self.directory).is_ok()
            && std::fs::write(self.path(), text).is_ok()
        {
            self.dirty = false;
        }
    }
}

fn material_index_key(material: &str, normal: [i32; 3]) -> String {
    format!("{material}|{},{},{}", normal[0], normal[1], normal[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_pack() -> tempfile::TempDir {
        let pack = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(pack.path().join("textures/blocks")).unwrap();
        std::fs::write(pack.path().join("textures/blocks/stone.png"), b"png").unwrap();
        pack
    }

    #[test]
    fn known_missing_hit_does_not_fall_through_to_pack_resolution() {
        let pack = create_pack();
        let cache = tempfile::tempdir().unwrap();
        let resolver = ObjTextureResolver::with_pack_roots([pack.path()], "textures");
        assert!(resolver.texture_for("minecraft_stone", [0, 1, 0]).is_some());
        resolver.open_material_index(cache.path(), resolver.material_index_revision().unwrap());
        resolver
            .material_index
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .entries
            .insert(material_index_key("minecraft_stone", [0, 1, 0]), None);
        assert!(resolver.texture_for("minecraft_stone", [0, 1, 0]).is_none());
    }

    #[test]
    fn model_only_pack_updates_invalidate_cached_material_sources() {
        for folder in ["blocks", "definitions/blocks"] {
            let textures = create_pack();
            std::fs::write(textures.path().join("textures/blocks/other.png"), b"png").unwrap();
            let models = tempfile::tempdir().unwrap();
            let directory = models.path().join(folder);
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join("split_block.json");
            let definition = |texture| {
                serde_json::json!({
                "minecraft:block": {
                    "description": { "identifier": "minecraft:split_block" },
                    "components": { "minecraft:material_instances": { "*": { "texture": texture } } }
                }
            }).to_string()
            };
            let resolver = || {
                ObjTextureResolver::with_pack_roots([models.path(), textures.path()], "textures")
            };
            std::fs::write(&path, definition("stone")).unwrap();
            let cache = tempfile::tempdir().unwrap();
            let before = resolver();
            let stamp = before.material_index_revision().unwrap();
            before.open_material_index(cache.path(), stamp);
            assert_eq!(
                before
                    .texture_for("minecraft_split_block_side", [0, 0, 1])
                    .unwrap()
                    .relative_path,
                "textures/blocks/stone.png"
            );
            before.flush_material_index();
            std::fs::write(&path, definition("other")).unwrap();
            let after = resolver();
            let updated = after.material_index_revision().unwrap();
            assert_ne!(updated, stamp);
            after.open_material_index(cache.path(), updated);
            assert_eq!(
                after
                    .texture_for("minecraft_split_block_side", [0, 0, 1])
                    .unwrap()
                    .relative_path,
                "textures/blocks/other.png"
            );
        }
    }

    #[test]
    fn geometry_material_slot_changes_invalidate_revision() {
        let pack = create_pack();
        std::fs::create_dir_all(pack.path().join("models")).unwrap();
        let path = pack.path().join("models/slots.json");
        let geometry = |slot| {
            serde_json::json!({
                "geometry.test": { "bones": [{"name": "root", "cubes": [{
                    "origin": [0, 0, 0], "size": [1, 1, 1], "material_instance": slot
                }]}] }
            })
            .to_string()
        };
        std::fs::write(&path, geometry("stone")).unwrap();
        let before = ObjTextureResolver::with_pack_roots([pack.path()], "textures");
        std::fs::write(&path, geometry("other")).unwrap();
        let after = ObjTextureResolver::with_pack_roots([pack.path()], "textures");
        assert_ne!(
            before.material_index_revision(),
            after.material_index_revision()
        );
    }

    #[test]
    fn revision_tracks_in_place_definitions_paths_pack_order_and_export_directory() {
        let pack = create_pack();
        let atlas = pack.path().join("textures/terrain_texture.json");
        std::fs::write(
            &atlas,
            r#"{"texture_data":{"test":{"textures":"textures/blocks/stone"}}}"#,
        )
        .unwrap();
        let resolver = || ObjTextureResolver::with_pack_roots([pack.path()], "textures");
        let original = resolver().material_index_revision().unwrap();
        assert_eq!(resolver().material_index_revision(), Some(original));
        // Same directory and same byte length, different path mapping.
        std::fs::write(
            &atlas,
            r#"{"texture_data":{"test":{"textures":"textures/blocks/other"}}}"#,
        )
        .unwrap();
        let changed = resolver().material_index_revision().unwrap();
        assert_ne!(changed, original);
        std::fs::write(
            pack.path().join("blocks.json"),
            r#"{"test":{"textures":"test"}}"#,
        )
        .unwrap();
        let blocks = resolver().material_index_revision().unwrap();
        assert_ne!(blocks, changed);
        std::fs::write(
            pack.path().join("blocks.json"),
            r#"{"test":{"textures":"stone"}}"#,
        )
        .unwrap();
        assert_ne!(resolver().material_index_revision(), Some(blocks));
        let cache = tempfile::tempdir().unwrap();
        let before = resolver();
        let revision = before.material_index_revision().unwrap();
        before.open_material_index(cache.path(), revision);
        assert!(
            before
                .texture_for("minecraft_new_block", [0, 1, 0])
                .is_none()
        );
        before.flush_material_index();
        std::fs::write(pack.path().join("textures/blocks/new_block.png"), b"png").unwrap();
        let after = resolver();
        let updated = after.material_index_revision().unwrap();
        assert_ne!(updated, revision);
        after.open_material_index(cache.path(), updated);
        assert!(
            after
                .texture_for("minecraft_new_block", [0, 1, 0])
                .is_some()
        );
        let other = create_pack();
        let ordered = ObjTextureResolver::with_pack_roots([pack.path(), other.path()], "textures");
        let reversed = ObjTextureResolver::with_pack_roots([other.path(), pack.path()], "textures");
        assert_ne!(
            ordered.material_index_revision(),
            reversed.material_index_revision()
        );
        let exported = ObjTextureResolver::with_pack_roots([pack.path()], "export");
        assert_ne!(
            after.material_index_revision(),
            exported.material_index_revision()
        );
    }
}

impl ObjTextureResolver {
    /// Revision of loaded material definitions, ordered packs and resource paths.
    ///
    /// Includes the lookup schema and export directory. Pixel content does not
    /// affect path resolution. Returns `None` after an incomplete path scan.
    #[must_use]
    pub fn material_index_revision(&self) -> Option<u64> {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        "bedrock-block-model/material-index/2".hash(&mut hash);
        self.texture_directory_name.hash(&mut hash);
        self.pack_lookup_order.hash(&mut hash);
        // Custom block definitions and geometry can select material slots before
        // pack-level atlas lookup, including packs that contain only models.
        self.models.blocks.len().hash(&mut hash);
        for (name, block) in &self.models.blocks {
            name.hash(&mut hash);
            block.raw.hash(&mut hash);
        }
        self.models.terrain_textures.textures.len().hash(&mut hash);
        for (name, texture) in &self.models.terrain_textures.textures {
            name.hash(&mut hash);
            texture.paths.hash(&mut hash);
        }
        self.models.geometries.geometries.len().hash(&mut hash);
        for (name, geometry) in &self.models.geometries.geometries {
            name.hash(&mut hash);
            geometry.raw.hash(&mut hash);
        }
        for pack in &self.packs {
            pack.hash_material_index(&mut hash)?;
        }
        Some(hash.finish())
    }

    /// Attaches a versioned material-index cache under `directory`.
    ///
    /// Use [`Self::material_index_revision`] for `stamp`. The cache file is
    /// per-revision. [`Self::texture_for`] adds entries in memory;
    /// [`Self::flush_material_index`] persists them at a caller-selected boundary.
    pub fn open_material_index(&self, directory: &Path, stamp: u64) {
        let mut file = MaterialIndexFile {
            stamp,
            directory: directory.to_path_buf(),
            entries: HashMap::new(),
            dirty: false,
        };
        file.load();
        *self
            .material_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(file);
    }

    /// Writes pending material-index entries to disk; a no-op without a cache.
    pub fn flush_material_index(&self) {
        if let Some(file) = self
            .material_index
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_mut()
        {
            file.save_if_dirty();
        }
    }

    /// Resolves a material face, including cached missing results. Pack lookup
    /// and file restoration run outside the shared index lock.
    #[must_use]
    pub fn texture_for(&self, material: &str, normal: [i32; 3]) -> Option<ObjResolvedTexture> {
        let key = material_index_key(material, normal);
        let cached = {
            let state = self
                .material_index
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let Some(file) = state.as_ref() else {
                drop(state);
                return self.texture_for_uncached(material, normal);
            };
            file.entries.get(&key).cloned()
        };
        match cached {
            Some(None) => return None,
            Some(Some(entry)) => {
                if let Some(texture) = self.material_index_entry_texture(&entry) {
                    return Some(texture);
                }
            }
            None => {}
        }
        let resolved = self.texture_for_uncached(material, normal);
        let entry = resolved.as_ref().and_then(|texture| {
            let index = self
                .packs
                .iter()
                .position(|pack| pack.root == texture.pack_root)?;
            let source = texture
                .source_path
                .path()
                .strip_prefix(&texture.pack_root)
                .ok()?
                .to_string_lossy()
                .into_owned();
            Some([index.to_string(), source, texture.relative_path.clone()])
        });
        let mut state = self
            .material_index
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(file) = state.as_mut() {
            file.entries.insert(key, entry);
            file.dirty = true;
        }
        resolved
    }

    fn material_index_entry_texture(&self, entry: &[String; 3]) -> Option<ObjResolvedTexture> {
        let index: usize = entry[0].parse().ok()?;
        let pack = self.packs.get(index)?;
        let file = pack.texture_file_for_path(Path::new(&entry[1]))?;
        Some(pack.resolved_texture(&self.texture_directory_name, file))
    }
}
