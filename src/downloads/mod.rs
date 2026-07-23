pub mod api;
mod integrity;
pub mod manager;
mod multi;
mod runtime;
mod single;

mod md5;
pub mod wu_client;

pub use manager::DownloadOptions;

const DOWNLOAD_TASK_STAGE_LABELS: [(&str, &str); 9] = [
    ("downloading", "下载中"),
    ("merging", "合并文件"),
    ("verifying", "校验中"),
    ("renaming", "整理文件"),
    ("resolving_url", "解析下载地址"),
    ("reading_body", "读取响应"),
    ("parsing", "解析中"),
    ("url_resolved", "已获取下载地址"),
    ("single_thread_fallback", "切换单线程"),
];

pub(crate) fn register_download_task_stage_labels() {
    crate::tasks::task_manager::register_task_stage_labels(DOWNLOAD_TASK_STAGE_LABELS);
}
