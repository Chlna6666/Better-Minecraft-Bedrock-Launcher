use anyhow::Context;
use image::ImageReader;
use std::io::Cursor;

include!(concat!(env!("OUT_DIR"), "/entity_avatar_assets.rs"));

/// Decodes the entity sprites embedded by `build.rs` into GPUI-ready RGBA data.
///
/// Entity overlays intentionally have no instance-specific disk cache. The
/// generated PNG files are application assets, so the resource pack version
/// cannot leave stale avatar files behind and the first map view uses one
/// deterministic catalog for every world.
pub(crate) fn load_generated_entity_avatars_rgba() -> Vec<(String, u32, u32, Vec<u8>)> {
    let mut avatars = Vec::with_capacity(ENTITY_AVATAR_ASSETS.len());
    for &(identifier, path) in ENTITY_AVATAR_ASSETS {
        let Some(bytes) = crate::assets::asset_source::load_image_asset(path)
            .ok()
            .flatten()
        else {
            tracing::debug!(identifier, path, "embedded entity avatar is missing");
            continue;
        };
        let image = match ImageReader::new(Cursor::new(bytes.as_ref()))
            .with_guessed_format()
            .context("detect embedded entity avatar format")
            .and_then(|reader| reader.decode().context("decode embedded entity avatar"))
        {
            Ok(image) => image.into_rgba8(),
            Err(error) => {
                tracing::debug!(
                    ?error,
                    identifier,
                    path,
                    "failed to decode embedded entity avatar"
                );
                continue;
            }
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
        let actual_identifiers = avatars
            .iter()
            .map(|(identifier, ..)| identifier.as_str())
            .collect::<Vec<_>>();
        let expected_identifiers = ENTITY_AVATAR_ASSETS
            .iter()
            .map(|(identifier, _)| *identifier)
            .collect::<Vec<_>>();
        assert_eq!(
            actual_identifiers, expected_identifiers,
            "every manifest entity avatar must be embedded and decodable"
        );
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
