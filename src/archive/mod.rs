pub mod api;
pub mod manager;
pub mod runtime;
pub mod zip;

const ARCHIVE_TASK_STAGE_LABELS: [(&str, &str); 3] = [
    ("extracting", "解压中"),
    ("preparing_files", "准备安装"),
    ("patching", "处理中"),
];

pub(crate) fn register_archive_task_stage_labels() {
    crate::tasks::task_manager::register_task_stage_labels(ARCHIVE_TASK_STAGE_LABELS);
}
