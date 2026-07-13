use crate::core::minecraft::gdk::stream::MsiXVDStream;
use crate::tasks::task_manager::{
    create_task_with_details, finish_task, is_cancelled, update_progress,
};
use crate::utils::file_ops;
use std::path::{Path, PathBuf};
use tracing::{error, info};

pub fn start_unpack_gdk_task(
    input_path: impl Into<PathBuf>,
    folder_name: &str,
) -> Result<String, String> {
    crate::core::minecraft::gdk::register_gdk_task_stage_labels();
    let task_id = create_task_with_details(
        None,
        "安装 GDK 游戏",
        Some(folder_name.to_string()),
        "initializing",
        None,
        false,
    );

    let input_path_buf = input_path.into();
    let folder_name = folder_name.to_string();
    let version_dir = file_ops::bmcbl_subdir("versions").join(&folder_name);
    let task_id_clone = task_id.clone();

    info!(
        "start gdk unpack task: {}, input: {:?}, output: {:?}",
        task_id, input_path_buf, version_dir
    );

    let _ = tokio::task::spawn_blocking(move || {
        update_progress(&task_id_clone, 0, None, Some("initializing"));

        if is_cancelled(&task_id_clone) {
            finish_task(
                &task_id_clone,
                "cancelled",
                Some("cancelled before start".into()),
            );
            return;
        }

        let mut stream = match MsiXVDStream::new(&input_path_buf) {
            Ok(s) => s,
            Err(e) => {
                let msg = format!("GDK file parse error: {e}");
                error!("{msg}");
                finish_task(&task_id_clone, "error", Some(msg));
                return;
            }
        };

        update_progress(&task_id_clone, 0, None, Some("extracting"));

        match stream.extract_to(&version_dir, task_id_clone.clone()) {
            Ok(()) => {
                info!(
                    "GDK 解包任务完成: task_id={}, folder_name={}, input={:?}, output={:?}",
                    task_id_clone, folder_name, input_path_buf, version_dir
                );
                finish_task(
                    &task_id_clone,
                    "completed",
                    Some(format!("已安装到 {}", version_dir.display())),
                );
            }
            Err(e) => {
                if e == "cancelled" || is_cancelled(&task_id_clone) {
                    let _ = std::fs::remove_dir_all(&version_dir);
                    finish_task(&task_id_clone, "cancelled", Some("user cancelled".into()));
                } else {
                    let msg = format!("extract failed: {e}");
                    error!("{msg}");
                    finish_task(&task_id_clone, "error", Some(msg));
                }
            }
        }
    });

    Ok(task_id)
}
