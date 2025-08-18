use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use once_cell::sync::Lazy;

pub static CANCEL_DOWNLOAD: Lazy<Arc<AtomicBool>> = Lazy::new(|| Arc::new(AtomicBool::new(false)));

pub fn is_cancelled() -> bool {
    CANCEL_DOWNLOAD.load(Ordering::Relaxed)
}
