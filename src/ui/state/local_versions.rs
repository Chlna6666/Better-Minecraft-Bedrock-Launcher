use gpui::{Global, SharedString};
use std::sync::Arc;

use crate::core::version::launch_versions::LaunchVersionEntry;

pub struct LocalVersionsState {
    pub loaded: bool,
    pub loading: bool,
    pub refresh_pending: bool,
    pub error: Option<SharedString>,
    pub versions: Arc<[LaunchVersionEntry]>,
}

impl Default for LocalVersionsState {
    fn default() -> Self {
        Self {
            loaded: false,
            loading: false,
            refresh_pending: false,
            error: None,
            versions: Arc::default(),
        }
    }
}

impl Global for LocalVersionsState {}
