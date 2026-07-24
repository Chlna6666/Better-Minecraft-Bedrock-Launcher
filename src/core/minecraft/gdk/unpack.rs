use crate::core::minecraft::gdk::stream::MsiXVDStream;
use crate::tasks::runtime::{run_archive_blocking, spawn_archive_task};
use crate::tasks::task_manager::{
    create_task_with_details, finish_task, is_cancelled, update_progress,
};
use crate::utils::file_ops;
use std::path::PathBuf;
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
    info!(
        "start gdk unpack task: {}, input: {:?}, output: {:?}",
        task_id, input_path_buf, version_dir
    );

    spawn_archive_task(task_id.clone(), {
        let task_id = task_id.clone();
        async move {
            let task_id_for_error = task_id.clone();
            if let Err(error) = run_archive_blocking(move || {
                run_unpack_gdk_task(task_id, input_path_buf, folder_name, version_dir);
            })
            .await
            {
                finish_task(
                    &task_id_for_error,
                    "error",
                    Some(format!("GDK 解包工作线程异常结束: {error}")),
                );
            }
        }
    })?;

    Ok(task_id)
}

fn run_unpack_gdk_task(
    task_id: String,
    input_path: PathBuf,
    folder_name: String,
    version_dir: PathBuf,
) {
    update_progress(&task_id, 0, None, Some("initializing"));

    if is_cancelled(&task_id) {
        finish_task(&task_id, "cancelled", Some("cancelled before start".into()));
        return;
    }

    let mut stream = match MsiXVDStream::new(&input_path) {
        Ok(stream) => stream,
        Err(error) => {
            let message = format!("GDK file parse error: {error}");
            error!("{message}");
            finish_task(&task_id, "error", Some(message));
            return;
        }
    };

    update_progress(&task_id, 0, None, Some("extracting"));

    match stream.extract_to(&version_dir, task_id.clone()) {
        Ok(()) => {
            info!(
                "GDK 解包任务完成: task_id={}, folder_name={}, input={:?}, output={:?}",
                task_id, folder_name, input_path, version_dir
            );
            finish_task(
                &task_id,
                "completed",
                Some(format!("已安装到 {}", version_dir.display())),
            );
        }
        Err(error) if error == "cancelled" || is_cancelled(&task_id) => {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&version_dir) {
                error!(
                    path = %version_dir.display(),
                    ?cleanup_error,
                    "failed to remove cancelled GDK install directory"
                );
            }
            finish_task(&task_id, "cancelled", Some("user cancelled".into()));
        }
        Err(error) => {
            let message = format!("extract failed: {error}");
            error!("{message}");
            finish_task(&task_id, "error", Some(message));
        }
    }
}
