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
