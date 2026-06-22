use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

static GENERATED_ASSETS: OnceLock<RwLock<HashMap<String, &'static [u8]>>> = OnceLock::new();

fn assets() -> &'static RwLock<HashMap<String, &'static [u8]>> {
    GENERATED_ASSETS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn get(path: &str) -> Option<&'static [u8]> {
    let map = assets().read().ok()?;
    map.get(path).copied()
}

pub fn insert(path: String, bytes: Vec<u8>) {
    let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let Ok(mut map) = assets().write() else {
        return;
    };
    map.insert(path, leaked);
}
