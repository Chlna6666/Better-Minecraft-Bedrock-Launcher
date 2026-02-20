use std::path::PathBuf;
use tauri::command;
use tracing::{error, info};
use crate::config::config::read_config;
use crate::utils::file_ops;
use crate::core::minecraft::gdk::stream::MsiXVDStream;
use crate::tasks::task_manager::{create_task, finish_task, is_cancelled, update_progress};

#[command]
pub async fn unpack_gdk(input_path: String, folder_name: String) -> Result<String, String> {
    // 1. 创建任务并获取 ID
    let task_id = create_task(None, "extracting", None);

    // 2. 准备参数以供后台任务使用
    let input_path_buf = PathBuf::from(input_path);
    let version_dir = file_ops::bmcbl_subdir("versions").join(&folder_name);
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

                // 根据配置决定是否删除下载的包（仅删除 BMCBL/downloads 下的文件，避免误删用户文件）
                if let Ok(cfg) = read_config() {
                    if !cfg.game.keep_downloaded_game_package {
                        let downloads_dir = file_ops::bmcbl_subdir("downloads");
                        let input_abs =
                            input_path_buf.canonicalize().unwrap_or_else(|_| input_path_buf.clone());
                        let downloads_abs =
                            downloads_dir.canonicalize().unwrap_or_else(|_| downloads_dir.clone());
                        if input_abs.starts_with(&downloads_abs) {
                            if let Err(e) = std::fs::remove_file(&input_abs) {
                                error!("GDK 解包完成后删除源文件失败: {}", e);
                            } else {
                                info!("GDK 解包完成后已删除源文件: {:?}", input_abs);
                            }
                        } else {
                            info!("GDK 解包完成：源文件不在 downloads 目录，跳过删除: {:?}", input_abs);
                        }
                    }
                }

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
