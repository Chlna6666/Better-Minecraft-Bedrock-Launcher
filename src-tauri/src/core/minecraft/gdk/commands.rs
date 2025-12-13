use std::path::PathBuf;
use tauri::command;
use tracing::{error, info};
use crate::core::minecraft::gdk::stream::MsiXVDStream;
use crate::tasks::task_manager::{create_task, finish_task, is_cancelled, update_progress};

#[command]
pub async fn unpack_gdk(input_path: String, folder_name: String) -> Result<String, String> {
    // 1. 创建任务并获取 ID
    let task_id = create_task(None, "extracting", None);

    // 2. 准备参数以供后台任务使用
    let input_path_buf = PathBuf::from(input_path);
    let version_dir = PathBuf::from("./BMCBL/versions").join(&folder_name);
    let task_id_clone = task_id.clone();

    info!("启动 GDK 解包任务: {}, 输出目录: {:?}", task_id, version_dir);

    // 3. 在后台线程中运行解包 (spawn_blocking 用于 CPU/IO 密集型任务)
    tokio::task::spawn_blocking(move || {
        // 更新初始状态
        update_progress(&task_id_clone, 0, None, Some("initializing"));

        if is_cancelled(&task_id_clone) {
            finish_task(&task_id_clone, "cancelled", Some("User cancelled before start".into()));
            return;
        }

        // 初始化 Stream
        let mut stream = match MsiXVDStream::new(&input_path_buf) {
            Ok(s) => s,
            Err(e) => {
                let msg = format!("GDK 文件解析错误: {}", e);
                error!("{}", msg);
                finish_task(&task_id_clone, "error", Some(msg));
                return;
            }
        };

        update_progress(&task_id_clone, 0, None, Some("extracting"));

        // 执行解压 (传入 task_id 用于进度和取消)
        match stream.extract_to(&version_dir, task_id_clone.clone()) {
            Ok(_) => {
                info!("GDK 解包任务完成: {}", task_id_clone);
                finish_task(&task_id_clone, "completed", None);
            },
            Err(e) => {
                if e == "cancelled" || is_cancelled(&task_id_clone) {
                    info!("GDK 解包任务已取消: {}", task_id_clone);
                    // 清理已解压的部分文件（可选，建议清理以防残留损坏数据）
                    if let Err(rm_err) = std::fs::remove_dir_all(&version_dir) {
                        error!("取消后清理目录失败: {}", rm_err);
                    }
                    finish_task(&task_id_clone, "cancelled", Some("User cancelled".into()));
                } else {
                    let msg = format!("解压过程出错: {}", e);
                    error!("{}", msg);
                    finish_task(&task_id_clone, "error", Some(msg));
                }
            }
        }
    });

    // 4. 立即返回 task_id
    Ok(task_id)
}