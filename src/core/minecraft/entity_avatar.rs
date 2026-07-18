use anyhow::Context;
use image::ImageReader;
use std::io::Cursor;

const GENERATED_ICON_PREFIX: &str = "images/map/entity/";

/// Decodes the entity sprites embedded by `build.rs` into GPUI-ready RGBA data.
///
/// Entity overlays intentionally have no instance-specific disk cache. The
/// generated WebP files are application assets, so the resource pack version
/// cannot leave stale avatar files behind and the first map view uses one
/// deterministic catalog for every world.
pub(crate) fn load_generated_entity_avatars_rgba() -> Vec<(String, u32, u32, Vec<u8>)> {
    let mut avatars = Vec::new();
    for path in crate::assets::asset_source::list_image_assets()
        .into_iter()
        .filter(|path| path.starts_with(GENERATED_ICON_PREFIX) && path.ends_with(".webp"))
    {
        let Some(bytes) = crate::assets::asset_source::load_image_asset(path.as_ref())
            .ok()
            .flatten()
        else {
            tracing::debug!(path = %path, "embedded entity avatar is missing");
            continue;
        };
        let image = match ImageReader::new(Cursor::new(bytes.as_ref()))
            .with_guessed_format()
            .context("detect embedded entity avatar format")
            .and_then(|reader| reader.decode().context("decode embedded entity avatar"))
        {
            Ok(image) => image.into_rgba8(),
            Err(error) => {
                tracing::debug!(?error, path = %path, "failed to decode embedded entity avatar");
                continue;
            }
        };
        let Some(identifier) = path
            .strip_prefix(GENERATED_ICON_PREFIX)
            .and_then(|name| name.strip_suffix(".webp"))
        else {
            continue;
        };
        let (width, height) = image.dimensions();
        avatars.push((identifier.to_string(), width, height, image.into_raw()));
    }
    avatars.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    avatars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_entity_avatar_catalog_is_decodable() {
        let avatars = load_generated_entity_avatars_rgba();

        for expected in [
            "sheep",
            "pufferfish",
            "tropicalfish",
            "glow_squid",
            "slime",
            "silverfish",
            "magma_cube",
            "witch",
            "villager",
            "villager_v2",
            "zombie_villager",
            "snowball",
            "balloon",
            "armor_stand",
        ] {
            assert!(
                avatars
                    .iter()
                    .any(|(identifier, ..)| identifier == expected),
                "missing generated entity avatar: {expected}"
            );
        }
        assert!(avatars.iter().all(|(_, width, height, pixels)| {
            *width > 0
                && *height > 0
                && pixels.len()
                    == usize::try_from(*width)
                        .ok()
                        .and_then(|width| usize::try_from(*height).ok()?.checked_mul(width))
                        .and_then(|pixel_count| pixel_count.checked_mul(4))
                        .unwrap_or_default()
        }));
    }
}
