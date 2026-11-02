//! Texture Set descriptions and same-pack references; no image decoding or GPU dependencies.
use super::*;

/// A Texture Set channel, resolved against the definition's own resource pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PbrTextureLayer {
    /// An encoded source image, including archived resources.
    Image(ObjResolvedTexture),
    /// A byte-valued constant channel; alpha defaults to 255.
    Constant([u8; 4]),
}

/// Channels supported by the renderer-independent Texture Set resolver.
/// Height maps and MERS subsurface are reported but are not converted to normals or scattering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjResolvedPbrTextureSet {
    pub color: Option<PbrTextureLayer>,
    pub normal: Option<PbrTextureLayer>,
    pub material: Option<PbrTextureLayer>,
    pub has_heightmap: bool,
    pub has_subsurface: bool,
}

impl ObjTextureResolver {
    /// Resolves a Texture Set next to the selected color texture in resource-pack priority order.
    /// Referenced images must belong to the definition's own pack; another pack cannot fill gaps.
    ///
    /// # Errors
    /// Returns malformed definitions, conflicting layers, invalid constants or missing images.
    pub fn pbr_texture_set_for(
        &self,
        texture: &ObjResolvedTexture,
    ) -> Result<Option<ObjResolvedPbrTextureSet>> {
        let relative = texture
            .source_path
            .path()
            .strip_prefix(&texture.pack_root)
            .map_err(|_| BlockModelError::Message("texture is outside its resource pack".into()))?;
        let sidecar = relative.with_extension("texture_set.json");
        for &index in &self.pack_lookup_order {
            if let Some(set) =
                self.packs[index].pbr_texture_set(&sidecar, &self.texture_directory_name)?
            {
                return Ok(Some(set));
            }
        }
        Ok(None)
    }
}

impl ObjResourcePack {
    pub(super) fn pbr_texture_set(
        &self,
        path: &Path,
        directory: &str,
    ) -> Result<Option<ObjResolvedPbrTextureSet>> {
        let Some(file) = self.pbr_resource(path) else {
            return Ok(None);
        };
        let value = crate::json::read_resource_json(&file)?;
        let set = value
            .get("minecraft:texture_set")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                BlockModelError::Message(format!("invalid Texture Set: {}", file.path().display()))
            })?;
        if set.contains_key("normal") && set.contains_key("heightmap")
            || set.contains_key("metalness_emissive_roughness")
                && set.contains_key("metalness_emissive_roughness_subsurface")
        {
            return Err(BlockModelError::Message(format!(
                "conflicting Texture Set layers: {}",
                file.path().display()
            )));
        }
        let layer = |name: &str| -> Result<Option<PbrTextureLayer>> {
            let Some(value) = set.get(name) else {
                return Ok(None);
            };
            if let Some(name) = value.as_str() {
                if let Some(hex) = name.strip_prefix('#') {
                    if (hex.len() == 6 || hex.len() == 8) && hex.is_ascii() {
                        let mut color = [255; 4];
                        for (i, dst) in color.iter_mut().enumerate().take(hex.len() / 2) {
                            *dst =
                                u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| {
                                    BlockModelError::Message(
                                        "invalid Texture Set hexadecimal color".into(),
                                    )
                                })?;
                        }
                        return Ok(Some(PbrTextureLayer::Constant(color)));
                    }
                    return Err(BlockModelError::Message(
                        "invalid Texture Set hexadecimal color".into(),
                    ));
                }
                let name = Path::new(name);
                if name.is_absolute()
                    || name
                        .components()
                        .any(|part| !matches!(part, Component::Normal(_)))
                {
                    return Err(BlockModelError::Message(
                        "Texture Set image must be pack-relative".into(),
                    ));
                }
                let relative = path.parent().unwrap_or(Path::new("")).join(name);
                let source = self.texture_file_for_path(&relative).ok_or_else(|| {
                    BlockModelError::Message(format!(
                        "missing same-pack Texture Set image: {}",
                        relative.display()
                    ))
                })?;
                return Ok(Some(PbrTextureLayer::Image(
                    self.resolved_texture(directory, source),
                )));
            }
            let array = value
                .as_array()
                .filter(|array| array.len() == 3 || array.len() == 4)
                .ok_or_else(|| {
                    BlockModelError::Message(
                        "Texture Set constant must contain 3 or 4 bytes".into(),
                    )
                })?;
            let mut color = [255; 4];
            for (dst, value) in color.iter_mut().zip(array) {
                *dst = value
                    .as_u64()
                    .and_then(|value| u8::try_from(value).ok())
                    .ok_or_else(|| {
                        BlockModelError::Message("Texture Set constant is outside 0..=255".into())
                    })?;
            }
            Ok(Some(PbrTextureLayer::Constant(color)))
        };
        Ok(Some(ObjResolvedPbrTextureSet {
            color: layer("color")?,
            normal: layer("normal")?,
            material: if set.contains_key("metalness_emissive_roughness_subsurface") {
                layer("metalness_emissive_roughness_subsurface")?
            } else {
                layer("metalness_emissive_roughness")?
            },
            has_heightmap: set.contains_key("heightmap"),
            has_subsurface: set.contains_key("metalness_emissive_roughness_subsurface"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn texture_set_resolves_channels_and_rejects_cross_pack_fallback() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for pack in [&first, &second] {
            fs::create_dir_all(pack.path().join("textures/blocks")).unwrap();
        }
        fs::write(first.path().join("textures/blocks/stone.png"), b"image").unwrap();
        fs::write(first.path().join("textures/blocks/stone_n.png"), b"normal").unwrap();
        let source = ObjResolvedTexture {
            source_path: ResourceFile::file(first.path().join("textures/blocks/stone.png")),
            relative_path: "textures/blocks/stone.png".into(),
            pack_root: first.path().to_path_buf(),
        };
        let sidecar = second.path().join("textures/blocks/stone.texture_set.json");
        fs::write(&sidecar,r##"{"format_version":"1.16.100","minecraft:texture_set":{"color":"#ff8000","metalness_emissive_roughness":[128,0,255]}}"##).unwrap();
        let resolver =
            ObjTextureResolver::with_pack_roots([first.path(), second.path()], "textures");
        let set = resolver.pbr_texture_set_for(&source).unwrap().unwrap();
        assert_eq!(
            set.color,
            Some(PbrTextureLayer::Constant([255, 128, 0, 255]))
        );
        assert_eq!(
            set.material,
            Some(PbrTextureLayer::Constant([128, 0, 255, 255]))
        );
        fs::write(
            &sidecar,
            r#"{"minecraft:texture_set":{"normal":"stone_n"}}"#,
        )
        .unwrap();
        // The first pack's normal is deliberately not a valid fallback for the second's set.
        assert!(resolver.pbr_texture_set_for(&source).is_err());
        fs::write(
            second.path().join("textures/blocks/stone_n.png"),
            b"same-pack normal",
        )
        .unwrap();
        let resolver =
            ObjTextureResolver::with_pack_roots([first.path(), second.path()], "textures");
        let set = resolver.pbr_texture_set_for(&source).unwrap().unwrap();
        let Some(PbrTextureLayer::Image(normal)) = set.normal else {
            panic!("missing normal")
        };
        assert_eq!(normal.pack_root, second.path());
        assert_eq!(normal.source_path.read().unwrap(), b"same-pack normal");
        fs::write(
            &sidecar,
            r#"{"minecraft:texture_set":{"normal":"stone_n","heightmap":"h"}}"#,
        )
        .unwrap();
        assert!(resolver.pbr_texture_set_for(&source).is_err());
        fs::write(
            &sidecar,
            r#"{"minecraft:texture_set":{"color":"../stone"}}"#,
        )
        .unwrap();
        assert!(resolver.pbr_texture_set_for(&source).is_err());
    }
}
