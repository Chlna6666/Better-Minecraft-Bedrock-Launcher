use anyhow::anyhow;
use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

struct IconEntry {
    path: &'static str,
    offset: usize,
    len: usize,
}

include!(concat!(env!("OUT_DIR"), "/lucide_icons.rs"));

static ICON_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/lucide_icons.bin"));

#[doc(hidden)]
#[must_use]
pub const fn __icon_path(index: usize) -> &'static str {
    ICONS[index].path
}

fn icon(path: &str) -> Option<&'static IconEntry> {
    ICONS
        .binary_search_by(|entry| entry.path.cmp(path))
        .ok()
        .map(|index| &ICONS[index])
}

/// A BMCBL-private GPUI asset source for the selected Lucide icon payload.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let Some(icon) = icon(path) else {
            return Ok(None);
        };
        let end = icon
            .offset
            .checked_add(icon.len)
            .ok_or_else(|| anyhow!("Lucide icon range overflow for {path}"))?;
        let bytes = ICON_BYTES
            .get(icon.offset..end)
            .ok_or_else(|| anyhow!("invalid Lucide icon range for {path}"))?;
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|entry| entry.path.starts_with(path))
            .map(|entry| SharedString::from(entry.path))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_macro_matches_embedded_asset() {
        let path = icon!(circle_alert);
        let bytes = Assets
            .load(path)
            .expect("payload loads")
            .expect("icon exists");
        assert!(bytes.starts_with(b"<svg"));
    }

    #[test]
    fn generated_index_is_sorted_and_unique() {
        assert!(ICONS.windows(2).all(|pair| pair[0].path < pair[1].path));
        assert_eq!(ICON_BYTES.len(), ICON_BYTES_LEN);
    }
}
