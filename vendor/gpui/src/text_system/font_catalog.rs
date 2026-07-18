use crate::SharedString;
use parking_lot::RwLock;
use std::{collections::HashSet, sync::Arc};

pub(super) struct FontCatalog {
    available_names: RwLock<Option<Arc<[String]>>>,
    fallback_families: RwLock<Arc<[SharedString]>>,
}

impl Default for FontCatalog {
    fn default() -> Self {
        Self {
            available_names: RwLock::new(None),
            fallback_families: RwLock::new(default_fallback_families().into()),
        }
    }
}

impl FontCatalog {
    pub(super) fn available_font_names(
        &self,
        load_names: impl FnOnce() -> Vec<String>,
    ) -> Arc<[String]> {
        if let Some(names) = self.available_names.read().as_ref() {
            return Arc::clone(names);
        }

        let mut loaded_names = load_names();
        loaded_names.push(".SystemUIFont".to_owned());
        let loaded_names: Arc<[String]> = normalize_available_names(loaded_names).into();
        let mut names = self.available_names.write();
        Arc::clone(names.get_or_insert(loaded_names))
    }

    pub(super) fn invalidate_available_names(&self) {
        self.available_names.write().take();
    }

    pub(super) fn fallback_families(&self) -> Arc<[SharedString]> {
        Arc::clone(&self.fallback_families.read())
    }

    pub(super) fn set_fallback_families(&self, mut families: Vec<SharedString>) -> bool {
        families.extend(default_fallback_families());
        let families: Arc<[SharedString]> = normalize_fallback_names(families).into();
        let mut current = self.fallback_families.write();
        if current.as_ref() == families.as_ref() {
            false
        } else {
            *current = families;
            true
        }
    }
}

fn default_fallback_families() -> Vec<SharedString> {
    [
        ".ZedMono",
        ".ZedSans",
        "Helvetica",
        "Segoe UI",
        "Ubuntu",
        "Adwaita Sans",
        "Cantarell",
        "Noto Sans",
        "DejaVu Sans",
        "Arial",
    ]
    .into_iter()
    .map(SharedString::from)
    .collect()
}

fn normalize_available_names(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::with_capacity(names.len());
    let mut names = names
        .into_iter()
        .filter_map(|name| {
            let name = name.trim();
            if name.is_empty() || !seen.insert(name.to_lowercase()) {
                None
            } else {
                Some(name.to_owned())
            }
        })
        .collect::<Vec<_>>();
    names.sort_by_cached_key(|name| name.to_lowercase());
    names
}

fn normalize_fallback_names(names: Vec<SharedString>) -> Vec<SharedString> {
    let mut seen = HashSet::with_capacity(names.len());
    names
        .into_iter()
        .filter_map(|name| {
            let trimmed = name.trim();
            if trimmed.is_empty() || !seen.insert(trimmed.to_lowercase()) {
                None
            } else if trimmed.len() == name.len() {
                Some(name)
            } else {
                Some(SharedString::from(trimmed.to_owned()))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::FontCatalog;
    use crate::SharedString;

    #[test]
    fn fallback_names_preserve_user_order_before_defaults() {
        let catalog = FontCatalog::default();
        assert!(catalog.set_fallback_families(vec![
            " Custom Sans ".into(),
            "custom sans".into(),
            "Noto Sans".into(),
        ]));

        let families = catalog.fallback_families();
        assert_eq!(families[0], SharedString::from("Custom Sans"));
        assert_eq!(families[1], SharedString::from("Noto Sans"));
    }

    #[test]
    fn available_names_are_loaded_once_until_invalidated() {
        let catalog = FontCatalog::default();
        let mut loads = 0;

        let first = catalog.available_font_names(|| {
            loads += 1;
            vec!["Test Sans".to_owned()]
        });
        let second = catalog.available_font_names(|| {
            loads += 1;
            Vec::new()
        });

        assert_eq!(loads, 1);
        assert!(std::sync::Arc::ptr_eq(&first, &second));

        catalog.invalidate_available_names();
        let third = catalog.available_font_names(|| {
            loads += 1;
            vec!["Other Sans".to_owned()]
        });
        assert_eq!(loads, 2);
        assert!(!std::sync::Arc::ptr_eq(&first, &third));
    }
}
