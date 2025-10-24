use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use once_cell::sync::Lazy;

pub static GLOBAL_CANCEL_DOWNLOAD: Lazy<Arc<AtomicBool>> =
    Lazy::new(|| Arc::new(AtomicBool::new(false)));

pub fn is_global_cancelled() -> bool {
    GLOBAL_CANCEL_DOWNLOAD.load(Ordering::Relaxed)
}

/// 为单个下载任务新建取消标志（任务间互不影响）
pub fn new_cancel_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// 检查传入的取消标志（若为 None 则检查全局）
pub fn is_cancelled(flag: Option<&Arc<AtomicBool>>) -> bool {
    if let Some(f) = flag {
        f.load(Ordering::Relaxed)
    } else {
        is_global_cancelled()
    }
}
