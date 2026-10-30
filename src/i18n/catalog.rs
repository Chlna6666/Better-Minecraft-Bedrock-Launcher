mod binary;

use super::Locale;
use binary::{Catalog, Translation};
use std::sync::OnceLock;

const EMBEDDED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/locales.bin"));
static CATALOG: OnceLock<Catalog<'static>> = OnceLock::new();

/// Read the binary header and borrow its UTF-8 pool before the first render.
pub(crate) fn initialize() {
    CATALOG.get_or_init(|| {
        Catalog::read(EMBEDDED).expect("embedded language data must match the binary format")
    });
}

pub(crate) fn lookup(locale: Locale, key: &str) -> Option<Translation<'static>> {
    let catalog = CATALOG
        .get()
        .expect("language catalog must be initialized before rendering");
    lookup_in(catalog, locale, key)
}

fn lookup_in<'a>(catalog: &Catalog<'a>, locale: Locale, key: &str) -> Option<Translation<'a>> {
    let index = catalog.key_index(key)?;
    catalog
        .translation(usize::from(locale.index()), index)
        .or_else(|| catalog.translation(usize::from(Locale::EnUs.index()), index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_settings_have_translations_in_every_embedded_locale() {
        let catalog = Catalog::read(EMBEDDED).expect("embedded catalog");
        for locale in Locale::all() {
            for key in [
                "LauncherSettings.render_engine",
                "LauncherSettings.render_engine_desc",
                "LauncherSettings.render_engine.auto",
                "LauncherSettings.render_engine.dx11",
                "LauncherSettings.render_engine.dx12",
                "LauncherSettings.render_engine.vulkan",
                "LauncherSettings.render_engine.opengl",
            ] {
                let index = catalog.key_index(key).expect("renderer translation key");
                let translation = catalog
                    .translation(usize::from(locale.index()), index)
                    .unwrap_or_else(|| panic!("missing {key} for {locale:?}"));
                assert!(!translation.text.is_empty(), "empty {key} for {locale:?}");
            }
        }
    }

    #[test]
    fn missing_translation_falls_back_to_english_by_key() {
        let original = Catalog::read(EMBEDDED).expect("embedded catalog");
        let index = original.key_index("common.cancel").expect("embedded key");
        let mut bytes = EMBEDDED.to_vec();
        let key_count = u32::from_le_bytes(bytes[8..12].try_into().expect("key count")) as usize;
        // Header (32), shared key index (3 per key), then locale-major values (7).
        let record = 32 + key_count * 3 + index * 7;
        bytes[record..record + 3].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
        let catalog = Catalog::read(&bytes).expect("catalog with missing Chinese entry");
        assert_eq!(
            lookup_in(&catalog, Locale::ZhCn, "common.cancel")
                .expect("English fallback")
                .text,
            "Cancel"
        );
        assert!(lookup_in(&catalog, Locale::ZhCn, "missing.translation.for.test").is_none());
    }
}
