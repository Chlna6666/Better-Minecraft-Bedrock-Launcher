//! Resource-pack local-light descriptions; rendering and propagation belong to consumers.
use super::*;

/// Resource-authored local light color, expressed as sRGB bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalLightDefinition {
    pub color: [u8; 3],
    /// `point_light` supports direct specular lighting; `static_light` describes diffuse only.
    pub is_point: bool,
}
impl ObjTextureResolver {
    /// Resolves local-light overrides in resource-pack priority order.
    #[must_use]
    pub fn local_light_for(&self, block: &str) -> Option<LocalLightDefinition> {
        self.pack_lookup_order
            .iter()
            .find_map(|&index| self.packs[index].local_light(block))
    }
    /// Resolves an environment texture by logical pack-relative path, following pack priority.
    #[must_use]
    pub fn environment_texture(&self, path: &str) -> Option<ObjResolvedTexture> {
        self.texture_path_for_key(path)
            .map(|(pack, source)| pack.resolved_texture(&self.texture_directory_name, source))
    }
}
pub(super) fn read_local_lights(
    resources: &PackResources,
) -> Result<HashMap<String, LocalLightDefinition>> {
    let Some(file) = resources.get(Path::new("local_lighting/local_lighting.json")) else {
        return Ok(HashMap::new());
    };
    let value = crate::json::read_resource_json(&file)?;
    let mut lights = HashMap::new();
    if let Some(definitions) = value
        .get("minecraft:local_light_settings")
        .and_then(Value::as_object)
    {
        for (block, definition) in definitions {
            let invalid = || {
                crate::BlockModelError::Message(format!(
                    "invalid local light {block} in {}",
                    file.path().display()
                ))
            };
            let color = definition
                .get("light_color")
                .and_then(parse_color)
                .ok_or_else(invalid)?;
            let kind = definition
                .get("light_type")
                .and_then(Value::as_str)
                .filter(|kind| matches!(*kind, "point_light" | "static_light"))
                .ok_or_else(invalid)?;
            lights.insert(
                block.clone(),
                LocalLightDefinition {
                    color,
                    is_point: kind == "point_light",
                },
            );
        }
    }
    Ok(lights)
}
fn parse_color(value: &Value) -> Option<[u8; 3]> {
    if let Some(hex) = value.as_str().and_then(|value| value.strip_prefix('#')) {
        if hex.len() != 6 || !hex.is_ascii() {
            return None;
        }
        return Some([
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        ]);
    }
    let array = value.as_array().filter(|value| value.len() == 3)?;
    let mut color = [0; 3];
    for (out, input) in color.iter_mut().zip(array) {
        let input = input
            .as_f64()
            .filter(|value| (0. ..=255.).contains(value))?;
        *out = input.round() as u8;
    }
    Some(color)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_colors_accept_documented_hex_and_numeric_channels() {
        assert_eq!(
            parse_color(&serde_json::json!("#EFE39D")),
            Some([239, 227, 157])
        );
        assert_eq!(
            parse_color(&serde_json::json!([12, 34, 56])),
            Some([12, 34, 56])
        );
        assert_eq!(parse_color(&serde_json::json!([256, 0, 0])), None);
    }
    #[test]
    fn malformed_optional_lighting_preserves_pack_textures_and_reports_the_error() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("textures")).unwrap();
        std::fs::create_dir_all(root.join("local_lighting")).unwrap();
        std::fs::write(root.join("blocks.json"), "{}").unwrap();
        std::fs::write(root.join("local_lighting/local_lighting.json"), "{invalid").unwrap();
        let resources = PackResources::open(root).unwrap();
        let pack =
            super::super::resource_pack::ObjResourcePack::load(root.to_path_buf(), resources)
                .unwrap()
                .unwrap();
        assert!(pack.local_light_error.is_some());
        assert!(pack.local_light("minecraft:torch").is_none());
    }
    #[test]
    fn lighting_only_packs_resolve_authored_point_and_static_colors() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("local_lighting")).unwrap();
        std::fs::write(root.join("local_lighting/local_lighting.json"),r##"{"minecraft:local_light_settings":{"minecraft:torch":{"light_color":"#EFE39D","light_type":"point_light"},"minecraft:glowstone":{"light_color":[255,32,16],"light_type":"static_light"}}}"##).unwrap();
        let resolver = ObjTextureResolver::with_pack_roots([root], "textures");
        assert_eq!(
            resolver.local_light_for("minecraft:torch"),
            Some(LocalLightDefinition {
                color: [239, 227, 157],
                is_point: true
            })
        );
        assert_eq!(
            resolver.local_light_for("minecraft:glowstone"),
            Some(LocalLightDefinition {
                color: [255, 32, 16],
                is_point: false
            })
        );
    }
}
