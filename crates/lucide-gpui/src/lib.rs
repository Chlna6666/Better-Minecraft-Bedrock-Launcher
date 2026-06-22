use anyhow::anyhow;
use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

pub struct Assets;

pub(crate) mod registry {
    use super::*;

    fn registry() -> &'static RwLock<HashMap<&'static str, &'static [u8]>> {
        static REGISTRY: OnceLock<RwLock<HashMap<&'static str, &'static [u8]>>> = OnceLock::new();
        REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
    }

    pub fn register(path: &'static str, bytes: &'static [u8]) {
        if let Ok(mut map) = registry().write() {
            map.insert(path, bytes);
        }
    }

    pub fn get(path: &str) -> Option<&'static [u8]> {
        let map = registry().read().ok()?;
        map.get(path).copied()
    }

    pub fn list(prefix: &str) -> Vec<SharedString> {
        let map = match registry().read() {
            Ok(map) => map,
            Err(_) => return Vec::new(),
        };
        map.keys()
            .filter(|key| key.starts_with(prefix))
            .map(|key| SharedString::from(*key))
            .collect()
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(registry::get(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path.is_empty() || path == "lucide" || path == "lucide/" {
            return Ok(registry::list("lucide/"));
        }

        if registry::get(path).is_some() {
            return Ok(vec![SharedString::from(path.to_string())]);
        }

        if path.starts_with("lucide/") || path == "lucide" {
            return Err(anyhow!("could not find asset at path \"{path}\"").into());
        }

        Ok(Vec::new())
    }
}

include!(concat!(env!("OUT_DIR"), "/icons_gen.rs"));
